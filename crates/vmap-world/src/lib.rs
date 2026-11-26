//! The game world as plain data.
//!
//! [`World`] opens a game through `vmap-fs`, reads the tables that describe
//! its zones and areas, and builds [`Scene`]s: whole stitched maps for the map
//! viewer and single assets for the asset viewer. Nothing here knows about
//! the GPU or the user interface.

pub mod build;
pub mod catalog;
pub mod scene;

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use vmap_formats::gen5::{Arc, AreaHeader, Game, MapMatrix, TextFile, ZoneHeader, CHUNK_SPAN};
use vmap_fs::GameFs;

pub use build::{MapRequest, SceneBuilder};
pub use catalog::{ArchiveInfo, FileKind, Member, SubFile};
pub use scene::*;

pub struct World {
    pub fs: GameFs,
    pub game: Game,
    pub zones: Vec<ZoneHeader>,
    pub areas: Vec<AreaHeader>,
    /// Display names of locations, indexed by a zone's location number.
    pub location_names: Vec<String>,
    pub matrix_count: usize,
    pub chunk_span: f32,
}

/// Summary of one map matrix for lists and the overview.
#[derive(Debug, Clone)]
pub struct MatrixInfo {
    pub id: usize,
    pub width: usize,
    pub height: usize,
    pub cells: usize,
    /// Zones that use this matrix.
    pub zones: Vec<u32>,
    pub name: String,
}

impl World {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let fs = GameFs::open(path)?;
        let game = fs.game();
        let zone_data = fs.arc_file(Arc::ZoneData, 0).ok_or_else(|| anyhow!("zone table not found"))?;
        let zones = ZoneHeader::parse_table(&zone_data).context("reading zone table")?;
        let area_data = fs.arc_raw(Arc::AreaData).ok_or_else(|| anyhow!("area table not found"))?;
        let areas = AreaHeader::parse_table(&area_data).context("reading area table")?;
        let matrix_count = fs.arc_len(Arc::MapMatrices);
        // Location names live in a fixed system text file.
        let names_file = if game.is_bw2() { 109 } else { 89 };
        let location_names = fs
            .arc_file(Arc::SystemText, names_file)
            .and_then(|d| TextFile::parse(&d).ok())
            .map(|t| t.lines().to_vec())
            .unwrap_or_default();
        Ok(Self { fs, game, zones, areas, location_names, matrix_count, chunk_span: CHUNK_SPAN })
    }

    pub fn matrix(&self, id: usize) -> Result<MapMatrix> {
        let data = self.fs.arc_file(Arc::MapMatrices, id).ok_or_else(|| anyhow!("map matrix {id} not found"))?;
        MapMatrix::parse(&data).with_context(|| format!("reading map matrix {id}"))
    }

    /// Zones whose header points at this matrix.
    pub fn zones_of_matrix(&self, matrix: usize) -> Vec<u32> {
        self.zones
            .iter()
            .enumerate()
            .filter(|(_, z)| z.matrix_id as usize == matrix)
            .map(|(i, _)| i as u32)
            .collect()
    }

    pub fn location_name(&self, zone: u32) -> Option<&str> {
        let z = self.zones.get(zone as usize)?;
        self.location_names.get(z.location_name as usize).map(|s| s.as_str()).filter(|s| !s.is_empty())
    }

    /// Human-readable zone name, e.g. `Zone 389 (Nuvema Town)`.
    pub fn zone_label(&self, zone: u32) -> String {
        match self.location_name(zone) {
            Some(n) => format!("Zone {zone} ({n})"),
            None => format!("Zone {zone}"),
        }
    }

    pub fn matrix_info(&self, id: usize) -> Option<MatrixInfo> {
        let m = self.matrix(id).ok()?;
        let mut zones = self.zones_of_matrix(id);
        for z in m.zone_ids() {
            if !zones.contains(&z) {
                zones.push(z);
            }
        }
        zones.sort_unstable();
        let mut names: Vec<&str> = zones.iter().filter_map(|&z| self.location_name(z)).collect();
        names.dedup();
        let name = match names.len() {
            0 => String::new(),
            1..=3 => names.join(", "),
            n => format!("{}, {} and {} more", names[0], names[1], n - 2),
        };
        Some(MatrixInfo { id, width: m.width, height: m.height, cells: m.cell_count(), zones, name })
    }

    pub fn matrices(&self) -> Vec<MatrixInfo> {
        (0..self.matrix_count).filter_map(|i| self.matrix_info(i)).collect()
    }
}
