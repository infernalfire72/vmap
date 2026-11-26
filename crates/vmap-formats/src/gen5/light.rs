//! Area light files: one lighting setup per part of the day.

use glam::Vec3;

use crate::error::Result;
use crate::nitro::tex::rgb555_f32;
use crate::reader::Reader;

#[derive(Debug, Clone, Copy, Default)]
pub struct LightEntry {
    /// 0 morning, 1 day, 2 evening, 3 night, 4 late night.
    pub day_part: u16,
    pub minutes_shift: i16,
    pub enabled: [bool; 4],
    pub colors: [[f32; 3]; 4],
    pub directions: [Vec3; 4],
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
    pub specular: [f32; 3],
    pub emission: [f32; 3],
    pub fog_color: [f32; 3],
    /// Background (sky) colour.
    pub clear_color: [f32; 3],
}

impl LightEntry {
    pub const SIZE: usize = 0x34;

    pub fn parse_file(data: &[u8]) -> Result<Vec<Self>> {
        let mut r = Reader::new(data);
        let mut out = Vec::with_capacity(data.len() / Self::SIZE);
        while r.remaining() >= Self::SIZE {
            let mut e = Self { day_part: r.u16()?, minutes_shift: r.i16()?, ..Default::default() };
            for v in &mut e.enabled {
                *v = r.u8()? != 0;
            }
            for c in &mut e.colors {
                *c = rgb555_f32(r.u16()?);
            }
            for d in &mut e.directions {
                *d = Vec3::new(r.fx16()?, r.fx16()?, r.fx16()?);
            }
            e.diffuse = rgb555_f32(r.u16()?);
            e.ambient = rgb555_f32(r.u16()?);
            e.specular = rgb555_f32(r.u16()?);
            e.emission = rgb555_f32(r.u16()?);
            e.fog_color = rgb555_f32(r.u16()?);
            e.clear_color = rgb555_f32(r.u16()?);
            out.push(e);
        }
        Ok(out)
    }
}
