//! Area headers: which texture set, building bundle, animations and lights
//! a group of zones shares.

use crate::error::Result;
use crate::reader::Reader;

#[derive(Debug, Clone, Copy, Default)]
pub struct AreaHeader {
    pub buildings_id: u16,
    pub textures_id: u16,
    /// Texture scroll animation, or 255 for none.
    pub srt_anim: u8,
    /// Texture pattern animation, or 255 for none.
    pub pattern_anim: u8,
    pub is_exterior: bool,
    pub light_index: u8,
    pub outline_type: u8,
}

impl AreaHeader {
    pub const SIZE: usize = 10;

    pub fn parse_table(data: &[u8]) -> Result<Vec<Self>> {
        let mut r = Reader::new(data);
        let mut out = Vec::with_capacity(data.len() / Self::SIZE);
        while r.remaining() >= Self::SIZE {
            let buildings_id = r.u16()?;
            let textures_id = r.u16()?;
            let srt_anim = r.u8()?;
            let pattern_anim = r.u8()?;
            let is_exterior = r.u8()? != 0;
            let light_index = r.u8()?;
            let outline_type = r.u8()?;
            let _unknown = r.u8()?;
            out.push(Self {
                buildings_id,
                textures_id,
                srt_anim,
                pattern_anim,
                is_exterior,
                light_index,
                outline_type,
            });
        }
        Ok(out)
    }
}
