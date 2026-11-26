//! Zone headers: one 0x30-byte record per zone describing which map matrix,
//! area, scripts, text and entities it uses.

use crate::error::Result;
use crate::reader::Reader;

#[derive(Debug, Clone, Default)]
pub struct ZoneHeader {
    pub map_type: u8,
    pub npc_cache_index: u8,
    pub area_id: u16,
    pub matrix_id: u16,
    pub scripts_id: u16,
    pub init_scripts_id: u16,
    pub text_file_id: u16,
    pub bgm_spring: u16,
    pub bgm_summer: u16,
    pub bgm_autumn: u16,
    pub bgm_winter: u16,
    pub encounter_id: i16,
    pub entities_id: u16,
    pub parent_zone_id: u16,
    pub location_name: u16,
    pub location_name_display: u8,
    pub weather: u8,
    pub camera_index: u8,
    pub battle_bg: u8,
    pub map_transition: u8,
    pub can_cycle: bool,
    pub can_run: bool,
    pub can_escape_rope: bool,
    pub can_fly: bool,
    pub matrix_camera_boundary: i16,
    pub name_icon: u16,
    pub fly_pos: [i32; 3],
}

impl ZoneHeader {
    pub const SIZE: usize = 0x30;

    pub fn parse(r: &mut Reader) -> Result<Self> {
        let map_type = r.u8()?;
        let npc_cache_index = r.u8()?;
        let area_id = r.u16()?;
        let matrix_id = r.u16()?;
        let scripts_id = r.u16()?;
        let init_scripts_id = r.u16()?;
        let text_file_id = r.u16()?;
        let bgm_spring = r.u16()?;
        let bgm_summer = r.u16()?;
        let bgm_autumn = r.u16()?;
        let bgm_winter = r.u16()?;
        let enc = r.u16()? & 0x1FFF;
        let encounter_id = ((enc << 3) as i16) >> 3;
        let entities_id = r.u16()?;
        let parent_zone_id = r.u16()?;
        let loc = r.u16()?;
        let env = r.u16()?;
        let flags = r.u16()?;
        let matrix_camera_boundary = r.i16()?;
        let icon = r.u16()?;
        let fly_pos = [r.i32()?, r.i32()?, r.i32()?];
        Ok(Self {
            map_type,
            npc_cache_index,
            area_id,
            matrix_id,
            scripts_id,
            init_scripts_id,
            text_file_id,
            bgm_spring,
            bgm_summer,
            bgm_autumn,
            bgm_winter,
            encounter_id,
            entities_id,
            parent_zone_id,
            location_name: loc & 0x3FF,
            location_name_display: (loc >> 10) as u8 & 0x3F,
            weather: (env & 0x3F) as u8,
            camera_index: ((env >> 9) & 0x7F) as u8,
            map_transition: (flags & 0x1F) as u8,
            battle_bg: ((flags >> 5) & 0x1F) as u8,
            can_cycle: (flags >> 10) & 1 == 1,
            can_run: (flags >> 11) & 1 == 1,
            can_escape_rope: (flags >> 12) & 1 == 1,
            can_fly: (flags >> 13) & 1 == 1,
            matrix_camera_boundary,
            name_icon: icon & 0x1FFF,
            fly_pos,
        })
    }

    /// Parses the whole zone table.
    pub fn parse_table(data: &[u8]) -> Result<Vec<Self>> {
        let mut r = Reader::new(data);
        let mut out = Vec::with_capacity(data.len() / Self::SIZE);
        while r.remaining() >= Self::SIZE {
            out.push(Self::parse(&mut r)?);
        }
        Ok(out)
    }
}
