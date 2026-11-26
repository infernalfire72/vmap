//! Building bundles: every building model an area can place, with a small
//! header per building (its number, door and animation slots).

use crate::error::Result;
use crate::gf::GfContainer;
use crate::reader::Reader;

#[derive(Debug, Clone)]
pub struct BuildingResource<'a> {
    pub uid: u16,
    pub kind: u16,
    /// Building number of the door model attached to this building.
    pub door_uid: Option<u16>,
    pub door_pos: [i16; 3],
    /// 0 none, 1 generic ambient, 2 scripted, 3 time of day.
    pub anim_control: u8,
    pub anim_set_size: u8,
    /// Raw animation files (Nitro resources), up to four.
    pub animations: Vec<&'a [u8]>,
    /// Raw `BMD0` model.
    pub model: &'a [u8],
}

impl<'a> BuildingResource<'a> {
    /// Whether the building animates on its own (a windmill, a fountain)
    /// rather than when a script or the clock says so.
    pub fn plays_by_itself(&self) -> bool {
        self.anim_control == 1 && !self.animations.is_empty()
    }

    /// Animation files that play together in the ambient loop.
    pub fn ambient_animations(&self) -> &[&'a [u8]] {
        let n = if self.anim_set_size == 0 { self.animations.len() } else { self.anim_set_size as usize };
        &self.animations[..n.min(self.animations.len())]
    }
}

#[derive(Debug, Clone, Default)]
pub struct BuildingBundle<'a> {
    pub buildings: Vec<BuildingResource<'a>>,
}

impl<'a> BuildingBundle<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let c = GfContainer::parse(data)?;
        let count = c.len() / 2;
        let mut buildings = Vec::with_capacity(count);
        for i in 0..count {
            let hdr = c.get(data, i).unwrap_or(&[]);
            let model = c.get(data, count + i).unwrap_or(&[]);
            if let Ok(b) = parse_resource(hdr, model) {
                buildings.push(b);
            }
        }
        Ok(Self { buildings })
    }

    pub fn by_uid(&self, uid: u16) -> Option<&BuildingResource<'a>> {
        self.buildings.iter().find(|b| b.uid == uid)
    }
}

fn parse_resource<'a>(hdr: &'a [u8], model: &'a [u8]) -> Result<BuildingResource<'a>> {
    let mut r = Reader::new(hdr);
    let uid = r.u16()?;
    let kind = r.u16()?;
    let door = r.i16()?;
    let door_pos = [r.i16()?, r.i16()?, r.i16()?];
    let _unknown = r.i16()?;
    let _padding = r.u16()?;
    let base = r.pos();
    let anim_control = r.u8()?;
    let _unknown = r.u8()?;
    let anim_set_size = r.u8()?;
    let _anim_count = r.u8()?;
    let mut animations = Vec::new();
    for _ in 0..4 {
        let off = r.i32()?;
        if off < 0 {
            continue;
        }
        // Each animation is a complete Nitro file; its size sits at +8.
        let start = base + off as usize;
        if let Ok(len) = Reader::at(hdr, start + 8).u32() {
            if let Some(a) = hdr.get(start..start + len as usize) {
                animations.push(a);
            }
        }
    }
    Ok(BuildingResource {
        uid,
        kind,
        door_uid: (door >= 0).then_some(door as u16),
        door_pos,
        anim_control,
        anim_set_size,
        animations,
        model,
    })
}
