//! Runs a model's render commands and display lists the way the console
//! would, producing plain triangle lists grouped by material.

use glam::{Mat4, Vec3};

use crate::nitro::gx::{self, for_each_command, sext};
use crate::nitro::mdl::{mat4_from_4x3, sbc_len, Model};
use crate::nitro::tex::rgb555_f32;

/// One vertex of the output triangle list.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GeoVertex {
    pub pos: [f32; 3],
    /// Unit normal, or zero when the vertex is coloured rather than lit.
    pub normal: [f32; 3],
    pub color: [f32; 3],
    /// Texture coordinates normalised by the material's texture size.
    pub uv: [f32; 2],
}

/// All triangles that use one material.
#[derive(Debug, Clone, Default)]
pub struct GeoBatch {
    pub material: usize,
    pub vertices: Vec<GeoVertex>,
}

#[derive(Debug, Clone, Default)]
pub struct Geometry {
    pub batches: Vec<GeoBatch>,
    pub min: Vec3,
    pub max: Vec3,
}

impl Geometry {
    pub fn triangle_count(&self) -> usize {
        self.batches.iter().map(|b| b.vertices.len() / 3).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.batches.iter().all(|b| b.vertices.is_empty())
    }
}

/// Evaluates a model. `pose` replaces the local matrix of each node (for
/// animation); `None` uses the bind pose stored in the model.
pub fn evaluate(model: &Model, pose: Option<&[Mat4]>) -> Geometry {
    let mut ev = Evaluator {
        model,
        cur: Mat4::IDENTITY,
        stack: [Mat4::IDENTITY; 32],
        material: 0,
        visible: true,
        batches: Vec::new(),
        slot: vec![usize::MAX; model.materials.len().max(1)],
    };
    ev.run(pose);

    let mut geo = Geometry { batches: ev.batches, min: Vec3::splat(f32::MAX), max: Vec3::splat(f32::MIN) };
    for v in geo.batches.iter().flat_map(|b| &b.vertices) {
        let p = Vec3::from(v.pos);
        geo.min = geo.min.min(p);
        geo.max = geo.max.max(p);
    }
    if geo.is_empty() {
        geo.min = Vec3::ZERO;
        geo.max = Vec3::ZERO;
    }
    geo
}

struct Evaluator<'a> {
    model: &'a Model,
    cur: Mat4,
    stack: [Mat4; 32],
    material: usize,
    visible: bool,
    batches: Vec<GeoBatch>,
    /// Material index to batch index.
    slot: Vec<usize>,
}

impl Evaluator<'_> {
    fn run(&mut self, pose: Option<&[Mat4]>) {
        let model = self.model;
        let sbc = &model.sbc;
        let mut i = 0;
        while i < sbc.len() {
            let op = sbc[i] & 0x1F;
            let flags = sbc[i] >> 5;
            let arg = |n: usize| sbc.get(i + n).copied().unwrap_or(0);
            let Some(len) = sbc_len(op, flags, arg(2)) else { break };
            match op {
                0x01 => break,
                0x02 => self.visible = arg(2) != 0,
                0x03 => self.cur = self.stack[(arg(1) & 31) as usize],
                0x04 => self.material = arg(1) as usize,
                0x05 => {
                    if self.visible {
                        self.draw_shape(arg(1) as usize);
                    }
                }
                0x06 => {
                    let node = arg(1) as usize;
                    let (mut store, mut restore) = (None, None);
                    let mut k = 4;
                    if flags & 1 != 0 {
                        store = Some((arg(k) & 31) as usize);
                        k += 1;
                    }
                    if flags & 2 != 0 {
                        restore = Some((arg(k) & 31) as usize);
                    }
                    if let Some(r) = restore {
                        self.cur = self.stack[r];
                    }
                    let local = pose
                        .and_then(|p| p.get(node).copied())
                        .or_else(|| model.nodes.get(node).map(|n| n.local_matrix()))
                        .unwrap_or(Mat4::IDENTITY);
                    self.cur *= local;
                    if let Some(s) = store {
                        self.stack[s] = self.cur;
                    }
                }
                0x09 => {
                    // Weighted blend of several joint matrices.
                    let dest = (arg(1) & 31) as usize;
                    let count = arg(2) as usize;
                    let mut acc = Mat4::ZERO;
                    for n in 0..count {
                        let src = (arg(3 + n * 3) & 31) as usize;
                        let node = arg(4 + n * 3) as usize;
                        let weight = arg(5 + n * 3) as f32 / 256.0;
                        let inv = model.inv_binds.get(node).copied().unwrap_or(Mat4::IDENTITY);
                        acc += (self.stack[src] * inv) * weight;
                    }
                    self.stack[dest] = acc;
                    self.cur = acc;
                }
                0x0B => {
                    let s = if flags & 1 != 0 { model.inv_pos_scale } else { model.pos_scale };
                    self.cur *= Mat4::from_scale(Vec3::splat(s));
                }
                _ => {}
            }
            i += len;
        }
    }

    fn batch(&mut self, material: usize) -> usize {
        if material >= self.slot.len() {
            self.slot.resize(material + 1, usize::MAX);
        }
        if self.slot[material] == usize::MAX {
            self.slot[material] = self.batches.len();
            self.batches.push(GeoBatch { material, vertices: Vec::new() });
        }
        self.slot[material]
    }

    fn draw_shape(&mut self, shape: usize) {
        let Some(shape) = self.model.shapes.get(shape) else { return };
        let mat = self.model.materials.get(self.material);
        let tex_size = mat
            .map(|m| (m.tex_width.max(1) as f32, m.tex_height.max(1) as f32))
            .unwrap_or((1.0, 1.0));

        let mut color = [1.0f32; 3];
        if let Some(m) = mat {
            if m.diffuse_is_vertex_color {
                color = m.diffuse;
            }
        }
        let mut normal = Vec3::ZERO;
        let mut lit = false;
        let mut uv = [0.0f32; 2];
        let mut last = Vec3::ZERO;
        let mut prim = gx::PRIM_TRIANGLES;
        let mut run: Vec<GeoVertex> = Vec::new();
        let mut out: Vec<GeoVertex> = Vec::new();

        // Matrix state is local to this shape: display lists restore the
        // matrices they need from the stack.
        let mut cur = self.cur;
        let mut stack = self.stack;
        let mut sp = 0usize;

        let emit = |cur: &Mat4, last: Vec3, normal: Vec3, lit: bool, color: [f32; 3], uv: [f32; 2], run: &mut Vec<GeoVertex>| {
            let p = cur.transform_point3(last);
            let n = if lit { cur.transform_vector3(normal).normalize_or_zero() } else { Vec3::ZERO };
            run.push(GeoVertex { pos: p.to_array(), normal: n.to_array(), color, uv });
        };

        for_each_command(&shape.display_list, |cmd, p| match cmd {
            0x11 => {
                stack[sp & 31] = cur;
                sp += 1;
            }
            0x12 => {
                sp = sp.saturating_sub(sext(p[0], 6).max(0) as usize);
                cur = stack[sp & 31];
            }
            0x13 => stack[(p[0] & 31) as usize] = cur,
            0x14 => cur = stack[(p[0] & 31) as usize],
            0x15 => cur = Mat4::IDENTITY,
            0x16 => cur = mat4_from_words(p),
            0x17 => cur = mat4x3_from_words(p),
            0x18 => cur *= mat4_from_words(p),
            0x19 => cur *= mat4x3_from_words(p),
            0x1A => {
                let f = |i: usize| p[i] as i32 as f32 / 4096.0;
                cur *= Mat4::from_cols_array(&[
                    f(0), f(1), f(2), 0.0, f(3), f(4), f(5), 0.0, f(6), f(7), f(8), 0.0, 0.0, 0.0, 0.0, 1.0,
                ]);
            }
            0x1B => cur *= Mat4::from_scale(fx_vec3(p)),
            0x1C => cur *= Mat4::from_translation(fx_vec3(p)),
            0x20 => {
                color = rgb555_f32(p[0] as u16);
                lit = false;
            }
            0x21 => {
                normal = Vec3::new(
                    sext(p[0], 10) as f32 / 512.0,
                    sext(p[0] >> 10, 10) as f32 / 512.0,
                    sext(p[0] >> 20, 10) as f32 / 512.0,
                );
                lit = true;
            }
            0x22 => {
                uv = [
                    (p[0] as u16 as i16) as f32 / 16.0 / tex_size.0,
                    ((p[0] >> 16) as u16 as i16) as f32 / 16.0 / tex_size.1,
                ];
            }
            0x23 => {
                last = Vec3::new(
                    (p[0] as u16 as i16) as f32 / 4096.0,
                    ((p[0] >> 16) as u16 as i16) as f32 / 4096.0,
                    (p[1] as u16 as i16) as f32 / 4096.0,
                );
                emit(&cur, last, normal, lit, color, uv, &mut run);
            }
            0x24 => {
                last = Vec3::new(
                    sext(p[0], 10) as f32 / 64.0,
                    sext(p[0] >> 10, 10) as f32 / 64.0,
                    sext(p[0] >> 20, 10) as f32 / 64.0,
                );
                emit(&cur, last, normal, lit, color, uv, &mut run);
            }
            0x25 => {
                last.x = (p[0] as u16 as i16) as f32 / 4096.0;
                last.y = ((p[0] >> 16) as u16 as i16) as f32 / 4096.0;
                emit(&cur, last, normal, lit, color, uv, &mut run);
            }
            0x26 => {
                last.x = (p[0] as u16 as i16) as f32 / 4096.0;
                last.z = ((p[0] >> 16) as u16 as i16) as f32 / 4096.0;
                emit(&cur, last, normal, lit, color, uv, &mut run);
            }
            0x27 => {
                last.y = (p[0] as u16 as i16) as f32 / 4096.0;
                last.z = ((p[0] >> 16) as u16 as i16) as f32 / 4096.0;
                emit(&cur, last, normal, lit, color, uv, &mut run);
            }
            0x28 => {
                last += Vec3::new(
                    sext(p[0], 10) as f32 / 4096.0,
                    sext(p[0] >> 10, 10) as f32 / 4096.0,
                    sext(p[0] >> 20, 10) as f32 / 4096.0,
                );
                emit(&cur, last, normal, lit, color, uv, &mut run);
            }
            0x40 => {
                flush(prim, &mut run, &mut out);
                prim = p[0] & 3;
            }
            0x41 => flush(prim, &mut run, &mut out),
            _ => {}
        });
        flush(prim, &mut run, &mut out);

        if !out.is_empty() {
            let b = self.batch(self.material);
            self.batches[b].vertices.extend(out);
        }
    }
}

fn fx_vec3(p: &[u32]) -> Vec3 {
    Vec3::new(p[0] as i32 as f32 / 4096.0, p[1] as i32 as f32 / 4096.0, p[2] as i32 as f32 / 4096.0)
}

fn mat4_from_words(p: &[u32]) -> Mat4 {
    let mut m = [0f32; 16];
    for (d, s) in m.iter_mut().zip(p) {
        *d = *s as i32 as f32 / 4096.0;
    }
    Mat4::from_cols_array(&m)
}

fn mat4x3_from_words(p: &[u32]) -> Mat4 {
    let mut m = [0f32; 12];
    for (d, s) in m.iter_mut().zip(p) {
        *d = *s as i32 as f32 / 4096.0;
    }
    mat4_from_4x3(&m)
}

/// Turns one primitive run into triangles, keeping the winding of the source.
fn flush(prim: u32, run: &mut Vec<GeoVertex>, out: &mut Vec<GeoVertex>) {
    let v = &run[..];
    match prim {
        gx::PRIM_TRIANGLES => {
            for t in v.chunks_exact(3) {
                out.extend_from_slice(t);
            }
        }
        gx::PRIM_QUADS => {
            for q in v.chunks_exact(4) {
                out.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
            }
        }
        gx::PRIM_TRIANGLE_STRIP => {
            for i in 2..v.len() {
                if i % 2 == 0 {
                    out.extend_from_slice(&[v[i - 2], v[i - 1], v[i]]);
                } else {
                    out.extend_from_slice(&[v[i - 1], v[i - 2], v[i]]);
                }
            }
        }
        _ => {
            // Quad strip: vertices come in pairs across the strip.
            let mut i = 0;
            while i + 3 < v.len() {
                out.extend_from_slice(&[v[i], v[i + 1], v[i + 3], v[i], v[i + 3], v[i + 2]]);
                i += 2;
            }
        }
    }
    run.clear();
}
