//! Map chunks: one 32x32-tile piece of terrain with its model, per-tile
//! movement data and the buildings placed on it.

use glam::Vec3;

use crate::error::Result;
use crate::gf::GfContainer;
use crate::reader::Reader;

/// A building placed on a chunk, relative to the chunk centre.
#[derive(Debug, Clone, Copy)]
pub struct ChunkBuilding {
    /// Offset from the chunk centre. Z grows towards the top of the map here
    /// (the opposite of world Z).
    pub position: Vec3,
    /// Rotation about the vertical axis, in degrees.
    pub rotation_deg: f32,
    pub model_uid: u16,
}

impl ChunkBuilding {
    pub fn parse_list(data: &[u8]) -> Result<Vec<Self>> {
        let mut r = Reader::new(data);
        let count = r.u32()? as usize;
        let mut out = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let position = r.vec3_fx32()?;
            let rotation_deg = r.u16()? as f32 * (360.0 / 65536.0);
            let b = r.bytes(2)?;
            // The model number is stored big-endian.
            let model_uid = u16::from_be_bytes([b[0], b[1]]);
            out.push(Self { position, rotation_deg, model_uid });
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightKind {
    Default,
    Plain,
    Corner,
}

#[derive(Debug, Clone, Copy)]
pub struct Tile {
    pub height_kind: HeightKind,
    pub slope: u16,
    pub height: u16,
    pub tile_class: u16,
    pub flags: u16,
}

/// Per-tile movement and height data of a chunk.
#[derive(Debug, Clone, Default)]
pub struct Terrain {
    pub width: usize,
    pub height: usize,
    pub tiles: Vec<Tile>,
}

impl Terrain {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let width = r.u16()? as usize;
        let height = r.u16()? as usize;
        let mut tiles = Vec::with_capacity((width * height).min(4096));
        for _ in 0..width * height {
            let first = r.u16()?;
            let h = r.u16()?;
            let kind = r.u32()?;
            tiles.push(Tile {
                height_kind: match first & 3 {
                    1 => HeightKind::Plain,
                    2 => HeightKind::Corner,
                    _ => HeightKind::Default,
                },
                slope: first >> 2,
                height: h,
                tile_class: kind as u16,
                flags: (kind >> 16) as u16,
            });
        }
        Ok(Self { width, height, tiles })
    }

    pub fn tile(&self, x: usize, y: usize) -> Option<&Tile> {
        (x < self.width && y < self.height).then(|| &self.tiles[y * self.width + x])
    }
}

/// A parsed map chunk. The model is kept as raw bytes for the model parser.
#[derive(Debug, Clone)]
pub struct MapChunk<'a> {
    /// Container signature (`WB` for ordinary grid maps).
    pub kind: String,
    pub model: &'a [u8],
    pub terrain: Option<Terrain>,
    pub buildings: Vec<ChunkBuilding>,
}

impl<'a> MapChunk<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let c = GfContainer::parse(data)?;
        let model = c.get(data, 0).unwrap_or(&[]);
        let terrain = if c.magic == "WB" {
            c.get(data, 1).and_then(|t| Terrain::parse(t).ok())
        } else {
            None
        };
        let buildings = match c.last(data) {
            Some(b) if c.len() > 1 => ChunkBuilding::parse_list(b).unwrap_or_default(),
            _ => Vec::new(),
        };
        Ok(Self { kind: c.magic, model, terrain, buildings })
    }
}
