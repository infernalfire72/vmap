//! Uploads a [`Scene`] to the GPU: textures with mip chains, one uniform
//! block per material, one shared vertex buffer and instance transforms.

use std::collections::HashMap;
use std::ops::Range;

use glam::Mat4;
use vmap_formats::nitro::Image;
use vmap_world::{Animations, Cull, Lighting, MarkerKind, Scene, Wrap};
use wgpu::util::DeviceExt;

use crate::{mat4_cols, MaterialUniform, PipelineKey, Renderer};

pub(crate) struct Draw {
    pub pipeline: PipelineKey,
    pub material: usize,
    pub vertices: Range<u32>,
    pub instances: Range<u32>,
}

pub struct GpuScene {
    pub(crate) vertex_buffer: Option<wgpu::Buffer>,
    pub(crate) instance_buffer: Option<wgpu::Buffer>,
    material_binds: Vec<wgpu::BindGroup>,
    /// Per texture swap animation: one bind group per key frame.
    pattern_binds: Vec<Vec<wgpu::BindGroup>>,
    /// The (animation, key frame) whose bind group currently stands in for
    /// a material's own.
    material_override: Vec<Option<(usize, usize)>>,
    uniform_buffer: wgpu::Buffer,
    uniform_block: usize,
    uniforms: Vec<MaterialUniform>,
    /// (first vertex, vertex count) of each batch of each model.
    batch_ranges: Vec<Vec<(u32, u32)>>,
    animations: Animations,
    animated_at: Option<f32>,
    pub(crate) draws: Vec<Draw>,
    pub(crate) box_buffer: wgpu::Buffer,
    pub(crate) marker_buffer: Option<wgpu::Buffer>,
    /// Instances of each kind of marker, in the order of `MarkerKind::ALL`.
    pub(crate) marker_ranges: [Range<u32>; 4],
    pub lighting: Lighting,
    _textures: Vec<wgpu::Texture>,
}

/// Colour used for each kind of marker (RGBA).
pub fn marker_color(kind: MarkerKind) -> [f32; 4] {
    match kind {
        MarkerKind::Npc => [0.20, 0.55, 1.00, 0.85],
        MarkerKind::Warp => [1.00, 0.35, 0.75, 0.80],
        MarkerKind::Trigger => [1.00, 0.80, 0.15, 0.55],
        MarkerKind::Furniture => [0.30, 0.90, 0.45, 0.80],
    }
}

impl GpuScene {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &Renderer, scene: &Scene) -> Self {
        // Textures. Index `textures.len()` is a white fallback.
        let mut textures = Vec::with_capacity(scene.textures.len() + 1);
        let mut views = Vec::with_capacity(scene.textures.len() + 1);
        for t in &scene.textures {
            let tex = upload_texture(device, queue, &t.image);
            views.push(tex.create_view(&Default::default()));
            textures.push(tex);
        }
        let white = upload_texture(device, queue, &Image { width: 1, height: 1, rgba: vec![255; 4] });
        let white_view = white.create_view(&Default::default());
        textures.push(white);

        let mut samplers: HashMap<(Wrap, Wrap), wgpu::Sampler> = HashMap::new();
        let address = |w: Wrap| match w {
            Wrap::Clamp => wgpu::AddressMode::ClampToEdge,
            Wrap::Repeat => wgpu::AddressMode::Repeat,
            Wrap::Mirror => wgpu::AddressMode::MirrorRepeat,
        };

        // One uniform block per material, all in a single buffer.
        let align = device.limits().min_uniform_buffer_offset_alignment.max(1) as usize;
        let block = std::mem::size_of::<MaterialUniform>().div_ceil(align) * align;
        let anims = &scene.animations;
        let mut uniform_data = vec![0u8; block * scene.materials.len().max(1)];
        let mut uniforms = Vec::with_capacity(scene.materials.len());
        for (i, m) in scene.materials.iter().enumerate() {
            // A material that only gets its texture from a swap animation
            // is still textured.
            let textured = m.texture.is_some() || anims.tex_pattern.iter().any(|a| a.material == i);
            let u = MaterialUniform {
                diffuse: [m.diffuse[0], m.diffuse[1], m.diffuse[2], m.alpha],
                ambient: [m.ambient[0], m.ambient[1], m.ambient[2], textured as u32 as f32],
                emission: [m.emission[0], m.emission[1], m.emission[2], m.lights as f32],
                tex_row0: [m.tex_matrix[0][0], m.tex_matrix[0][1], m.tex_matrix[0][2], 0.0],
                tex_row1: [m.tex_matrix[1][0], m.tex_matrix[1][1], m.tex_matrix[1][2], 0.0],
            };
            uniform_data[i * block..i * block + std::mem::size_of::<MaterialUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&u));
            uniforms.push(u);
        }
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vmap materials"),
            contents: &uniform_data,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        for m in &scene.materials {
            samplers.entry((m.wrap_s, m.wrap_t)).or_insert_with(|| {
                device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("vmap sampler"),
                    address_mode_u: address(m.wrap_s),
                    address_mode_v: address(m.wrap_t),
                    mag_filter: wgpu::FilterMode::Nearest,
                    min_filter: wgpu::FilterMode::Linear,
                    mipmap_filter: wgpu::MipmapFilterMode::Linear,
                    ..Default::default()
                })
            });
        }
        // A material's uniforms and sampler with a given texture.
        let bind = |i: usize, texture: Option<usize>| {
            let m = &scene.materials[i];
            let view = texture.and_then(|t| views.get(t)).unwrap_or(&white_view);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vmap material"),
                layout: &renderer.material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &uniform_buffer,
                            offset: (i * block) as u64,
                            size: wgpu::BufferSize::new(std::mem::size_of::<MaterialUniform>() as u64),
                        }),
                    },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&samplers[&(m.wrap_s, m.wrap_t)]),
                    },
                ],
            })
        };
        let material_binds: Vec<_> = scene.materials.iter().enumerate().map(|(i, m)| bind(i, m.texture)).collect();
        let pattern_binds: Vec<Vec<_>> = anims
            .tex_pattern
            .iter()
            .map(|a| a.keys.iter().map(|&(_, texture)| bind(a.material, Some(texture))).collect())
            .collect();

        // Instances grouped by model, so each batch is one instanced draw.
        let mut by_model: Vec<Vec<[f32; 16]>> = vec![Vec::new(); scene.models.len()];
        for n in &scene.nodes {
            by_model[n.model].push(mat4_cols(&n.transform));
        }
        let mut instance_data: Vec<[f32; 16]> = Vec::with_capacity(scene.nodes.len());
        let mut instance_ranges = Vec::with_capacity(scene.models.len());
        for list in &by_model {
            let start = instance_data.len() as u32;
            instance_data.extend_from_slice(list);
            instance_ranges.push(start..instance_data.len() as u32);
        }

        let total: usize = scene.models.iter().flat_map(|m| &m.batches).map(|b| b.vertices.len()).sum();
        let mut vertex_data: Vec<u8> = Vec::with_capacity(total * 44);
        let mut draws = Vec::new();
        let mut cursor = 0u32;
        let mut batch_ranges: Vec<Vec<(u32, u32)>> = Vec::with_capacity(scene.models.len());
        for (mi, model) in scene.models.iter().enumerate() {
            batch_ranges.push(Vec::with_capacity(model.batches.len()));
            for b in &model.batches {
                let count = b.vertices.len() as u32;
                vertex_data.extend_from_slice(bytemuck::cast_slice(&b.vertices));
                let range = cursor..cursor + count;
                batch_ranges[mi].push((cursor, count));
                cursor += count;
                let mat = &scene.materials[b.material];
                if instance_ranges[mi].is_empty() || mat.cull == Cull::Both || count == 0 {
                    continue;
                }
                draws.push(Draw {
                    pipeline: PipelineKey { cull: mat.cull, blend: mat.translucent },
                    material: b.material,
                    vertices: range,
                    instances: instance_ranges[mi].clone(),
                });
            }
        }
        // Opaque geometry first, then everything that blends.
        draws.sort_by_key(|d| (d.pipeline.blend, d.pipeline.cull as u8));

        // Writable, because moving joints rewrite their vertices.
        let buffer = |label, contents: &[u8]| {
            (!contents.is_empty()).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                })
            })
        };
        let vertex_buffer = buffer("vmap vertices", &vertex_data);
        let instance_buffer = buffer("vmap instances", bytemuck::cast_slice(&instance_data));

        // Markers: a unit box stretched over each footprint.
        // Grouped by kind, so each kind can be switched off on its own.
        let mut marker_data: Vec<[f32; 20]> = Vec::with_capacity(scene.markers.len());
        let mut marker_ranges: [Range<u32>; 4] = Default::default();
        for kind in MarkerKind::ALL {
            let start = marker_data.len() as u32;
            for m in scene.markers.iter().filter(|m| m.kind == kind) {
                let (min, size) = m.bounds();
                let xf = Mat4::from_translation(min) * Mat4::from_scale(size);
                let mut row = [0f32; 20];
                row[..16].copy_from_slice(&mat4_cols(&xf));
                row[16..].copy_from_slice(&marker_color(m.kind));
                marker_data.push(row);
            }
            marker_ranges[kind.index()] = start..marker_data.len() as u32;
        }
        let marker_buffer = buffer("vmap markers", bytemuck::cast_slice(&marker_data));
        let box_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vmap box"),
            contents: bytemuck::cast_slice(&unit_box()),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            vertex_buffer,
            instance_buffer,
            material_override: vec![None; material_binds.len()],
            material_binds,
            pattern_binds,
            uniform_buffer,
            uniform_block: block,
            uniforms,
            batch_ranges,
            animations: scene.animations.clone(),
            animated_at: None,
            draws,
            box_buffer,
            marker_buffer,
            marker_ranges,
            lighting: scene.lighting,
            _textures: textures,
        }
    }

    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    /// Whether anything in the scene moves by itself.
    pub fn has_animations(&self) -> bool {
        !self.animations.is_empty()
    }

    /// Puts everything animated into its state at `time` seconds.
    pub fn animate(&mut self, queue: &wgpu::Queue, time: f32) {
        if self.animations.is_empty() || self.animated_at == Some(time) {
            return;
        }
        self.animated_at = Some(time);

        for a in &self.animations.tex_srt {
            let Some(u) = self.uniforms.get_mut(a.material) else { continue };
            let m = a.matrix(time);
            let rows = ([m[0][0], m[0][1], m[0][2], 0.0], [m[1][0], m[1][1], m[1][2], 0.0]);
            if (u.tex_row0, u.tex_row1) != rows {
                (u.tex_row0, u.tex_row1) = rows;
                queue.write_buffer(&self.uniform_buffer, (a.material * self.uniform_block) as u64, bytemuck::bytes_of(u));
            }
        }

        for (i, a) in self.animations.tex_pattern.iter().enumerate() {
            if let Some(slot) = self.material_override.get_mut(a.material) {
                *slot = Some((i, a.key_at(time)));
            }
        }

        if let Some(vb) = &self.vertex_buffer {
            let stride = std::mem::size_of::<vmap_formats::nitro::GeoVertex>() as u64;
            for a in &self.animations.joints {
                let Some(ranges) = self.batch_ranges.get(a.model) else { continue };
                let batches = a.vertices(time);
                // A pose never changes how many vertices a model has; if the
                // counts disagree the animation belongs to another model.
                if batches.len() != ranges.len() {
                    continue;
                }
                for (vertices, &(first, count)) in batches.iter().zip(ranges) {
                    if vertices.len() as u32 == count && count > 0 {
                        queue.write_buffer(vb, first as u64 * stride, bytemuck::cast_slice(vertices));
                    }
                }
            }
        }
    }

    pub(crate) fn material_bind(&self, material: usize) -> &wgpu::BindGroup {
        match self.material_override[material] {
            Some((animation, key)) => &self.pattern_binds[animation][key],
            None => &self.material_binds[material],
        }
    }
}

/// 36 vertices of a box spanning 0..1 on every axis.
fn unit_box() -> Vec<[f32; 3]> {
    let c = |i: usize| [(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32];
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut v = Vec::with_capacity(36);
    for q in quads {
        for i in [0, 1, 2, 0, 2, 3] {
            v.push(c(q[i]));
        }
    }
    v
}

fn upload_texture(device: &wgpu::Device, queue: &wgpu::Queue, image: &Image) -> wgpu::Texture {
    let mips = build_mips(image);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vmap texture"),
        size: wgpu::Extent3d { width: image.width, height: image.height, depth_or_array_layers: 1 },
        mip_level_count: mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, m) in mips.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &m.rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(m.width * 4), rows_per_image: Some(m.height) },
            wgpu::Extent3d { width: m.width, height: m.height, depth_or_array_layers: 1 },
        );
    }
    texture
}

/// Box-filtered mip chain. Colour is weighted by alpha so transparent texels
/// do not bleed their (usually black) colour into the visible ones.
fn build_mips(image: &Image) -> Vec<Image> {
    let mut mips = vec![image.clone()];
    loop {
        let prev = mips.last().unwrap();
        if prev.width <= 1 && prev.height <= 1 {
            break;
        }
        let (w, h) = ((prev.width / 2).max(1), (prev.height / 2).max(1));
        let mut next = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 3];
                let mut alpha = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(prev.width - 1);
                    let sy = (y * 2 + dy).min(prev.height - 1);
                    let i = ((sy * prev.width + sx) * 4) as usize;
                    let a = prev.rgba[i + 3] as u32;
                    alpha += a;
                    for c in 0..3 {
                        acc[c] += prev.rgba[i + c] as u32 * a;
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                if alpha > 0 {
                    for c in 0..3 {
                        next.rgba[o + c] = (acc[c] / alpha) as u8;
                    }
                }
                next.rgba[o + 3] = (alpha / 4) as u8;
            }
        }
        mips.push(next);
    }
    mips
}
