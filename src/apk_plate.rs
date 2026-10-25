//! GPU translation of the APK's bg_vert.glsl and bg_frag.glsl.

use eframe::{
    egui::{self, Painter, Pos2, Rect},
    egui_wgpu::{Callback, CallbackResources, CallbackTrait, ScreenDescriptor},
    wgpu,
};

struct GpuPlate {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
}

pub struct PlateCallback {
    viewport: Rect,
    center: Pos2,
    radius: f32,
    light: [f32; 4],
    format: wgpu::TextureFormat,
}

pub fn paint(
    painter: &Painter,
    viewport: Rect,
    center: Pos2,
    radius: f32,
    angle: f32,
    amount: f32,
    range: f32,
    format: wgpu::TextureFormat,
) {
    let (sin, cos) = angle.sin_cos();
    painter.add(egui::Shape::Callback(Callback::new_paint_callback(
        viewport,
        PlateCallback {
            viewport,
            center,
            radius,
            light: [cos, sin, range, amount],
            format,
        },
    )));
}

impl CallbackTrait for PlateCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<GpuPlate>().is_none() {
            resources.insert(create_gpu_plate(device, self.format));
        }
        let gpu = resources.get::<GpuPlate>().expect("plate GPU resources");
        let values = [
            self.viewport.min.x,
            self.viewport.min.y,
            self.viewport.width().max(1.0),
            self.viewport.height().max(1.0),
            self.center.x,
            self.center.y,
            self.radius,
            0.0,
            self.light[0],
            self.light[1],
            self.light[2],
            self.light[3],
        ];
        let mut bytes = Vec::with_capacity(48);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&gpu.uniform, 0, &bytes);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        if let Some(gpu) = resources.get::<GpuPlate>() {
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &gpu.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

fn create_gpu_plate(device: &wgpu::Device, format: wgpu::TextureFormat) -> GpuPlate {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("APK plate shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("apk_plate.wgsl").into()),
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("APK plate light"),
        size: 48,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("APK plate layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("APK plate uniforms"),
        layout: &bind_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("APK plate pipeline layout"),
        bind_group_layouts: &[Some(&bind_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("APK plate pipeline"),
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
                // CellWorld.a() draws the plate with glBlendFunc(GL_ONE,
                // GL_SRC_ALPHA). Its shader adds warm light to the already
                // cleared pale-blue substrate instead of replacing that color.
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::SrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::SrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    });
    GpuPlate {
        pipeline,
        bind_group,
        uniform,
    }
}
