//! What each tool accepts. The doc comments become the descriptions an agent
//! sees, so they say what a field does and in which units.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeOfDay {
    Morning,
    Day,
    Evening,
    Night,
    LateNight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    /// A camera with perspective that can look from any angle.
    Perspective,
    /// A flat view from straight above, like a plan.
    TopDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Npc,
    Warp,
    Trigger,
    Interactable,
}

/// One entity of a zone.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EntityRef {
    pub kind: EntityKind,
    /// Zone the entity belongs to. May be left out when the view shows a
    /// single zone.
    pub zone: Option<u32>,
    /// Number of the entity within its kind and zone, counting from 0, as
    /// `list_entities` reports it.
    pub index: usize,
}

/// Everything about a map view that can be set directly. Fields left out
/// keep their current value (or their default, for a new view).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ViewChanges {
    /// Draw the ground. Default true.
    pub terrain: Option<bool>,
    /// Draw buildings and other placed objects. Default true.
    pub buildings: Option<bool>,
    /// Draw markers at all: coloured boxes for NPCs (blue), warps (pink),
    /// triggers (yellow) and interactable objects (green). Default true.
    pub markers: Option<bool>,
    /// Draw NPC markers (needs `markers`). Default true.
    pub npcs: Option<bool>,
    /// Draw warp markers (needs `markers`). Default true.
    pub warps: Option<bool>,
    /// Draw trigger markers (needs `markers`). Default true.
    pub triggers: Option<bool>,
    /// Draw markers of interactable objects (needs `markers`). Default true.
    pub interactables: Option<bool>,
    /// Shade with the area's lights. Default true.
    pub lighting: Option<bool>,
    /// Apply textures. Default true.
    pub textures: Option<bool>,
    /// Apply the colours stored in the models' vertices. Default true.
    pub vertex_colors: Option<bool>,
    /// Put water, flowers, windmills and the like into their state at
    /// `time`. Default true.
    pub animations: Option<bool>,
    /// Moment of the animations, in seconds. Default 0.
    pub time: Option<f32>,
    /// Season: changes textures and buildings of outdoor areas.
    pub season: Option<Season>,
    /// Time of day: changes the lighting.
    pub time_of_day: Option<TimeOfDay>,

    /// Zones to hide, on a map that holds several zones.
    pub hide_zones: Option<Vec<u32>>,
    /// Zones to show again.
    pub show_zones: Option<Vec<u32>>,
    /// Show exactly these zones and hide every other zone of the map.
    pub only_zones: Option<Vec<u32>>,
    /// Show every zone of the map again.
    pub show_all_zones: Option<bool>,
    /// Draw the cells of a map with several zones that belong to no zone:
    /// the sea and forest that fill the gaps between zones. Default true.
    /// `only_zones` switches them off and `show_all_zones` on, unless this
    /// says otherwise.
    pub unassigned_cells: Option<bool>,

    /// `perspective` or `top_down`.
    pub projection: Option<Projection>,
    /// Compass heading the camera looks towards, in degrees: 0 north,
    /// 90 east, 180 south, 270 west.
    pub yaw: Option<f32>,
    /// How steeply the camera looks down, in degrees: 90 is straight down,
    /// 10 is almost level. Ignored by `top_down`.
    pub pitch: Option<f32>,
    /// Distance from the camera to the point it looks at, in world units
    /// (one tile is 16 units, one chunk 512). Smaller is closer.
    pub distance: Option<f32>,
    /// Vertical field of view in degrees. Default 45.
    pub fov: Option<f32>,
    /// Point to look at as [x, y, z] in world units. X grows eastwards, Z
    /// southwards, Y upwards.
    pub target: Option<[f32; 3]>,
    /// Look at this zone and move close enough to see all of it.
    pub look_at_zone: Option<u32>,
    /// Look at this tile, given as [x, z] in tiles.
    pub look_at_tile: Option<[f32; 2]>,
    /// Look at this entity from close by.
    pub look_at_entity: Option<EntityRef>,
    /// Frame everything that is visible.
    pub fit: Option<bool>,

    /// Picture width in pixels (64 to 2048). Default 960.
    pub width: Option<u32>,
    /// Picture height in pixels (64 to 2048). Default 640.
    pub height: Option<u32>,
    /// Background colour as `#rrggbb`, or `sky` for the area's own colour.
    pub background: Option<String>,

    /// Also list every marker in the picture with its pixel position.
    pub list_markers: Option<bool>,
    /// Also list every zone in the picture with its name and the pixel its
    /// centre is drawn at: what the window shows as zone names.
    pub list_zones: Option<bool>,
    /// Also write the picture to this file (PNG).
    pub save_to: Option<String>,
    /// Set to false to get only the description, without the picture.
    pub image: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RenderMapParams {
    /// Zone to show. On a map made of several zones only this zone is
    /// drawn, unless `surroundings` is set.
    pub zone: Option<u32>,
    /// Map matrix to show, with all its zones. Not needed with `zone`.
    pub matrix: Option<usize>,
    /// With `zone`: draw the whole map and only look at the zone.
    pub surroundings: Option<bool>,
    #[serde(flatten)]
    pub view: ViewChanges,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AdjustViewParams {
    /// Slide the view as [right, up], in fractions of the picture: [0.5, 0]
    /// brings what is at the right edge to the centre.
    pub pan: Option<[f32; 2]>,
    /// Move the point looked at as [east, south], in world units (16 is one
    /// tile).
    pub move_by: Option<[f32; 2]>,
    /// Turn the camera by this many degrees; positive turns to the right
    /// (clockwise seen from above).
    pub rotate: Option<f32>,
    /// Change how steeply the camera looks down by this many degrees;
    /// positive looks down more.
    pub tilt: Option<f32>,
    /// Come closer by this factor: 2 shows half as much, 0.5 twice as much.
    pub zoom: Option<f32>,
    #[serde(flatten)]
    pub view: ViewChanges,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PickParams {
    /// Pixel column in the last picture of the map view, from the left.
    pub x: f32,
    /// Pixel row in the last picture of the map view, from the top.
    pub y: f32,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OpenGameParams {
    /// A CTRMap project (.cmproj), a folder with an extracted ROM, or a
    /// .nds file.
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ListMapsParams {
    /// Only maps whose name, or the name of a zone on them, contains this
    /// text.
    pub search: Option<String>,
    /// Skip this many results. Default 0.
    pub offset: Option<usize>,
    /// Return at most this many results. Default 60.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ListZonesParams {
    /// Only zones whose name contains this text.
    pub search: Option<String>,
    /// Only zones on this map matrix.
    pub matrix: Option<usize>,
    /// Skip this many results. Default 0.
    pub offset: Option<usize>,
    /// Return at most this many results. Default 100.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ZoneParams {
    pub zone: u32,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MapParams {
    pub matrix: usize,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ListEntitiesParams {
    /// Zone whose entities to list.
    pub zone: Option<u32>,
    /// List the entities of every zone on this map matrix instead.
    pub matrix: Option<usize>,
    /// Only these kinds. Default: all four.
    pub kinds: Option<Vec<EntityKind>>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ChunkParams {
    /// Number of the map chunk.
    pub chunk: usize,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BuildingsParams {
    /// Number of the building bundle (see `get_zone` for a zone's bundle).
    pub bundle: usize,
    /// Read the bundles of interior areas instead of exterior ones.
    pub interior: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadTextParams {
    /// Number of the text file.
    pub file: usize,
    /// Read from the script texts (dialogue) instead of the system texts.
    pub script: Option<bool>,
    /// First line to return. Default 0.
    pub start: Option<usize>,
    /// Number of lines to return. Default 200.
    pub count: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ListArchivesParams {
    /// Only archives whose path or role contains this text.
    pub search: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ListFilesParams {
    /// Archive number: the path `a/X/Y/Z` read as the number XYZ, as
    /// `list_archives` reports it.
    pub archive: u32,
    /// Only files of this kind, for example `3D model`, `textures`, `text`.
    pub kind: Option<String>,
    /// Only files the open project changed.
    pub changed_only: Option<bool>,
    /// Skip this many files. Default 0.
    pub offset: Option<usize>,
    /// Return at most this many files. Default 200.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileParams {
    /// Archive number, as `list_archives` reports it.
    pub archive: u32,
    /// Number of the file in the archive.
    pub file: usize,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RenderModelParams {
    /// Archive number, as `list_archives` reports it.
    pub archive: u32,
    /// Number of the file in the archive.
    pub file: usize,
    /// For a container: which file inside to show. Default: the first
    /// model.
    pub sub_file: Option<usize>,
    /// Compass heading the camera looks towards, in degrees. Default 35.
    pub yaw: Option<f32>,
    /// How steeply the camera looks down, in degrees. Default 28.
    pub pitch: Option<f32>,
    /// Come closer by this factor. Default 1 shows the whole model.
    pub zoom: Option<f32>,
    /// Apply textures. Default true.
    pub textures: Option<bool>,
    /// Shade with lights. Default true.
    pub lighting: Option<bool>,
    /// Apply the colours stored in the vertices. Default true.
    pub vertex_colors: Option<bool>,
    /// Play the model's animations (buildings bring theirs along). Default
    /// true.
    pub animations: Option<bool>,
    /// Moment of the animations, in seconds. Default 0.
    pub time: Option<f32>,
    /// Picture width in pixels (64 to 2048). Default 768.
    pub width: Option<u32>,
    /// Picture height in pixels (64 to 2048). Default 576.
    pub height: Option<u32>,
    /// Background colour as `#rrggbb`.
    pub background: Option<String>,
    /// Also write the picture to this file (PNG).
    pub save_to: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TexturesParams {
    /// Archive number, as `list_archives` reports it.
    pub archive: u32,
    /// Number of the file in the archive.
    pub file: usize,
    /// For a container: which file inside to read. Default: the first one
    /// with textures.
    pub sub_file: Option<usize>,
    /// Return only the texture with this name, instead of a sheet of all.
    pub texture: Option<String>,
    /// Palette to use with `texture`. Default: the one that matches by name.
    pub palette: Option<String>,
    /// Enlargement, 1 to 16. Default 2 for a sheet, 4 for one texture.
    pub zoom: Option<u32>,
    /// Also write the picture to this file (PNG).
    pub save_to: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FollowWarpParams {
    /// Number of the warp in its zone, as `list_entities` reports it.
    pub warp: usize,
    /// Zone the warp belongs to. Default: the zone the map view was opened
    /// for.
    pub zone: Option<u32>,
    /// Draw the whole map the warp leads to, not only the zone.
    pub surroundings: Option<bool>,
}
