//! wgpu renderer for [`vmap_world::Scene`].
//!
//! The renderer always draws into an offscreen [`RenderTarget`]. The desktop
//! window shows that texture, and headless callers (command line, MCP) read
//! its pixels back, so both see exactly the same picture.

mod camera;
mod gpu_scene;
mod headless;

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use vmap_world::{Cull, Lighting};

pub use camera::{Camera, OrbitCamera};
pub use gpu_scene::GpuScene;
pub use headless::Headless;
pub use wgpu;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// A GPU device created without a window, for headless rendering.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
}

impl Gpu {
    pub fn headless() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| anyhow!("no usable graphics adapter: {e}"))?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = pollster::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor { label: Some("vmap"), ..Default::default() }),
        )
        .map_err(|e| anyhow!("could not create a graphics device: {e}"))?;
        Ok(Self { device, queue, adapter_name })
    }
}

/// Per-frame switches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    pub lighting: bool,
    pub textures: bool,
    pub vertex_colors: bool,
    pub markers: bool,
    /// Which kinds of marker to draw while `markers` is on, in the order
    /// of `MarkerKind::ALL`: NPCs, warps, triggers, interactables.
    pub marker_kinds: [bool; 4],
    /// Background colour; `None` uses the scene's sky colour.
    pub background: Option<[f32; 3]>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            lighting: true,
            textures: true,
            vertex_colors: true,
            markers: true,
            marker_kinds: [true; 4],
            background: None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [f32; 16],
    light_dir: [[f32; 4]; 4],
    light_color: [[f32; 4]; 4],
    options: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct MaterialUniform {
    pub diffuse: [f32; 4],
    pub ambient: [f32; 4],
    pub emission: [f32; 4],
    pub tex_row0: [f32; 4],
    pub tex_row1: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PipelineKey {
    pub cull: Cull,
    pub blend: bool,
}

/// Pipelines and layouts shared by every scene. Create once per device.
pub struct Renderer {
    pub(crate) samples: u32,
    pub(crate) material_layout: wgpu::BindGroupLayout,
    globals_buffer: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    box_pipeline: wgpu::RenderPipeline,
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let samples = 4;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vmap scene shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let uniform_entry = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buffer.as_entire_binding() }],
        });

        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let box_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("markers"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });

        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x2];
        let instance_attrs =
            wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let box_vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3];
        let box_instance_attrs = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4];

        let multisample = wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false };
        let depth = |write: bool| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };

        let mut pipelines = HashMap::new();
        for cull in [Cull::None, Cull::Back, Cull::Front] {
            for blend in [false, true] {
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("scene"),
                    layout: Some(&scene_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[
                            Some(wgpu::VertexBufferLayout {
                                array_stride: 44,
                                step_mode: wgpu::VertexStepMode::Vertex,
                                attributes: &vertex_attrs,
                            }),
                            Some(wgpu::VertexBufferLayout {
                                array_stride: 64,
                                step_mode: wgpu::VertexStepMode::Instance,
                                attributes: &instance_attrs,
                            }),
                        ],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: match cull {
                            Cull::Back => Some(wgpu::Face::Back),
                            Cull::Front => Some(wgpu::Face::Front),
                            _ => None,
                        },
                        ..Default::default()
                    },
                    depth_stencil: Some(depth(!blend)),
                    multisample,
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(if blend { "fs_blend" } else { "fs_opaque" }),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: COLOR_FORMAT,
                            blend: blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                pipelines.insert(PipelineKey { cull, blend }, pipeline);
            }
        }

        let box_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("markers"),
            layout: Some(&box_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_box"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &box_vertex_attrs,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 80,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &box_instance_attrs,
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth(false)),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_box"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self { samples, material_layout, globals_buffer, globals_bind, pipelines, box_pipeline }
    }

    /// Draws a scene into a target.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &GpuScene,
        target: &RenderTarget,
        camera: &Camera,
        options: &RenderOptions,
    ) {
        let lighting: &Lighting = &scene.lighting;
        let mut g = Globals {
            view_proj: camera.view_proj().to_cols_array(),
            light_dir: [[0.0; 4]; 4],
            light_color: [[0.0; 4]; 4],
            options: [
                options.lighting as u32 as f32,
                options.textures as u32 as f32,
                options.vertex_colors as u32 as f32,
                0.0,
            ],
        };
        for (i, l) in lighting.lights.iter().enumerate() {
            g.light_dir[i] = [l.direction.x, l.direction.y, l.direction.z, l.enabled as u32 as f32];
            g.light_color[i] = [l.color[0], l.color[1], l.color[2], 1.0];
        }
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&g));

        let bg = options.background.unwrap_or(lighting.clear_color);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("vmap frame") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&target.color_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0] as f64,
                            g: bg[1] as f64,
                            b: bg[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);

            if let (Some(vb), Some(ib)) = (&scene.vertex_buffer, &scene.instance_buffer) {
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_vertex_buffer(1, ib.slice(..));
                let mut current: Option<PipelineKey> = None;
                for d in &scene.draws {
                    if current != Some(d.pipeline) {
                        pass.set_pipeline(&self.pipelines[&d.pipeline]);
                        current = Some(d.pipeline);
                    }
                    pass.set_bind_group(1, scene.material_bind(d.material), &[]);
                    pass.draw(d.vertices.clone(), d.instances.clone());
                }
            }

            if options.markers {
                if let Some(mb) = &scene.marker_buffer {
                    pass.set_pipeline(&self.box_pipeline);
                    pass.set_vertex_buffer(0, scene.box_buffer.slice(..));
                    pass.set_vertex_buffer(1, mb.slice(..));
                    for (kind, range) in scene.marker_ranges.iter().enumerate() {
                        if options.marker_kinds[kind] && !range.is_empty() {
                            pass.draw(0..36, range.clone());
                        }
                    }
                }
            }
        }
        queue.submit([encoder.finish()]);
    }
}

/// Offscreen colour and depth buffers the renderer draws into.
pub struct RenderTarget {
    pub width: u32,
    pub height: u32,
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

impl RenderTarget {
    pub fn new(device: &wgpu::Device, renderer: &Renderer, width: u32, height: u32) -> Self {
        let (width, height) = (width.clamp(1, 8192), height.clamp(1, 8192));
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let make = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = make(
            "vmap colour",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let msaa = make("vmap msaa", COLOR_FORMAT, renderer.samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = make("vmap depth", DEPTH_FORMAT, renderer.samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
        Self {
            width,
            height,
            color_view: color.create_view(&Default::default()),
            msaa_view: msaa.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
        }
    }

    pub fn aspect(&self) -> f32 {
        self.width as f32 / self.height as f32
    }

    /// Reads the rendered image back as tightly packed RGBA rows.
    pub fn read_pixels(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Vec<u8>> {
        let row = self.width as usize * 4;
        let padded = row.div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vmap readback"),
            size: (padded * self.height as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("waiting for the GPU failed: {e}"))?;
        rx.recv()?.map_err(|e| anyhow!("reading the image back failed: {e}"))?;

        let data = slice.get_mapped_range().map_err(|e| anyhow!("reading the image back failed: {e}"))?;
        let mut out = Vec::with_capacity(row * self.height as usize);
        for y in 0..self.height as usize {
            out.extend_from_slice(&data[y * padded..y * padded + row]);
        }
        drop(data);
        buffer.unmap();
        Ok(out)
    }
}

pub(crate) fn mat4_cols(m: &Mat4) -> [f32; 16] {
    m.to_cols_array()
}
