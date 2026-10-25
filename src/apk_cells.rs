//! GPU translation of the APK's cell, lip, ker and gamete fragment shaders.
//! Each organism is one instanced quad; the fragment stage evaluates the
//! original membrane and nucleus equations at every pixel.

use crate::sim::RenderCell;
use eframe::{
    egui::{self, Painter, Pos2, Rect, Vec2},
    egui_wgpu::{Callback, CallbackResources, CallbackTrait, ScreenDescriptor},
    wgpu,
};

#[derive(Clone, Copy)]
struct Instance {
    center: [f32; 2],
    radius: f32,
    scale: f32,
    color: [f32; 4],
    kind: u32,
    angle: f32,
    phase: f32,
    reserve: f32,
    signals: [f32; 2],
    activity: [f32; 2],
    animation_phase: f32,
    padding: [f32; 3],
    auxiliary: [f32; 4],
    clip_start: u32,
    clip_count: u32,
    clip_padding: [u32; 2],
}

impl Instance {
    fn write_bytes(self, output: &mut Vec<u8>) {
        for value in self.center {
            output.extend_from_slice(&value.to_le_bytes());
        }
        output.extend_from_slice(&self.radius.to_le_bytes());
        output.extend_from_slice(&self.scale.to_le_bytes());
        for value in self.color {
            output.extend_from_slice(&value.to_le_bytes());
        }
        output.extend_from_slice(&self.kind.to_le_bytes());
        output.extend_from_slice(&self.angle.to_le_bytes());
        output.extend_from_slice(&self.phase.to_le_bytes());
        output.extend_from_slice(&self.reserve.to_le_bytes());
        for value in self.signals.into_iter().chain(self.activity) {
            output.extend_from_slice(&value.to_le_bytes());
        }
        output.extend_from_slice(&self.animation_phase.to_le_bytes());
        for value in self.padding {
            output.extend_from_slice(&value.to_le_bytes());
        }
        for value in self.auxiliary {
            output.extend_from_slice(&value.to_le_bytes());
        }
        for value in [self.clip_start, self.clip_count]
            .into_iter()
            .chain(self.clip_padding)
        {
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
}

struct GpuCells {
    marker_pipeline: wgpu::RenderPipeline,
    body_pipeline: wgpu::RenderPipeline,
    appendage_pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    plate: CellBatch,
    preview: CellBatch,
    photo_view: wgpu::TextureView,
    photo_sampler: wgpu::Sampler,
    _photo_texture: wgpu::Texture,
}

struct CellBatch {
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    view: wgpu::Buffer,
    clips: wgpu::Buffer,
    capacity: usize,
    clip_capacity: usize,
    count: u32,
}

pub struct CellCallback {
    viewport: Rect,
    format: wgpu::TextureFormat,
    cells: Vec<Instance>,
    clips: Vec<[f32; 4]>,
    preview: bool,
}

pub fn paint(
    painter: &Painter,
    viewport: Rect,
    cells: &[RenderCell],
    center: Pos2,
    scale: f32,
    format: wgpu::TextureFormat,
) {
    paint_pass(painter, viewport, cells, center, scale, format, None);
}

pub fn paint_preview(
    painter: &Painter,
    viewport: Rect,
    cells: &[RenderCell],
    center: Pos2,
    scale: f32,
    format: wgpu::TextureFormat,
    selected_mode: usize,
) {
    paint_pass(
        painter,
        viewport,
        cells,
        center,
        scale,
        format,
        Some(selected_mode),
    );
}

fn paint_pass(
    painter: &Painter,
    viewport: Rect,
    cells: &[RenderCell],
    center: Pos2,
    scale: f32,
    format: wgpu::TextureFormat,
    selected_mode: Option<usize>,
) {
    let clip = painter.clip_rect();
    let mut instances = Vec::with_capacity(cells.len());
    let mut clips = Vec::new();
    for cell in cells {
        let pos = center + Vec2::new(cell.x * scale, cell.y * scale);
        let radius = cell.radius * scale;
        // Match appendage_reach() in apk_cells.wgsl. The body's bounds alone
        // can hide a visible tail or spike when its center leaves the view.
        let appendage = match cell.cell_type {
            1 => 4.0 * radius,
            3 | 8 => radius + scale * 0.009,
            7 | 10 | 17 => radius + scale * 0.012,
            9 => radius + scale * 0.0135,
            11 => radius + scale * 0.006,
            _ => radius,
        };
        let visible_radius = appendage.max(
            radius
                + if selected_mode.is_some() {
                    scale * 0.015
                } else {
                    0.0
                },
        );
        if radius <= 0.0
            || !clip.intersects(Rect::from_center_size(
                pos,
                Vec2::splat(visible_radius * 2.0),
            ))
        {
            continue;
        }
        let clip_start = clips.len() as u32;
        for plane in &cell.contact_chords {
            clips.push([plane[0], plane[1], plane[2] * 50.0, 0.0]);
        }
        instances.push(Instance {
            center: [pos.x, pos.y],
            radius,
            scale,
            color: [cell.color[0], cell.color[1], cell.color[2], 1.0],
            kind: cell.cell_type.max(0) as u32,
            angle: cell.angle,
            phase: cell.phase,
            reserve: cell.reserve,
            signals: cell.signals,
            activity: cell.activity,
            animation_phase: cell.animation_phase,
            padding: [
                selected_mode.is_some_and(|mode| cell.mode == mode as i32) as u8 as f32,
                0.0,
                0.0,
            ],
            auxiliary: cell.auxiliary,
            clip_start,
            clip_count: cell.contact_chords.len() as u32,
            clip_padding: [0; 2],
        });
    }
    let callback = Callback::new_paint_callback(
        viewport,
        CellCallback {
            viewport,
            format,
            cells: instances,
            clips,
            preview: selected_mode.is_some(),
        },
    );
    painter.add(egui::Shape::Callback(callback));
}

impl CallbackTrait for CellCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<GpuCells>().is_none() {
            resources.insert(create_gpu_cells(device, queue, self.format));
        }
        let gpu = resources.get_mut::<GpuCells>().expect("cell GPU resources");
        let batch = if self.preview {
            &mut gpu.preview
        } else {
            &mut gpu.plate
        };
        let mut rebuild_bind_group = false;
        if self.cells.len() > batch.capacity {
            batch.capacity = self.cells.len().next_power_of_two();
            batch.instances = instance_buffer(device, batch.capacity);
            rebuild_bind_group = true;
        }
        if self.clips.len() > batch.clip_capacity {
            batch.clip_capacity = self.clips.len().next_power_of_two();
            batch.clips = clip_buffer(device, batch.clip_capacity);
            rebuild_bind_group = true;
        }
        if rebuild_bind_group {
            batch.bind_group = create_bind_group(
                device,
                &gpu.bind_layout,
                &batch.instances,
                &batch.view,
                &gpu.photo_view,
                &gpu.photo_sampler,
                &batch.clips,
            );
        }
        batch.count = self.cells.len() as u32;
        if !self.cells.is_empty() {
            let mut bytes = Vec::with_capacity(self.cells.len() * 112);
            for cell in &self.cells {
                cell.write_bytes(&mut bytes);
            }
            debug_assert_eq!(bytes.len(), self.cells.len() * 112);
            queue.write_buffer(&batch.instances, 0, &bytes);
        }
        if !self.clips.is_empty() {
            let mut bytes = Vec::with_capacity(self.clips.len() * 16);
            for plane in &self.clips {
                for value in plane {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            queue.write_buffer(&batch.clips, 0, &bytes);
        }
        let mut view = Vec::with_capacity(16);
        for value in [
            self.viewport.min.x,
            self.viewport.min.y,
            self.viewport.width().max(1.0),
            self.viewport.height().max(1.0),
        ] {
            view.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&batch.view, 0, &view);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(gpu) = resources.get::<GpuCells>() else {
            return;
        };
        let batch = if self.preview {
            &gpu.preview
        } else {
            &gpu.plate
        };
        if batch.count > 0 {
            pass.set_bind_group(0, &batch.bind_group, &[]);
            if self.preview {
                pass.set_pipeline(&gpu.marker_pipeline);
                pass.draw(0..6, 0..batch.count);
            }
            pass.set_pipeline(&gpu.body_pipeline);
            pass.draw(0..6, 0..batch.count);
            pass.set_pipeline(&gpu.appendage_pipeline);
            pass.draw(0..6, 0..batch.count);
        }
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK cell instances"),
        size: (capacity.max(1) * 112) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn clip_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK cell contact chords"),
        size: (capacity.max(1) * 16) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    instances: &wgpu::Buffer,
    view: &wgpu::Buffer,
    photo_view: &wgpu::TextureView,
    photo_sampler: &wgpu::Sampler,
    clips: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("APK cell data"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: instances.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: view.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(photo_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(photo_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: clips.as_entire_binding(),
            },
        ],
    })
}

fn create_gpu_cells(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
) -> GpuCells {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("APK cell body shaders"),
        source: wgpu::ShaderSource::Wgsl(include_str!("apk_cells.wgsl").into()),
    });
    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("APK cell layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("APK cell pipeline layout"),
        bind_group_layouts: &[Some(&bind_layout)],
        immediate_size: 0,
    });
    let marker_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("APK preview marker pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("marker_vertex"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("marker_fragment"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    });
    let body_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("APK cell pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    });
    let appendage_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("APK cell appendage pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("appendage_vertex"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("appendage_fragment"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    });
    let dots = crate::visuals::decode_png(include_bytes!("../assets/ui/small_dots.png"))
        .expect("bundled APK photocyte texture");
    let photo_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("APK small_dots.png"),
        size: wgpu::Extent3d {
            width: dots.size[0] as u32,
            height: dots.size[1] as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let rgba: Vec<u8> = dots
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_array())
        .collect();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &photo_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(dots.size[0] as u32 * 4),
            rows_per_image: Some(dots.size[1] as u32),
        },
        wgpu::Extent3d {
            width: dots.size[0] as u32,
            height: dots.size[1] as u32,
            depth_or_array_layers: 1,
        },
    );
    let photo_view = photo_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let photo_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("APK photocyte sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let plate = create_cell_batch(device, &bind_layout, &photo_view, &photo_sampler);
    let preview = create_cell_batch(device, &bind_layout, &photo_view, &photo_sampler);
    GpuCells {
        marker_pipeline,
        body_pipeline,
        appendage_pipeline,
        bind_layout,
        plate,
        preview,
        photo_view,
        photo_sampler,
        _photo_texture: photo_texture,
    }
}

fn create_cell_batch(
    device: &wgpu::Device,
    bind_layout: &wgpu::BindGroupLayout,
    photo_view: &wgpu::TextureView,
    photo_sampler: &wgpu::Sampler,
) -> CellBatch {
    let instances = instance_buffer(device, 1);
    let clips = clip_buffer(device, 1);
    let view = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK cell viewport"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = create_bind_group(
        device,
        bind_layout,
        &instances,
        &view,
        photo_view,
        photo_sampler,
        &clips,
    );
    CellBatch {
        bind_group,
        instances,
        view,
        clips,
        capacity: 1,
        clip_capacity: 1,
        count: 0,
    }
}
