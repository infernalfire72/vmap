//! Nitro animation resources: texture placement (`BTA0`), texture swap
//! (`BTP0`) and joint (`BCA0`) animations.

use glam::{Mat3, Mat4, Vec3};

use super::mdl::{compose, pivot_rotation, Model};
use super::{read_dict, NitroFile};
use crate::error::{FormatError, Result};
use crate::reader::Reader;

/// Values sampled every `step` frames.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub step: u32,
    pub values: Vec<f32>,
}

impl Track {
    pub fn constant(v: f32) -> Self {
        Self { step: 1, values: vec![v] }
    }

    pub fn is_constant(&self) -> bool {
        self.values.len() <= 1
    }

    /// Value at a frame. Tracks stored for every frame are not interpolated
    /// (they may wrap around from one frame to the next, as scrolling
    /// textures do); sparser tracks are, like the console does.
    pub fn sample(&self, frame: f32) -> f32 {
        let Some(&last) = self.values.last() else { return 0.0 };
        let pos = (frame / self.step.max(1) as f32).max(0.0);
        let i = pos.floor() as usize;
        match (self.values.get(i), self.values.get(i + 1)) {
            (Some(a), Some(b)) if self.step > 1 => a + (b - a) * (pos - i as f32),
            (Some(a), _) => *a,
            _ => last,
        }
    }
}

fn step_of(flags: u32) -> u32 {
    1 << (flags >> 30)
}

/// Number of stored samples of a curve.
fn sample_count(frame_count: u16, step: u32) -> usize {
    (frame_count as u32 / step.max(1)).max(1) as usize
}

/// Animated texture placement of one material.
#[derive(Debug, Clone)]
pub struct TexSrtTrack {
    pub material: String,
    pub scale_s: Track,
    pub scale_t: Track,
    pub rot_sin: Track,
    pub rot_cos: Track,
    pub trans_s: Track,
    pub trans_t: Track,
}

#[derive(Debug, Clone)]
pub struct TexSrtAnimation {
    pub name: String,
    pub frame_count: u16,
    pub tracks: Vec<TexSrtTrack>,
}

/// Reads every animation of a `BTA0` file.
pub fn parse_tex_srt(data: &[u8]) -> Result<Vec<TexSrtAnimation>> {
    let file = NitroFile::parse(data)?;
    let blk = file.block(b"SRT0").ok_or_else(|| FormatError::Invalid("the file has no SRT0 block".into()))?;
    let mut out = Vec::new();
    for e in read_dict(blk, 8)? {
        let base = e.u32(0) as usize;
        let mut r = Reader::at(blk, base);
        r.skip(4);
        let frame_count = r.u16()?;

        // A scale or translation channel: fixed-point values, 16 or 32 bit.
        let vector = |flags: u32, param: u32| -> Result<Track> {
            let (constant, short) = (flags & (1 << 29) != 0, flags & (1 << 28) != 0);
            if constant {
                let v = if short { param as u16 as i16 as i32 } else { param as i32 };
                return Ok(Track::constant(v as f32 / 4096.0));
            }
            let step = step_of(flags);
            let mut r = Reader::at(blk, base + param as usize);
            let mut values = Vec::new();
            for _ in 0..sample_count(frame_count, step) {
                values.push(if short { r.fx16()? } else { r.fx32()? });
            }
            Ok(Track { step, values })
        };
        // The rotation channel: pairs of sine and cosine.
        let rotation = |flags: u32, param: u32| -> Result<(Track, Track)> {
            let fx = |v: u32| (v as u16 as i16) as f32 / 4096.0;
            if flags & (1 << 29) != 0 {
                return Ok((Track::constant(fx(param)), Track::constant(fx(param >> 16))));
            }
            let step = step_of(flags);
            let mut r = Reader::at(blk, base + param as usize);
            let (mut sin, mut cos) = (Vec::new(), Vec::new());
            for _ in 0..sample_count(frame_count, step) {
                sin.push(r.fx16()?);
                cos.push(r.fx16()?);
            }
            Ok((Track { step, values: sin }, Track { step, values: cos }))
        };

        let mut tracks = Vec::new();
        for t in read_dict(blk, base + 8)? {
            let (rot_sin, rot_cos) = rotation(t.u32(4), t.u32(5))?;
            tracks.push(TexSrtTrack {
                scale_s: vector(t.u32(0), t.u32(1))?,
                scale_t: vector(t.u32(2), t.u32(3))?,
                rot_sin,
                rot_cos,
                trans_s: vector(t.u32(6), t.u32(7))?,
                trans_t: vector(t.u32(8), t.u32(9))?,
                material: t.name,
            });
        }
        out.push(TexSrtAnimation { name: e.name, frame_count, tracks });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TexPatternKey {
    /// Frame from which this texture is shown.
    pub frame: u16,
    pub texture: String,
    pub palette: String,
}

#[derive(Debug, Clone)]
pub struct TexPatternTrack {
    pub material: String,
    pub keys: Vec<TexPatternKey>,
}

#[derive(Debug, Clone)]
pub struct TexPatternAnimation {
    pub name: String,
    pub frame_count: u16,
    pub tracks: Vec<TexPatternTrack>,
}

/// Reads every animation of a `BTP0` file.
pub fn parse_tex_pattern(data: &[u8]) -> Result<Vec<TexPatternAnimation>> {
    let file = NitroFile::parse(data)?;
    let blk = file.block(b"PAT0").ok_or_else(|| FormatError::Invalid("the file has no PAT0 block".into()))?;
    let mut out = Vec::new();
    for e in read_dict(blk, 8)? {
        let base = e.u32(0) as usize;
        let mut r = Reader::at(blk, base);
        r.skip(4);
        let frame_count = r.u16()?;
        let tex_count = r.u8()? as usize;
        let pal_count = r.u8()? as usize;
        let tex_offset = r.u16()? as usize;
        let pal_offset = r.u16()? as usize;

        let names = |offset: usize, count: usize| -> Result<Vec<String>> {
            let mut r = Reader::at(blk, base + offset);
            (0..count).map(|_| r.str_padded(16)).collect()
        };
        let textures = names(tex_offset, tex_count)?;
        let palettes = names(pal_offset, pal_count)?;

        let mut tracks = Vec::new();
        for t in read_dict(blk, base + 12)? {
            let key_count = (t.u32(0) & 0xFFFF) as usize;
            let offset = (t.u32(1) >> 16) as usize;
            let mut r = Reader::at(blk, base + offset);
            let mut keys = Vec::with_capacity(key_count);
            for _ in 0..key_count {
                let frame = r.u16()?;
                let texture = textures.get(r.u8()? as usize).cloned().unwrap_or_default();
                let palette = palettes.get(r.u8()? as usize).cloned().unwrap_or_default();
                keys.push(TexPatternKey { frame, texture, palette });
            }
            tracks.push(TexPatternTrack { material: t.name, keys });
        }
        out.push(TexPatternAnimation { name: e.name, frame_count, tracks });
    }
    Ok(out)
}

/// One component of a joint's transform over time.
#[derive(Debug, Clone)]
pub enum Channel<T> {
    /// Keep the value the model itself gives the joint.
    Bind,
    /// No translation, no rotation or unit scale.
    Identity,
    Animated(T),
}

#[derive(Debug, Clone)]
pub struct RotationTrack {
    pub step: u32,
    pub values: Vec<Mat3>,
}

impl RotationTrack {
    pub fn sample(&self, frame: f32) -> Mat3 {
        let pos = (frame / self.step.max(1) as f32).max(0.0);
        let i = pos.floor() as usize;
        match (self.values.get(i), self.values.get(i + 1)) {
            (Some(a), Some(b)) if self.step > 1 => {
                let t = pos - i as f32;
                Mat3::from_cols(a.x_axis.lerp(b.x_axis, t), a.y_axis.lerp(b.y_axis, t), a.z_axis.lerp(b.z_axis, t))
            }
            (Some(a), _) => *a,
            _ => self.values.last().copied().unwrap_or(Mat3::IDENTITY),
        }
    }
}

#[derive(Debug, Clone)]
pub struct JointTrack {
    /// Index of the animated joint in the model.
    pub node: usize,
    pub translation: Channel<[Track; 3]>,
    pub rotation: Channel<RotationTrack>,
    pub scale: Channel<[Track; 3]>,
}

#[derive(Debug, Clone)]
pub struct JointAnimation {
    pub name: String,
    pub frame_count: u16,
    pub joints: Vec<JointTrack>,
}

impl JointAnimation {
    /// Local matrix of every joint of `model` at a frame, ready for
    /// [`super::evaluate`].
    pub fn pose(&self, model: &Model, frame: f32) -> Vec<Mat4> {
        let mut pose: Vec<Mat4> = model.nodes.iter().map(|n| n.local_matrix()).collect();
        let vec3 = |t: &[Track; 3]| Vec3::new(t[0].sample(frame), t[1].sample(frame), t[2].sample(frame));
        for j in &self.joints {
            let Some(node) = model.nodes.get(j.node) else { continue };
            let t = match &j.translation {
                Channel::Bind => node.translation,
                Channel::Identity => Vec3::ZERO,
                Channel::Animated(t) => vec3(t),
            };
            let r = match &j.rotation {
                Channel::Bind => node.rotation,
                Channel::Identity => Mat3::IDENTITY,
                Channel::Animated(r) => r.sample(frame),
            };
            let s = match &j.scale {
                Channel::Bind => node.scale,
                Channel::Identity => Vec3::ONE,
                Channel::Animated(s) => vec3(s),
            };
            pose[j.node] = compose(t, r, s);
        }
        pose
    }
}

/// Reads every animation of a `BCA0` file.
pub fn parse_joint(data: &[u8]) -> Result<Vec<JointAnimation>> {
    let file = NitroFile::parse(data)?;
    let blk = file.block(b"JNT0").ok_or_else(|| FormatError::Invalid("the file has no JNT0 block".into()))?;
    let mut out = Vec::new();
    for e in read_dict(blk, 8)? {
        let base = e.u32(0) as usize;
        let mut r = Reader::at(blk, base);
        r.skip(4);
        let frame_count = r.u16()?;
        let node_count = r.u16()? as usize;
        let _flags = r.u32()?;
        let rot3 = base + r.u32()? as usize;
        let rot5 = base + r.u32()? as usize;
        let offsets: Vec<usize> = (0..node_count).map(|_| r.u16().map(|o| o as usize)).collect::<Result<_>>()?;

        let mut joints = Vec::with_capacity(node_count);
        for off in offsets {
            let mut r = Reader::at(blk, base + off);
            let header = r.u32()?;
            let flag = |bit: u32| header & (1 << bit) != 0;
            let mut joint = JointTrack {
                node: (header >> 24) as usize,
                translation: Channel::Identity,
                rotation: Channel::Identity,
                scale: Channel::Identity,
            };
            // Bit 0: the whole transform is the identity.
            if !flag(0) {
                joint.translation = if flag(2) {
                    Channel::Bind
                } else if flag(1) {
                    Channel::Identity
                } else {
                    let mut axis = |constant: bool| -> Result<Track> {
                        if constant {
                            return Ok(Track::constant(r.fx32()?));
                        }
                        let (flags, offset) = (r.u32()?, r.u32()? as usize);
                        scalar_curve(blk, base + offset, flags, frame_count, false)
                    };
                    Channel::Animated([axis(flag(3))?, axis(flag(4))?, axis(flag(5))?])
                };
                joint.rotation = if flag(7) {
                    Channel::Bind
                } else if flag(6) {
                    Channel::Identity
                } else if flag(8) {
                    let index = r.u32()?;
                    Channel::Animated(RotationTrack { step: 1, values: vec![rotation(blk, rot3, rot5, index)?] })
                } else {
                    let (flags, offset) = (r.u32()?, r.u32()? as usize);
                    let step = step_of(flags);
                    let mut idx = Reader::at(blk, base + offset);
                    let mut values = Vec::new();
                    for _ in 0..sample_count(frame_count, step) {
                        values.push(rotation(blk, rot3, rot5, idx.u16()? as u32)?);
                    }
                    Channel::Animated(RotationTrack { step, values })
                };
                joint.scale = if flag(10) {
                    Channel::Bind
                } else if flag(9) {
                    Channel::Identity
                } else {
                    let mut axis = |constant: bool| -> Result<Track> {
                        if constant {
                            let scale = r.fx32()?;
                            let _inverse = r.fx32()?;
                            return Ok(Track::constant(scale));
                        }
                        let (flags, offset) = (r.u32()?, r.u32()? as usize);
                        scalar_curve(blk, base + offset, flags, frame_count, true)
                    };
                    Channel::Animated([axis(flag(11))?, axis(flag(12))?, axis(flag(13))?])
                };
            }
            joints.push(joint);
        }
        out.push(JointAnimation { name: e.name, frame_count, joints });
    }
    Ok(out)
}

/// A translation or scale curve of a joint. Scale curves store each value
/// together with its inverse.
fn scalar_curve(blk: &[u8], pos: usize, flags: u32, frame_count: u16, paired: bool) -> Result<Track> {
    let short = flags & (1 << 29) != 0;
    let step = step_of(flags);
    let mut r = Reader::at(blk, pos);
    let mut values = Vec::new();
    for _ in 0..sample_count(frame_count, step) {
        values.push(if short { r.fx16()? } else { r.fx32()? });
        if paired {
            r.skip(if short { 2 } else { 4 });
        }
    }
    Ok(Track { step, values })
}

/// One rotation of a joint animation's shared rotation tables.
fn rotation(blk: &[u8], rot3: usize, rot5: usize, index: u32) -> Result<Mat3> {
    let i = (index & 0x7FFF) as usize;
    if index & 0x8000 != 0 {
        // Axis-aligned: the same compact form joints use in models.
        let mut r = Reader::at(blk, rot3 + i * 6);
        let flags = r.u16()?;
        let (a, b) = (r.fx16()?, r.fx16()?);
        return Ok(pivot_rotation(flags << 4, a, b));
    }
    // Two columns in five values: 13 bits each, with the spare low bits
    // holding the sixth element. The third column is their cross product.
    let mut r = Reader::at(blk, rot5 + i * 10);
    let v = [r.i16()? as i32, r.i16()? as i32, r.i16()? as i32, r.i16()? as i32, r.i16()? as i32];
    let fx = |x: i32| x as f32 / 4096.0;
    let last = ((v[4] << 31) >> 19) | ((v[0] & 7) << 9) | ((v[1] & 7) << 6) | ((v[2] & 7) << 3) | (v[3] & 7);
    let x = Vec3::new(fx(v[0] >> 3), fx(v[1] >> 3), fx(v[2] >> 3));
    let y = Vec3::new(fx(v[3] >> 3), fx(v[4] >> 3), fx(last));
    Ok(Mat3::from_cols(x, y, x.cross(y)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_tracks_step_and_sparse_tracks_interpolate() {
        let dense = Track { step: 1, values: vec![0.0, 0.5, 0.0] };
        assert_eq!(dense.sample(0.9), 0.0);
        assert_eq!(dense.sample(1.0), 0.5);
        assert_eq!(dense.sample(99.0), 0.0);

        let sparse = Track { step: 4, values: vec![0.0, 1.0] };
        assert_eq!(sparse.sample(2.0), 0.5);
        assert_eq!(sparse.sample(4.0), 1.0);
        assert_eq!(sparse.sample(7.0), 1.0);
    }

    #[test]
    fn five_value_rotation_unpacks_the_sixth_element() {
        let enc = |v: i32, low: i32| (((v << 3) | low) as i16).to_le_bytes();
        // x = (0.5, 0, 0) and y = (0, 0.5, z), with z spread over the spare
        // low bits of the first four values.
        let mut blk = Vec::new();
        for b in [enc(2048, 0b001), enc(0, 0b010), enc(0, 0b011), enc(0, 0b100), enc(2048, 0)] {
            blk.extend_from_slice(&b);
        }
        let m = rotation(&blk, 0, 0, 0).unwrap();
        let z = ((1 << 9) | (2 << 6) | (3 << 3) | 4) as f32 / 4096.0;
        assert_eq!(m.x_axis, Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(m.y_axis, Vec3::new(0.0, 0.5, z));
    }
}
