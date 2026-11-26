//! Generation V (Black/White and Black 2/White 2) world data: zones, map
//! matrices, areas, map chunks, buildings and zone entities.

pub mod anim;
pub mod area;
pub mod building;
pub mod chunk;
pub mod entities;
pub mod light;
pub mod matrix;
pub mod text;
pub mod zone;

pub use anim::{parse_area_patterns, AreaPattern, AreaPatternKey};
pub use area::AreaHeader;
pub use building::{BuildingBundle, BuildingResource};
pub use chunk::{ChunkBuilding, MapChunk, Terrain};
pub use entities::ZoneEntities;
pub use light::LightEntry;
pub use matrix::MapMatrix;
pub use text::TextFile;
pub use zone::ZoneHeader;

/// World units per map tile.
pub const TILE_SIZE: f32 = 16.0;
/// World units per map chunk (32 x 32 tiles) unless a map type overrides it.
pub const CHUNK_SPAN: f32 = 512.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Game {
    Black,
    White,
    Black2,
    White2,
}

impl Game {
    /// Detects the game from the four-letter ROM game code.
    pub fn from_game_code(code: &str) -> Option<Self> {
        match code.get(0..3)? {
            "IRB" => Some(Self::Black),
            "IRA" => Some(Self::White),
            "IRE" => Some(Self::Black2),
            "IRD" => Some(Self::White2),
            _ => None,
        }
    }

    pub fn is_bw2(self) -> bool {
        matches!(self, Self::Black2 | Self::White2)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Black => "Pokemon Black",
            Self::White => "Pokemon White",
            Self::Black2 => "Pokemon Black 2",
            Self::White2 => "Pokemon White 2",
        }
    }
}

/// The game archives this tool reads. Archive numbers differ between the
/// first and second pair of games.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arc {
    SystemText,
    ScriptText,
    MapChunks,
    MapMatrices,
    MapReplaceTable,
    ZoneData,
    AreaData,
    MapTextures,
    NpcModelIndex,
    NpcModels,
    Scripts,
    EnvLights,
    MapAnimSrt,
    MapAnimPattern,
    RailData,
    ZoneEntities,
    ZoneEncounters,
    BuildingTexExterior,
    BuildingTexInterior,
    BuildingsExterior,
    BuildingsInterior,
}

impl Arc {
    pub const ALL: [Arc; 21] = [
        Arc::SystemText,
        Arc::ScriptText,
        Arc::MapChunks,
        Arc::MapMatrices,
        Arc::MapReplaceTable,
        Arc::ZoneData,
        Arc::AreaData,
        Arc::MapTextures,
        Arc::NpcModelIndex,
        Arc::NpcModels,
        Arc::Scripts,
        Arc::EnvLights,
        Arc::MapAnimSrt,
        Arc::MapAnimPattern,
        Arc::RailData,
        Arc::ZoneEntities,
        Arc::ZoneEncounters,
        Arc::BuildingTexExterior,
        Arc::BuildingTexInterior,
        Arc::BuildingsExterior,
        Arc::BuildingsInterior,
    ];

    /// Archive number as (Black/White, Black 2/White 2).
    fn ids(self) -> (u32, u32) {
        match self {
            Arc::SystemText => (2, 2),
            Arc::ScriptText => (3, 3),
            Arc::MapChunks => (8, 8),
            Arc::MapMatrices => (9, 9),
            Arc::MapReplaceTable => (10, 10),
            Arc::ZoneData => (12, 12),
            Arc::AreaData => (13, 13),
            Arc::MapTextures => (14, 14),
            Arc::NpcModelIndex => (48, 47),
            Arc::NpcModels => (49, 48),
            Arc::Scripts => (57, 56),
            Arc::EnvLights => (61, 60),
            Arc::MapAnimSrt => (69, 68),
            Arc::MapAnimPattern => (70, 69),
            Arc::RailData => (79, 78),
            Arc::ZoneEntities => (125, 126),
            Arc::ZoneEncounters => (126, 127),
            Arc::BuildingTexExterior => (176, 174),
            Arc::BuildingTexInterior => (177, 175),
            Arc::BuildingsExterior => (229, 225),
            Arc::BuildingsInterior => (230, 226),
        }
    }

    pub fn id(self, game: Game) -> u32 {
        let (bw, bw2) = self.ids();
        if game.is_bw2() { bw2 } else { bw }
    }

    /// Path inside the ROM's data directory, e.g. `a/0/0/8`.
    pub fn path(self, game: Game) -> String {
        arc_path(self.id(game))
    }

    pub fn label(self) -> &'static str {
        match self {
            Arc::SystemText => "System text",
            Arc::ScriptText => "Script text",
            Arc::MapChunks => "Map chunks",
            Arc::MapMatrices => "Map matrices",
            Arc::MapReplaceTable => "Map replacement table",
            Arc::ZoneData => "Zone data",
            Arc::AreaData => "Area data",
            Arc::MapTextures => "Map textures",
            Arc::NpcModelIndex => "NPC model index",
            Arc::NpcModels => "NPC models",
            Arc::Scripts => "Scripts",
            Arc::EnvLights => "Environment lights",
            Arc::MapAnimSrt => "Map texture animations",
            Arc::MapAnimPattern => "Map pattern animations",
            Arc::RailData => "Rail data",
            Arc::ZoneEntities => "Zone entities",
            Arc::ZoneEncounters => "Zone encounters",
            Arc::BuildingTexExterior => "Building textures (exterior)",
            Arc::BuildingTexInterior => "Building textures (interior)",
            Arc::BuildingsExterior => "Buildings (exterior)",
            Arc::BuildingsInterior => "Buildings (interior)",
        }
    }
}

pub fn arc_path(id: u32) -> String {
    format!("a/{}/{}/{}", id / 100 % 10, id / 10 % 10, id % 10)
}

/// In Black/White the building bundle is derived from the area number instead
/// of being stored in the area header.
pub fn bw1_buildings_id(area_id: usize) -> usize {
    if area_id >= 210 {
        area_id - 210
    } else if area_id >= 2 {
        (area_id - 2) / 4
    } else {
        0
    }
}
