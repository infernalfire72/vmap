//! Zone entities: the interactable furniture, NPCs, warps and script triggers
//! placed in a zone.

use crate::error::Result;
use crate::reader::Reader;

/// Where an entity stands: on the tile grid or on a rail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// World-space position.
    Grid { x: f32, y: f32, z: f32 },
    Rail { line: u16, front: u16, side: i16 },
}

impl Placement {
    pub fn world(&self) -> Option<[f32; 3]> {
        match *self {
            Placement::Grid { x, y, z } => Some([x, y, z]),
            Placement::Rail { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Furniture {
    pub script: u16,
    pub condition: u16,
    pub interactibility: u16,
    pub placement: Placement,
}

#[derive(Debug, Clone)]
pub struct Npc {
    pub uid: u16,
    /// Overworld model number.
    pub obj_code: u16,
    pub move_code: u16,
    pub event_type: u16,
    pub spawn_flag: u16,
    pub script: u16,
    pub face_direction: u16,
    pub params: [u16; 3],
    pub area_width: i16,
    pub area_height: i16,
    pub placement: Placement,
}

#[derive(Debug, Clone)]
pub struct Warp {
    pub target_zone: u16,
    pub target_warp: u16,
    pub face_direction: u8,
    pub transition: u8,
    pub placement: Placement,
    pub width: i16,
    pub height: i16,
}

#[derive(Debug, Clone)]
pub struct Trigger {
    pub script: u16,
    pub work_value: u16,
    pub work_id: u16,
    pub kind: u16,
    pub placement: Placement,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Default)]
pub struct ZoneEntities {
    pub furniture: Vec<Furniture>,
    pub npcs: Vec<Npc>,
    pub warps: Vec<Warp>,
    pub triggers: Vec<Trigger>,
}

const TILE: f32 = super::TILE_SIZE;

fn tile_centre(t: i32) -> f32 {
    (t as f32 + 0.5) * TILE
}

impl ZoneEntities {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let _length = r.u32()?;
        let furniture_count = r.u8()?;
        let npc_count = r.u8()?;
        let warp_count = r.u8()?;
        let trigger_count = r.u8()?;

        let mut out = Self::default();
        for _ in 0..furniture_count {
            let script = r.u16()?;
            let condition = r.u16()?;
            let interactibility = r.u16()?;
            let on_rail = r.u16()? != 0;
            let placement = if on_rail {
                let p = Placement::Rail { line: r.u16()?, front: r.u16()?, side: r.i16()? };
                r.skip(2);
                r.skip(4);
                p
            } else {
                let gx = r.i32()?;
                let gz = r.i32()?;
                let y = r.fx32()?;
                Placement::Grid { x: tile_centre(gx), y, z: tile_centre(gz) }
            };
            out.furniture.push(Furniture { script, condition, interactibility, placement });
        }
        for _ in 0..npc_count {
            let uid = r.u16()?;
            let obj_code = r.u16()?;
            let move_code = r.u16()?;
            let event_type = r.u16()?;
            let spawn_flag = r.u16()?;
            let script = r.u16()?;
            let face_direction = r.u16()?;
            let params = [r.u16()?, r.u16()?, r.u16()?];
            let area_width = r.i16()?;
            let area_height = r.i16()?;
            let on_rail = r.u32()? != 0;
            let placement = if on_rail {
                let p = Placement::Rail { line: r.u16()?, front: r.u16()?, side: r.i16()? };
                r.skip(2);
                p
            } else {
                let gx = r.u16()? as i32;
                let gz = r.u16()? as i32;
                let y = r.fx32()?;
                Placement::Grid { x: tile_centre(gx), y, z: tile_centre(gz) }
            };
            out.npcs.push(Npc {
                uid,
                obj_code,
                move_code,
                event_type,
                spawn_flag,
                script,
                face_direction,
                params,
                area_width,
                area_height,
                placement,
            });
        }
        for _ in 0..warp_count {
            let target_zone = r.u16()?;
            let target_warp = r.u16()?;
            let face_direction = r.u8()?;
            let transition = r.u8()?;
            let on_rail = r.u16()? == 1;
            let placement = if on_rail {
                Placement::Rail { line: r.u16()?, front: r.u16()?, side: r.i16()? }
            } else {
                let x = r.i16()? as f32;
                let y = r.i16()? as f32;
                let z = r.i16()? as f32;
                Placement::Grid { x, y, z }
            };
            let width = r.i16()?;
            let height = r.i16()?;
            let _unknown = r.u16()?;
            out.warps.push(Warp { target_zone, target_warp, face_direction, transition, placement, width, height });
        }
        for _ in 0..trigger_count {
            let script = r.u16()?;
            let work_value = r.u16()?;
            let work_id = r.u16()?;
            let kind = r.u16()?;
            let on_rail = r.i16()? != 0;
            let (placement, width, height) = if on_rail {
                let p = Placement::Rail { line: r.u16()?, front: r.u16()?, side: r.i16()? };
                (p, r.u16()?, r.u16()?)
            } else {
                let gx = r.i16()? as i32;
                let gz = r.i16()? as i32;
                let w = r.u16()?;
                let h = r.u16()?;
                let y = r.i16()? as f32;
                (Placement::Grid { x: gx as f32 * TILE, y, z: gz as f32 * TILE }, w, h)
            };
            let _unknown = r.i16()?;
            out.triggers.push(Trigger { script, work_value, work_id, kind, placement, width, height });
        }
        Ok(out)
    }
}
