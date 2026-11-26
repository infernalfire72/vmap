//! Map matrices: a grid of chunk numbers, optionally with the zone that owns
//! each cell.

use crate::error::{invalid, Result};
use crate::reader::Reader;

#[derive(Debug, Clone, Default)]
pub struct MapMatrix {
    pub width: usize,
    pub height: usize,
    /// Chunk number per cell, row by row; `None` for empty cells.
    pub chunks: Vec<Option<u32>>,
    /// Owning zone per cell, if the matrix is shared between zones.
    pub zones: Option<Vec<Option<u32>>>,
}

impl MapMatrix {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let has_zones = r.u32()? == 1;
        let width = r.u16()? as usize;
        let height = r.u16()? as usize;
        if width * height > 4096 {
            return invalid("map matrix is implausibly large");
        }
        let read_grid = |r: &mut Reader| -> Result<Vec<Option<u32>>> {
            let mut g = Vec::with_capacity(width * height);
            for _ in 0..width * height {
                let v = r.i32()?;
                g.push((v >= 0).then_some(v as u32));
            }
            Ok(g)
        };
        let chunks = read_grid(&mut r)?;
        let zones = if has_zones { Some(read_grid(&mut r)?) } else { None };
        Ok(Self { width, height, chunks, zones })
    }

    pub fn chunk(&self, x: usize, y: usize) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.chunks[y * self.width + x]
    }

    /// Zone that owns a cell. `None` if the matrix has no zone grid or the
    /// cell is unassigned.
    pub fn zone(&self, x: usize, y: usize) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.zones.as_ref().and_then(|z| z[y * self.width + x])
    }

    /// All distinct zones referenced by the zone grid.
    pub fn zone_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.zones.iter().flatten().flatten().copied().collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub fn cell_count(&self) -> usize {
        self.chunks.iter().flatten().count()
    }
}
