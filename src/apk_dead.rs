//! GPU translation of the APK's dead-cell particle shader.

use crate::sim::RenderDead;
use eframe::{
    egui::{self, Painter, Pos2, Rect, Vec2},
    egui_wgpu::{Callback, CallbackResources, CallbackTrait, ScreenDescriptor},
    wgpu,
};

struct GpuDead {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    particles: wgpu::Buffer,
    view: wgpu::Buffer,
    lookup_view: wgpu::TextureView,
    lookup_sampler: wgpu::Sampler,
    _lookup: wgpu::Texture,
    capacity: usize,
    count: u32,
}

struct DeadCallback {
    viewport: Rect,
    format: wgpu::TextureFormat,
    particles: Vec<[f32; 12]>,
}

pub fn paint(
    painter: &Painter,
    viewport: Rect,
    dead: &[RenderDead],
    center: Pos2,
    scale: f32,
    format: wgpu::TextureFormat,
) {
    if dead.is_empty() {
        return;
    }
    let clip = painter.clip_rect();
    let particles = dead
        .iter()
        .filter_map(|cell| {
            let pos = center + Vec2::new(cell.x * scale, cell.y * scale);
            let world_half = 0.063111 * cell.radius / 0.03;
            let pixel_half = world_half * scale;
            (cell.radius > 0.0
                && clip.intersects(Rect::from_center_size(pos, Vec2::splat(2.0 * pixel_half))))
            .then_some([
                pos.x,
                pos.y,
                pixel_half,
                world_half,
                cell.color[0],
                cell.color[1],
                cell.color[2],
                1.0,
                cell.age,
                cell.radius,
                0.0,
                0.0,
            ])
        })
        .collect();
    painter.add(egui::Shape::Callback(Callback::new_paint_callback(
        viewport,
        DeadCallback {
            viewport,
            format,
            particles,
        },
    )));
}

impl CallbackTrait for DeadCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<GpuDead>().is_none() {
            resources.insert(create_gpu_dead(device, queue, self.format));
        }
        let gpu = resources
            .get_mut::<GpuDead>()
            .expect("dead-cell GPU resources");
        if self.particles.len() > gpu.capacity {
            gpu.capacity = self.particles.len().next_power_of_two();
            gpu.particles = particle_buffer(device, gpu.capacity);
            gpu.bind_group = bind_group(
                device,
                &gpu.layout,
                &gpu.particles,
                &gpu.view,
                &gpu.lookup_view,
                &gpu.lookup_sampler,
            );
        }
        gpu.count = self.particles.len() as u32;
        if !self.particles.is_empty() {
            let mut bytes = Vec::with_capacity(self.particles.len() * 48);
            for particle in &self.particles {
                for value in particle {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            queue.write_buffer(&gpu.particles, 0, &bytes);
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
        queue.write_buffer(&gpu.view, 0, &view_bytes);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        if let Some(gpu) = resources.get::<GpuDead>() {
            if gpu.count > 0 {
                pass.set_pipeline(&gpu.pipeline);
                pass.set_bind_group(0, &gpu.bind_group, &[]);
                pass.draw(0..6, 0..gpu.count);
            }
        }
    }
}

fn particle_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK dead particles"),
        size: (capacity.max(1) * 48) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    particles: &wgpu::Buffer,
    view: &wgpu::Buffer,
    lookup_view: &wgpu::TextureView,
    lookup_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("APK dead shader data"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: particles.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: view.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(lookup_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(lookup_sampler),
            },
        ],
    })
}

fn create_gpu_dead(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
) -> GpuDead {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("APK dead-cell shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("apk_dead.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("APK dead-cell layout"),
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
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("APK dead-cell pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("APK dead-cell pipeline"),
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
    let particles = particle_buffer(device, 1);
    let view = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK dead-cell viewport"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let image =
        crate::visuals::decode_png(include_bytes!("../assets/ui/gauss_bessel_integral.png"))
            .expect("bundled APK dead-cell texture");
    let lookup = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("APK gauss_bessel_integral.png"),
        size: wgpu::Extent3d {
            width: image.size[0] as u32,
            height: image.size[1] as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_array())
        .collect();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &lookup,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.size[0] as u32 * 4),
            rows_per_image: Some(image.size[1] as u32),
        },
        wgpu::Extent3d {
            width: image.size[0] as u32,
            height: image.size[1] as u32,
            depth_or_array_layers: 1,
        },
    );
    let lookup_view = lookup.create_view(&wgpu::TextureViewDescriptor::default());
    let lookup_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("APK dead-cell sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bind_group = bind_group(
        device,
        &layout,
        &particles,
        &view,
        &lookup_view,
        &lookup_sampler,
    );
    GpuDead {
        pipeline,
        layout,
        bind_group,
        particles,
        view,
        lookup_view,
        lookup_sampler,
        _lookup: lookup,
        capacity: 1,
        count: 0,
    }
}
