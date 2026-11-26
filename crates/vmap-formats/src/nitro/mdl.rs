//! `MDL0` model blocks: nodes (joints), materials, shapes (display lists) and
//! the render command list that ties them together.

use glam::{Mat3, Mat4, Vec2, Vec3};

use crate::error::Result;
use crate::nitro::tex::{rgb555_f32, Tex0};
use crate::nitro::{read_dict, NitroFile};
use crate::reader::Reader;

/// A parsed `BMD0` file: its models and the textures embedded with them.
#[derive(Debug, Clone, Default)]
pub struct Mdl0 {
    pub models: Vec<Model>,
    pub textures: Option<Tex0>,
}

impl Mdl0 {
    pub fn from_bmd(data: &[u8]) -> Result<Self> {
        let file = NitroFile::parse(data)?;
        let Some(blk) = file.block(b"MDL0") else {
            return crate::error::invalid("file has no MDL0 block");
        };
        let mut models = Vec::new();
        for e in read_dict(blk, 8)? {
            models.push(Model::parse(blk, e.u32(0) as usize, e.name)?);
        }
        let textures = match file.block(b"TEX0") {
            Some(b) => Tex0::parse(b).ok(),
            None => None,
        };
        Ok(Self { models, textures })
    }
}

#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub pos_scale: f32,
    pub inv_pos_scale: f32,
    pub vertex_count: u16,
    pub polygon_count: u16,
    pub triangle_count: u16,
    pub quad_count: u16,
    pub bbox_min: Vec3,
    pub bbox_size: Vec3,
    pub nodes: Vec<Node>,
    pub materials: Vec<Material>,
    pub shapes: Vec<Shape>,
    /// Render command bytecode.
    pub sbc: Vec<u8>,
    /// Inverse bind matrix per node; only present for weighted models.
    pub inv_binds: Vec<Mat4>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub translation: Vec3,
    pub rotation: Mat3,
    pub scale: Vec3,
}

impl Node {
    pub fn local_matrix(&self) -> Mat4 {
        compose(self.translation, self.rotation, self.scale)
    }
}

pub fn compose(t: Vec3, r: Mat3, s: Vec3) -> Mat4 {
    Mat4::from_translation(t) * Mat4::from_mat3(r) * Mat4::from_scale(s)
}

#[derive(Debug, Clone)]
pub struct Material {
    pub name: String,
    pub texture: Option<String>,
    pub palette: Option<String>,
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
    pub specular: [f32; 3],
    pub emission: [f32; 3],
    /// The diffuse colour doubles as the vertex colour.
    pub diffuse_is_vertex_color: bool,
    pub poly_attr: u32,
    pub tex_image_param: u32,
    pub flags: u16,
    pub tex_width: u16,
    pub tex_height: u16,
    pub tex_scale: Vec2,
    pub tex_rot_sin: f32,
    pub tex_rot_cos: f32,
    pub tex_translation: Vec2,
}

impl Material {
    /// Bit mask of the enabled hardware lights (bit 0 = light 0).
    pub fn lights(&self) -> u32 {
        self.poly_attr & 0xF
    }
    /// 0 modulate, 1 decal, 2 toon/highlight, 3 shadow.
    pub fn polygon_mode(&self) -> u32 {
        (self.poly_attr >> 4) & 3
    }
    pub fn draw_back(&self) -> bool {
        (self.poly_attr >> 6) & 1 == 1
    }
    pub fn draw_front(&self) -> bool {
        (self.poly_attr >> 7) & 1 == 1
    }
    /// Polygon alpha, 0..=31. 31 is opaque, 0 means wireframe.
    pub fn alpha(&self) -> u32 {
        (self.poly_attr >> 16) & 31
    }
    pub fn polygon_id(&self) -> u32 {
        (self.poly_attr >> 24) & 63
    }
    pub fn repeat_s(&self) -> bool {
        (self.tex_image_param >> 16) & 1 == 1
    }
    pub fn repeat_t(&self) -> bool {
        (self.tex_image_param >> 17) & 1 == 1
    }
    pub fn flip_s(&self) -> bool {
        (self.tex_image_param >> 18) & 1 == 1
    }
    pub fn flip_t(&self) -> bool {
        (self.tex_image_param >> 19) & 1 == 1
    }
    /// 0 none, 1 texture coordinates, 2 normals, 3 vertex positions.
    pub fn texgen_mode(&self) -> u32 {
        self.tex_image_param >> 30
    }
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub name: String,
    pub has_normals: bool,
    pub has_colors: bool,
    pub has_uvs: bool,
    /// Packed geometry commands.
    pub display_list: Vec<u8>,
}

impl Model {
    fn parse(blk: &[u8], mo: usize, name: String) -> Result<Self> {
        let mut r = Reader::at(blk, mo);
        let _size = r.u32()?;
        let ofs_sbc = r.u32()? as usize;
        let ofs_mat = r.u32()? as usize;
        let ofs_shp = r.u32()? as usize;
        let ofs_evp = r.u32()? as usize;

        let _sbc_type = r.u8()?;
        let _scaling_rule = r.u8()?;
        let _tex_mtx_mode = r.u8()?;
        let node_count = r.u8()? as usize;
        let _mat_count = r.u8()?;
        let _shp_count = r.u8()?;
        let _first_unused_mtx = r.u8()?;
        r.skip(1);
        let pos_scale = r.fx32()?;
        let inv_pos_scale = r.fx32()?;
        let vertex_count = r.u16()?;
        let polygon_count = r.u16()?;
        let triangle_count = r.u16()?;
        let quad_count = r.u16()?;
        let bmin = Vec3::new(r.fx16()?, r.fx16()?, r.fx16()?);
        let bsize = Vec3::new(r.fx16()?, r.fx16()?, r.fx16()?);
        let box_scale = r.fx32()?;
        let _box_inv_scale = r.fx32()?;

        // Nodes.
        let nb = r.pos();
        let mut nodes = Vec::new();
        for e in read_dict(blk, nb)? {
            nodes.push(Node::parse(blk, nb + e.u32(0) as usize, e.name)?);
        }

        // Materials, then the tables that bind texture and palette names.
        let mb = mo + ofs_mat;
        let mut mr = Reader::at(blk, mb);
        let tex_map = mr.u16()? as usize;
        let pal_map = mr.u16()? as usize;
        let mut materials = Vec::new();
        for e in read_dict(blk, mb + 4)? {
            materials.push(Material::parse(blk, mb + e.u32(0) as usize, e.name)?);
        }
        for (table, is_texture) in [(tex_map, true), (pal_map, false)] {
            for e in read_dict(blk, mb + table)? {
                let v = e.u32(0);
                let offset = (v & 0xFFFF) as usize;
                let count = ((v >> 16) & 0xFF) as usize;
                let mut ir = Reader::at(blk, mb + offset);
                for _ in 0..count {
                    let idx = ir.u8()? as usize;
                    if let Some(m) = materials.get_mut(idx) {
                        if is_texture {
                            m.texture = Some(e.name.clone());
                        } else {
                            m.palette = Some(e.name.clone());
                        }
                    }
                }
            }
        }

        // Shapes.
        let sb = mo + ofs_shp;
        let mut shapes = Vec::new();
        for e in read_dict(blk, sb)? {
            let so = sb + e.u32(0) as usize;
            let mut sr = Reader::at(blk, so);
            let _tag = sr.u16()?;
            let _size = sr.u16()?;
            let flags = sr.u32()?;
            let dl_offset = sr.u32()? as usize;
            let dl_size = sr.u32()? as usize;
            let display_list = Reader::at(blk, so + dl_offset).bytes(dl_size)?.to_vec();
            shapes.push(Shape {
                name: e.name,
                has_normals: flags & 1 != 0,
                has_colors: flags & 2 != 0,
                has_uvs: flags & 4 != 0,
                display_list,
            });
        }

        let sbc_start = mo + ofs_sbc;
        let sbc_end = (mo + ofs_mat).max(sbc_start).min(blk.len());
        let sbc = blk.get(sbc_start..sbc_end).unwrap_or(&[]).to_vec();

        // Inverse bind matrices are only stored when a weighted-matrix
        // command needs them.
        let mut inv_binds = Vec::new();
        if sbc_uses_nodemix(&sbc) {
            let mut er = Reader::at(blk, mo + ofs_evp);
            for _ in 0..node_count {
                let mut m = [0f32; 12];
                for v in &mut m {
                    *v = er.fx32()?;
                }
                er.skip(9 * 4);
                inv_binds.push(mat4_from_4x3(&m));
            }
        }

        Ok(Self {
            name,
            pos_scale,
            inv_pos_scale,
            vertex_count,
            polygon_count,
            triangle_count,
            quad_count,
            bbox_min: bmin * box_scale,
            bbox_size: bsize * box_scale,
            nodes,
            materials,
            shapes,
            sbc,
            inv_binds,
        })
    }
}

/// DS 4x3 matrices store three basis rows followed by a translation row.
pub fn mat4_from_4x3(m: &[f32; 12]) -> Mat4 {
    Mat4::from_cols_array(&[
        m[0], m[1], m[2], 0.0, m[3], m[4], m[5], 0.0, m[6], m[7], m[8], 0.0, m[9], m[10], m[11], 1.0,
    ])
}

fn sbc_uses_nodemix(sbc: &[u8]) -> bool {
    let mut i = 0;
    while i < sbc.len() {
        let op = sbc[i] & 0x1F;
        let flags = sbc[i] >> 5;
        if op == 0x09 {
            return true;
        }
        if op == 0x01 {
            return false;
        }
        match sbc_len(op, flags, sbc.get(i + 2).copied().unwrap_or(0)) {
            Some(n) => i += n,
            None => return false,
        }
    }
    false
}

/// Total byte length of a render command, including its opcode byte.
pub(crate) fn sbc_len(op: u8, flags: u8, nodemix_count: u8) -> Option<usize> {
    Some(match op {
        0x00 | 0x01 | 0x0B => 1,
        0x02 => 3,
        0x03 | 0x04 | 0x05 => 2,
        0x06 => 4 + (flags & 1) as usize + ((flags >> 1) & 1) as usize,
        0x07 | 0x08 => 2 + (flags & 1) as usize + ((flags >> 1) & 1) as usize,
        0x09 => 3 + 3 * nodemix_count as usize,
        0x0A => 9,
        0x0C | 0x0D => 3,
        _ => return None,
    })
}

impl Node {
    fn parse(blk: &[u8], pos: usize, name: String) -> Result<Self> {
        let mut r = Reader::at(blk, pos);
        let flags = r.u16()?;
        let m00 = r.fx16()?;
        let translation = if flags & 0x1 == 0 { r.vec3_fx32()? } else { Vec3::ZERO };
        let rotation = if flags & 0x2 != 0 {
            Mat3::IDENTITY
        } else if flags & 0x8 != 0 {
            let a = r.fx16()?;
            let b = r.fx16()?;
            pivot_rotation(flags, a, b)
        } else {
            let mut m = [m00, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
            for v in m.iter_mut().skip(1) {
                *v = r.fx16()?;
            }
            Mat3::from_cols_array(&m)
        };
        let scale = if flags & 0x4 == 0 {
            let s = r.vec3_fx32()?;
            let _inverse = r.vec3_fx32()?;
            s
        } else {
            Vec3::ONE
        };
        Ok(Self { name, translation, rotation, scale })
    }
}

/// Decodes the compact rotation used for axis-aligned joints: one element is
/// +-1, its row and column are zero, and the remaining 2x2 holds A, B, C, D.
pub fn pivot_rotation(flags: u16, a: f32, b: f32) -> Mat3 {
    let pivot = ((flags >> 4) & 0xF) as usize;
    let one = if flags & 0x100 != 0 { -1.0 } else { 1.0 };
    let c = if flags & 0x200 != 0 { -b } else { b };
    let d = if flags & 0x400 != 0 { -a } else { a };
    let (one_col, one_row) = (pivot / 3, pivot % 3);
    let lut = [a, b, c, d];
    let mut m = [0f32; 9];
    let mut next = 0;
    for col in 0..3 {
        for row in 0..3 {
            if row != one_row && col != one_col {
                m[col * 3 + row] = lut[next.min(3)];
                next += 1;
            }
        }
    }
    if one_col < 3 {
        m[one_col * 3 + one_row] = one;
    }
    Mat3::from_cols_array(&m)
}

impl Material {
    fn parse(blk: &[u8], pos: usize, name: String) -> Result<Self> {
        let mut r = Reader::at(blk, pos);
        let _tag = r.u16()?;
        let _size = r.u16()?;
        let dif_amb = r.u32()?;
        let spe_emi = r.u32()?;
        let poly_attr = r.u32()?;
        let _poly_attr_mask = r.u32()?;
        let tex_image_param = r.u32()?;
        let _tex_image_param_mask = r.u32()?;
        let _palette_base = r.u16()?;
        let flags = r.u16()?;
        let tex_width = r.u16()?;
        let tex_height = r.u16()?;
        let _mag_w = r.fx32()?;
        let _mag_h = r.fx32()?;

        let mut tex_scale = Vec2::ONE;
        let (mut sin, mut cos) = (0.0, 1.0);
        let mut tex_translation = Vec2::ZERO;
        if flags & 0x2 == 0 {
            tex_scale = Vec2::new(r.fx32()?, r.fx32()?);
        }
        if flags & 0x4 == 0 {
            sin = r.fx32()?;
            cos = r.fx32()?;
        }
        if flags & 0x8 == 0 {
            tex_translation = Vec2::new(r.fx32()?, r.fx32()?);
        }

        Ok(Self {
            name,
            texture: None,
            palette: None,
            diffuse: rgb555_f32(dif_amb as u16),
            ambient: rgb555_f32((dif_amb >> 16) as u16),
            specular: rgb555_f32(spe_emi as u16),
            emission: rgb555_f32((spe_emi >> 16) as u16),
            diffuse_is_vertex_color: (dif_amb >> 15) & 1 == 1 || flags & 0x100 != 0,
            poly_attr,
            tex_image_param,
            flags,
            tex_width,
            tex_height,
            tex_scale,
            tex_rot_sin: sin,
            tex_rot_cos: cos,
            tex_translation,
        })
    }
}
