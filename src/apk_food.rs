//! GPU translation of the APK's food quad and circle shaders.

use crate::sim::RenderFood;
use eframe::{
    egui::{self, Painter, Pos2, Rect, Vec2},
    egui_wgpu::{Callback, CallbackResources, CallbackTrait, ScreenDescriptor},
    wgpu,
};

struct GpuFood {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    view: wgpu::Buffer,
    capacity: usize,
    count: u32,
}

struct FoodCallback {
    viewport: Rect,
    format: wgpu::TextureFormat,
    items: Vec<[f32; 8]>,
}

pub fn paint(
    painter: &Painter,
    viewport: Rect,
    food: &[RenderFood],
    center: Pos2,
    scale: f32,
    format: wgpu::TextureFormat,
) {
    if food.is_empty() {
        return;
    }
    let clip = painter.clip_rect();
    let items = food
        .iter()
        .filter_map(|item| {
            let pos = center + Vec2::new(item.x * scale, item.y * scale);
            let radius = 0.007 * (item.energy.max(0.0) / 1.152).sqrt() * scale;
            (radius > 0.0
                && clip.intersects(Rect::from_center_size(pos, Vec2::splat(2.0 * radius))))
            .then_some({
                let (red, green, blue) = if item.state <= 0.0 {
                    (0.6, 0.4, 0.2)
                } else {
                    (
                        0.8 + 0.2 * item.state,
                        0.4 * (1.0 - item.state),
                        0.2 * (1.0 - item.state),
                    )
                };
                [pos.x, pos.y, radius, 0.0, red, green, blue, 0.4]
            })
        })
        .collect();
    painter.add(egui::Shape::Callback(Callback::new_paint_callback(
        viewport,
        FoodCallback {
            viewport,
            format,
            items,
        },
    )));
}

impl CallbackTrait for FoodCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<GpuFood>().is_none() {
            resources.insert(create_gpu_food(device, self.format));
        }
        let gpu = resources.get_mut::<GpuFood>().expect("food GPU resources");
        if self.items.len() > gpu.capacity {
            gpu.capacity = self.items.len().next_power_of_two();
            gpu.instances = instance_buffer(device, gpu.capacity);
            gpu.bind_group = bind_group(device, &gpu.layout, &gpu.instances, &gpu.view);
        }
        gpu.count = self.items.len() as u32;
        if !self.items.is_empty() {
            let mut bytes = Vec::with_capacity(self.items.len() * 32);
            for item in &self.items {
                for value in item {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            queue.write_buffer(&gpu.instances, 0, &bytes);
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
        if let Some(gpu) = resources.get::<GpuFood>() {
            if gpu.count > 0 {
                pass.set_pipeline(&gpu.pipeline);
                pass.set_bind_group(0, &gpu.bind_group, &[]);
                pass.draw(0..6, 0..gpu.count);
            }
        }
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK food instances"),
        size: (capacity.max(1) * 32) as u64,
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
        label: Some("APK food data"),
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

fn create_gpu_food(device: &wgpu::Device, format: wgpu::TextureFormat) -> GpuFood {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("APK food shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("apk_food.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("APK food layout"),
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
        label: Some("APK food pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("APK food pipeline"),
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
    let instances = instance_buffer(device, 1);
    let view = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK food viewport"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = bind_group(device, &layout, &instances, &view);
    GpuFood {
        pipeline,
        layout,
        bind_group,
        instances,
        view,
        capacity: 1,
        count: 0,
    }
}
