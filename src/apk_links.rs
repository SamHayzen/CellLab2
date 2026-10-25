//! The APK's procedural link triangles and fragment shader.

use crate::sim::RenderLink;
use eframe::{
    egui::{self, Painter, Pos2, Rect, Vec2},
    egui_wgpu::{Callback, CallbackResources, CallbackTrait, ScreenDescriptor},
    wgpu,
};

struct GpuLinks {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    plate: LinkBatch,
    preview: LinkBatch,
}

struct LinkBatch {
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    view: wgpu::Buffer,
    capacity: usize,
    count: u32,
}

struct LinkCallback {
    viewport: Rect,
    format: wgpu::TextureFormat,
    links: Vec<[f32; 12]>,
    preview: bool,
}

pub fn paint(
    painter: &Painter,
    viewport: Rect,
    links: &[RenderLink],
    center: Pos2,
    scale: f32,
    format: wgpu::TextureFormat,
) {
    paint_pass(painter, viewport, links, center, scale, format, false);
}

pub fn paint_preview(
    painter: &Painter,
    viewport: Rect,
    links: &[RenderLink],
    center: Pos2,
    scale: f32,
    format: wgpu::TextureFormat,
) {
    paint_pass(painter, viewport, links, center, scale, format, true);
}

fn paint_pass(
    painter: &Painter,
    viewport: Rect,
    links: &[RenderLink],
    center: Pos2,
    scale: f32,
    format: wgpu::TextureFormat,
    preview: bool,
) {
    if links.is_empty() {
        return;
    }
    let clip = painter.clip_rect();
    let instances = links
        .iter()
        .filter_map(|link| {
            let bar = Vec2::new(link.right[0] - link.left[0], link.right[1] - link.left[1]);
            let length_squared = bar.length_sq();
            if length_squared < 1.0e-12 {
                return None;
            }
            let project = |point: [f32; 2]| {
                -1.0 + 2.0 * Vec2::new(point[0] - link.left[0], point[1] - link.left[1]).dot(bar)
                    / length_squared
            };
            let first = center + Vec2::new(link.from[0] * scale, link.from[1] * scale);
            let second = center + Vec2::new(link.to[0] * scale, link.to[1] * scale);
            let left = center + Vec2::new(link.left[0] * scale, link.left[1] * scale);
            let right = center + Vec2::new(link.right[0] * scale, link.right[1] * scale);
            let bounds = Rect::from_two_pos(first, second)
                .union(Rect::from_two_pos(left, right))
                .expand(1.0);
            if !clip.intersects(bounds) {
                return None;
            }
            Some([
                first.x,
                first.y,
                second.x,
                second.y,
                left.x,
                left.y,
                right.x,
                right.y,
                project(link.from),
                project(link.to),
                0.0,
                0.0,
            ])
        })
        .collect();
    painter.add(egui::Shape::Callback(Callback::new_paint_callback(
        viewport,
        LinkCallback {
            viewport,
            format,
            links: instances,
            preview,
        },
    )));
}

impl CallbackTrait for LinkCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<GpuLinks>().is_none() {
            resources.insert(create_gpu_links(device, self.format));
        }
        let gpu = resources.get_mut::<GpuLinks>().expect("link GPU resources");
        let batch = if self.preview {
            &mut gpu.preview
        } else {
            &mut gpu.plate
        };
        if self.links.len() > batch.capacity {
            batch.capacity = self.links.len().next_power_of_two();
            batch.instances = instance_buffer(device, batch.capacity);
            batch.bind_group = bind_group(device, &gpu.layout, &batch.instances, &batch.view);
        }
        batch.count = self.links.len() as u32;
        if !self.links.is_empty() {
            let mut bytes = Vec::with_capacity(self.links.len() * 48);
            for link in &self.links {
                for value in link {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            queue.write_buffer(&batch.instances, 0, &bytes);
        }
        let mut view_bytes = Vec::with_capacity(16);
        for value in [
            self.viewport.min.x,
            self.viewport.min.y,
            self.viewport.width().max(1.0),
            self.viewport.height().max(1.0),
        ] {
            view_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&batch.view, 0, &view_bytes);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        if let Some(gpu) = resources.get::<GpuLinks>() {
            let batch = if self.preview {
                &gpu.preview
            } else {
                &gpu.plate
            };
            if batch.count > 0 {
                pass.set_pipeline(&gpu.pipeline);
                pass.set_bind_group(0, &batch.bind_group, &[]);
                pass.draw(0..6, 0..batch.count);
            }
        }
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK link instances"),
        size: (capacity.max(1) * 48) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    instances: &wgpu::Buffer,
    view: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("APK link data"),
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
        ],
    })
}

fn create_gpu_links(device: &wgpu::Device, format: wgpu::TextureFormat) -> GpuLinks {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("APK link shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("apk_links.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("APK link layout"),
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
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("APK link pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("APK link pipeline"),
        layout: Some(&pipeline_layout),
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
    let plate = create_link_batch(device, &layout);
    let preview = create_link_batch(device, &layout);
    GpuLinks {
        pipeline,
        layout,
        plate,
        preview,
    }
}

fn create_link_batch(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> LinkBatch {
    let instances = instance_buffer(device, 1);
    let view = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK link viewport"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = bind_group(device, layout, &instances, &view);
    LinkBatch {
        bind_group,
        instances,
        view,
        capacity: 1,
        count: 0,
    }
}
