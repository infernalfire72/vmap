//! The map view: what `render_map` opens, `adjust_view` moves and `pick`
//! asks questions about.

use std::f32::consts::{FRAC_PI_2, TAU};

use anyhow::{anyhow, bail, Result};
use glam::Vec3;
use serde_json::{json, Map, Value};
use vmap_formats::gen5::TILE_SIZE;
use vmap_render::{Camera, GpuScene, Headless, OrbitCamera, RenderOptions};
use vmap_world::{MapRequest, Marker, NodeKind, Scene, World};

use crate::info::{entity_value, kind_name, marker_kind};
use crate::params::{
    AdjustViewParams, EntityKind, EntityRef, FollowWarpParams, PickParams, Projection, RenderMapParams, ViewChanges,
};
use crate::reply::{parse_color, round, save_png, to_text, vec3, Picture, Reply};
use crate::state::{OpenGame, State};

const SEASONS: [&str; 4] = ["spring", "summer", "autumn", "winter"];
const TIMES_OF_DAY: [&str; 5] = ["morning", "day", "evening", "night", "late_night"];
const SIZE_RANGE: std::ops::RangeInclusive<u32> = 64..=2048;
const DISTANCE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=400_000.0;
const MIN_PITCH: f32 = 0.087; // 5 degrees
/// How many built maps are kept for going back to.
const KEPT_SCENES: usize = 4;
/// How many markers `list_markers` reports at most.
const LISTED_MARKERS: usize = 200;

/// Everything that decides what a built map contains.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneKey {
    pub matrix: usize,
    /// Draw only this zone of the map.
    pub only_zone: Option<u32>,
    pub season: u8,
    pub day_part: u16,
    pub terrain: bool,
    pub buildings: bool,
    pub animations: bool,
    /// Zones left out, in ascending order.
    pub hidden: Vec<u32>,
    /// Draw the cells that belong to no zone.
    pub unassigned: bool,
}

/// A map built for one [`SceneKey`], ready to draw.
pub struct Built {
    pub key: SceneKey,
    pub scene: Scene,
    pub gpu: GpuScene,
}

#[derive(Debug, Clone)]
pub struct MapView {
    pub key: SceneKey,
    /// Zone the view was opened for, if it was opened for one.
    pub zone: Option<u32>,
    /// Zones on the map's grid; empty if the map has no zone grid.
    pub grid_zones: Vec<u32>,
    pub options: RenderOptions,
    /// Moment of the animations, in seconds.
    pub time: f32,
    pub camera: OrbitCamera,
    pub width: u32,
    pub height: u32,
}

impl MapView {
    fn aspect(&self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }

    fn shows(&self, zone: u32) -> bool {
        self.key.only_zone.is_none_or(|only| only == zone) && !self.key.hidden.contains(&zone)
    }

    fn visible_zones(&self) -> Vec<u32> {
        self.grid_zones.iter().copied().filter(|z| self.shows(*z)).collect()
    }
}

fn no_view() -> anyhow::Error {
    anyhow!("there is no map view yet; open one with render_map")
}

fn finite(name: &str, value: f32) -> Result<f32> {
    if value.is_finite() {
        Ok(value)
    } else {
        bail!("{name} must be a number")
    }
}

// ------------------------------------------------------------------ tools

pub fn render_map(state: &mut State, p: RenderMapParams) -> Result<Reply> {
    let (game, headless) = state.game_and_renderer()?;
    let mut notes = Vec::new();
    let view = open_view(&game.world, p.zone, p.matrix, p.surroundings.unwrap_or(false), None, &mut notes)?;
    shoot(game, headless, view, &p.view, true, None, notes, Map::new())
}

pub fn adjust_view(state: &mut State, p: AdjustViewParams) -> Result<Reply> {
    let (game, headless) = state.game_and_renderer()?;
    let view = game.view.clone().ok_or_else(no_view)?;
    shoot(game, headless, view, &p.view, false, Some(&p), Vec::new(), Map::new())
}

pub fn get_view(state: &mut State) -> Result<Reply> {
    let (game, headless) = state.game_and_renderer()?;
    let view = game.view.clone().ok_or_else(no_view)?;
    let OpenGame { world, scenes, .. } = game;
    let built = built(scenes, world, headless, &view.key)?;
    Ok(Reply::json(&describe(&view, world, &built.scene)))
}

pub fn follow_warp(state: &mut State, p: FollowWarpParams) -> Result<Reply> {
    let (game, headless) = state.game_and_renderer()?;
    let current = game.view.clone();
    let zone = p
        .zone
        .or(current.as_ref().and_then(|v| v.zone))
        .ok_or_else(|| anyhow!("say which zone the warp belongs to (the map view was not opened for one zone)"))?;
    let world = &game.world;
    let entities = world
        .zone_entities(zone)
        .ok_or_else(|| anyhow!("{} has no entities", world.zone_label(zone)))??;
    let warp = entities.warps.get(p.warp).ok_or_else(|| {
        anyhow!("{} has {} warps (numbered from 0); there is no warp {}", world.zone_label(zone), entities.warps.len(), p.warp)
    })?;
    let (to_zone, to_warp) = (warp.target_zone as u32, warp.target_warp as usize);
    if to_zone as usize >= world.zones.len() {
        bail!(
            "warp {} of {} leads to zone {to_zone}, which does not exist: the game decides where it leads while it runs",
            p.warp,
            world.zone_label(zone)
        );
    }
    let mut notes = Vec::new();
    let view = open_view(world, Some(to_zone), None, p.surroundings.unwrap_or(false), current.as_ref(), &mut notes)?;
    let changes = ViewChanges {
        look_at_entity: Some(EntityRef { kind: EntityKind::Warp, zone: Some(to_zone), index: to_warp }),
        ..Default::default()
    };
    let mut extra = Map::new();
    extra.insert(
        "followed".into(),
        json!({
            "from_zone": zone,
            "warp": p.warp,
            "to_zone": to_zone,
            "to_zone_name": world.location_name(to_zone),
            "arrives_at_warp": to_warp,
        }),
    );
    shoot(game, headless, view, &changes, true, None, notes, extra)
}

pub fn pick(state: &mut State, p: PickParams) -> Result<Reply> {
    let (game, headless) = state.game_and_renderer()?;
    let view = game.view.clone().ok_or_else(no_view)?;
    let (width, height) = (view.width as f32, view.height as f32);
    if !(0.0..=width).contains(&p.x) || !(0.0..=height).contains(&p.y) {
        bail!("the picture is {} by {} pixels; ({}, {}) lies outside it", view.width, view.height, p.x, p.y);
    }
    let OpenGame { world, scenes, .. } = game;
    let scene = &built(scenes, world, headless, &view.key)?.scene;
    let ndc = [p.x / width * 2.0 - 1.0, 1.0 - p.y / height * 2.0];
    let (origin, dir) = view.camera.ray(ndc, view.aspect());

    let surface = scene.ray_cast(origin, dir);
    let marker = scene
        .markers
        .iter()
        .filter(|m| view.options.markers && view.options.marker_kinds[m.kind.index()])
        .filter_map(|m| m.ray_hit(origin, dir).map(|t| (t, m)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        // A marker behind a wall is not what the pixel shows.
        .filter(|(t, _)| surface.is_none_or(|s| *t <= s.distance + 0.5));

    let mut out = Map::new();
    out.insert("pixel".into(), json!([p.x, p.y]));
    if let Some((_, m)) = marker {
        let mut value = marker_value(m);
        let details = world
            .zone_entities(m.zone)
            .and_then(|e| e.ok())
            .and_then(|e| entity_value(world, &e, marker_kind(m.kind), m.index));
        if let Some(details) = details {
            value["details"] = details;
        }
        out.insert("marker".into(), value);
    }
    match surface {
        Some(hit) => {
            let mut value = json!({ "is": node_name(&scene.nodes[hit.node].kind) });
            match scene.nodes[hit.node].kind {
                NodeKind::Building { uid, .. } | NodeKind::Door { uid } => value["building_model"] = json!(uid),
                _ => {}
            }
            value["position"] = vec3(hit.point);
            value["tile"] = tile(hit.point);
            place(scene, world, hit.point, &mut value);
            out.insert(if marker.is_some() { "behind_it" } else { "surface" }.into(), value);
        }
        None => {
            // Nothing drawn there: say where the pixel lies on the level the
            // camera looks at.
            if let Some(point) = view.camera.pick_ground(ndc, view.aspect(), view.camera.target.y) {
                let mut value = json!({ "position": vec3(point), "tile": tile(point) });
                place(scene, world, point, &mut value);
                out.insert("empty_at_target_height".into(), value);
            }
        }
    }
    if marker.is_none() && surface.is_none() {
        out.insert("note".into(), json!("nothing is drawn at this pixel"));
    }
    Ok(Reply::json(&Value::Object(out)))
}

// ------------------------------------------------------------ opening a view

/// A new view of a zone or a whole map, with default settings, or with the
/// settings of `inherit` (but not its camera position).
fn open_view(
    world: &World,
    zone: Option<u32>,
    matrix: Option<usize>,
    surroundings: bool,
    inherit: Option<&MapView>,
    notes: &mut Vec<String>,
) -> Result<MapView> {
    let (matrix, zone) = match (zone, matrix) {
        (Some(z), asked) => {
            let header = world
                .zones
                .get(z as usize)
                .ok_or_else(|| anyhow!("there is no zone {z}; the game has zones 0 to {}", world.zones.len().saturating_sub(1)))?;
            let own = header.matrix_id as usize;
            if let Some(m) = asked.filter(|m| *m != own) {
                notes.push(format!("zone {z} lies on matrix {own}, not {m}; showing matrix {own}"));
            }
            (own, Some(z))
        }
        (None, Some(m)) => (m, None),
        (None, None) => bail!("give a zone or a matrix; list_zones and list_maps show what there is"),
    };
    if matrix >= world.matrix_count {
        bail!("there is no map matrix {matrix}; the game has matrices 0 to {}", world.matrix_count.saturating_sub(1));
    }
    let grid = world.matrix(matrix)?;
    let grid_zones = if grid.zones.is_some() { grid.zone_ids() } else { Vec::new() };
    let mut only_zone = zone;
    if !grid_zones.is_empty() {
        match zone {
            Some(z) if !grid_zones.contains(&z) => {
                notes.push(format!("zone {z} uses matrix {matrix} but owns no cell of it; showing the whole map"));
                only_zone = None;
            }
            Some(_) if surroundings => only_zone = None,
            _ => {}
        }
    } else if surroundings {
        notes.push("this map holds a single zone, so there are no surroundings to add".into());
    }

    let mut view = MapView {
        key: SceneKey {
            matrix,
            only_zone,
            season: 0,
            day_part: 1,
            terrain: true,
            buildings: true,
            animations: true,
            hidden: Vec::new(),
            unassigned: true,
        },
        zone,
        grid_zones,
        options: RenderOptions::default(),
        time: 0.0,
        camera: OrbitCamera::default(),
        width: 960,
        height: 640,
    };
    if let Some(old) = inherit {
        view.key.season = old.key.season;
        view.key.day_part = old.key.day_part;
        view.key.terrain = old.key.terrain;
        view.key.buildings = old.key.buildings;
        view.key.animations = old.key.animations;
        view.options = old.options;
        view.time = old.time;
        (view.width, view.height) = (old.width, old.height);
        view.camera = OrbitCamera { target: Vec3::ZERO, distance: view.camera.distance, ..old.camera };
    }
    Ok(view)
}

/// Returns the map built for `key`, building it if it is not among the
/// ones kept.
fn built<'a>(scenes: &'a mut Vec<Built>, world: &World, headless: &Headless, key: &SceneKey) -> Result<&'a mut Built> {
    match scenes.iter().position(|b| &b.key == key) {
        Some(0) => {}
        Some(i) => {
            let b = scenes.remove(i);
            scenes.insert(0, b);
        }
        None => {
            let request = MapRequest {
                matrix: key.matrix,
                only_zone: key.only_zone,
                season: key.season,
                day_part: key.day_part,
                terrain: key.terrain,
                buildings: key.buildings,
                // Always built, and switched on and off while drawing.
                markers: true,
                animations: key.animations,
                hidden_zones: key.hidden.clone(),
                hide_unassigned: !key.unassigned,
            };
            let scene = world.build_map(&request)?;
            let gpu = headless.upload(&scene);
            scenes.insert(0, Built { key: key.clone(), scene, gpu });
            scenes.truncate(KEPT_SCENES);
        }
    }
    Ok(&mut scenes[0])
}

/// Applies changes to a view, draws it and describes the result. The view
/// becomes the current one only if all of that worked.
#[allow(clippy::too_many_arguments)]
fn shoot(
    game: &mut OpenGame,
    headless: &mut Headless,
    mut view: MapView,
    changes: &ViewChanges,
    fresh: bool,
    relative: Option<&AdjustViewParams>,
    mut notes: Vec<String>,
    extra: Map<String, Value>,
) -> Result<Reply> {
    let protected = game.protected_folders();
    let OpenGame { world, scenes, .. } = &mut *game;
    apply_settings(&mut view, changes, &mut notes)?;
    let built = built(scenes, world, headless, &view.key)?;
    apply_camera(&mut view, &built.scene, changes, fresh, &mut notes)?;
    if let Some(relative) = relative {
        apply_relative(&mut view, relative, &mut notes)?;
    }

    let wants_picture = changes.image != Some(false);
    let picture = if wants_picture || changes.save_to.is_some() {
        let time = if view.key.animations { view.time } else { 0.0 };
        built.gpu.animate(&headless.gpu.queue, time);
        let camera = view.camera.camera(view.aspect());
        let rgba = headless.render(&built.gpu, &camera, &view.options, view.width, view.height)?;
        let picture = Picture::new(view.width, view.height, rgba);
        let png = picture.png()?;
        Some((picture, png))
    } else {
        None
    };

    let mut out = extra;
    if let Value::Object(described) = describe(&view, world, &built.scene) {
        out.extend(described);
    }
    if let (Some(path), Some((_, png))) = (&changes.save_to, &picture) {
        out["picture"]["saved_to"] = json!(save_png(path, png, &protected)?);
    }
    let image = match picture {
        Some((picture, png)) if wants_picture => Some(picture.to_send(png)?),
        _ => None,
    };
    if image.as_ref().is_some_and(|i| i.is_lossy()) {
        // Too large to send as PNG; `save_to` always writes the PNG.
        out["picture"]["sent_as"] = json!("jpeg");
    }
    if changes.list_markers == Some(true) {
        out.insert("visible_markers".into(), visible_markers(&view, &built.scene, &mut notes));
    }
    if changes.list_zones == Some(true) {
        out.insert("zones_in_picture".into(), zones_in_picture(&view, world, &built.scene));
    }
    if built.scene.nodes.is_empty() {
        notes.push("nothing was built for this view: every part of it is switched off or hidden".into());
    }
    if !notes.is_empty() {
        out.insert("notes".into(), json!(notes));
    }
    game.view = Some(view);
    Ok(Reply { text: to_text(&Value::Object(out)), image })
}

// --------------------------------------------------------- applying changes

/// Everything but the camera.
fn apply_settings(view: &mut MapView, c: &ViewChanges, notes: &mut Vec<String>) -> Result<()> {
    let key = &mut view.key;
    key.terrain = c.terrain.unwrap_or(key.terrain);
    key.buildings = c.buildings.unwrap_or(key.buildings);
    key.animations = c.animations.unwrap_or(key.animations);
    if let Some(season) = c.season {
        key.season = season as u8;
    }
    if let Some(time_of_day) = c.time_of_day {
        key.day_part = time_of_day as u16;
    }
    if let Some(time) = c.time {
        if finite("time", time)? < 0.0 {
            bail!("time must not be negative");
        }
        view.time = time;
    }

    let o = &mut view.options;
    o.markers = c.markers.unwrap_or(o.markers);
    for (slot, change) in [c.npcs, c.warps, c.triggers, c.interactables].into_iter().enumerate() {
        o.marker_kinds[slot] = change.unwrap_or(o.marker_kinds[slot]);
    }
    o.lighting = c.lighting.unwrap_or(o.lighting);
    o.textures = c.textures.unwrap_or(o.textures);
    o.vertex_colors = c.vertex_colors.unwrap_or(o.vertex_colors);
    if let Some(background) = &c.background {
        o.background = if background.trim().eq_ignore_ascii_case("sky") {
            None
        } else {
            Some(parse_color(background)?.map(|b| b as f32 / 255.0))
        };
    }
    if !o.markers && [c.npcs, c.warps, c.triggers, c.interactables].contains(&Some(true)) {
        notes.push("markers are switched off as a whole, so no kind of marker is drawn; set markers to true".into());
    }

    for (name, change, size) in [("width", c.width, &mut view.width), ("height", c.height, &mut view.height)] {
        if let Some(asked) = change {
            *size = asked.clamp(*SIZE_RANGE.start(), *SIZE_RANGE.end());
            if *size != asked {
                notes.push(format!("{name} was limited to {size}"));
            }
        }
    }
    apply_zone_changes(view, c, notes);
    Ok(())
}

fn apply_zone_changes(view: &mut MapView, c: &ViewChanges, notes: &mut Vec<String>) {
    let show_all = c.show_all_zones == Some(true);
    if !show_all
        && c.hide_zones.is_none()
        && c.show_zones.is_none()
        && c.only_zones.is_none()
        && c.unassigned_cells.is_none()
    {
        return;
    }
    if view.grid_zones.is_empty() {
        notes.push("this map holds a single zone, so there are no zones to hide or show".into());
        return;
    }
    let grid = view.grid_zones.clone();
    let mut on_map = |list: &[u32]| -> Vec<u32> {
        let (known, unknown): (Vec<u32>, Vec<u32>) = list.iter().partition(|z| grid.contains(z));
        if !unknown.is_empty() {
            notes.push(format!("not on this map, so ignored: zones {unknown:?} (get_map lists the zones of a map)"));
        }
        known
    };
    let mut visible = if show_all { grid.clone() } else { view.visible_zones() };
    if let Some(only) = &c.only_zones {
        visible = on_map(only);
    }
    if let Some(show) = &c.show_zones {
        for z in on_map(show) {
            if !visible.contains(&z) {
                visible.push(z);
            }
        }
    }
    if let Some(hide) = &c.hide_zones {
        let hide = on_map(hide);
        visible.retain(|z| !hide.contains(z));
    }
    // A view of one zone alone leaves out the cells without a zone; keep
    // it that way unless asked.
    let was_single = view.key.only_zone.is_some();
    view.key.unassigned = c.unassigned_cells.unwrap_or(if show_all {
        true
    } else if c.only_zones.is_some() || was_single {
        false
    } else {
        view.key.unassigned
    });
    // Kept as the list of hidden zones from here on.
    view.key.only_zone = None;
    view.key.hidden = grid.iter().copied().filter(|z| !visible.contains(z)).collect();
    if visible.is_empty() {
        notes.push("every zone of the map is hidden now".into());
    }
}

/// Absolute camera settings: projection and angles first, then what to look
/// at, then an explicit target and distance.
fn apply_camera(view: &mut MapView, scene: &Scene, c: &ViewChanges, fresh: bool, notes: &mut Vec<String>) -> Result<()> {
    let aspect = view.aspect();
    let cam = &mut view.camera;
    match c.projection {
        Some(Projection::TopDown) => {
            cam.ortho = true;
            cam.pitch = FRAC_PI_2;
        }
        Some(Projection::Perspective) if cam.ortho => {
            cam.ortho = false;
            cam.pitch = OrbitCamera::default().pitch;
        }
        _ => {}
    }
    if let Some(yaw) = c.yaw {
        cam.yaw = finite("yaw", yaw)?.to_radians().rem_euclid(TAU);
    }
    if let Some(pitch) = c.pitch {
        if cam.ortho {
            notes.push("pitch does nothing in the top_down projection, which always looks straight down".into());
        } else {
            cam.pitch = finite("pitch", pitch)?.to_radians().clamp(MIN_PITCH, FRAC_PI_2);
        }
    }
    if let Some(fov) = c.fov {
        cam.fov_y = finite("fov", fov)?.clamp(10.0, 100.0).to_radians();
    }

    let frame = |cam: &mut OrbitCamera, min: Vec3, max: Vec3| {
        cam.frame(min, max, aspect);
        if cam.pitch > 1.5 {
            cam.target.y = max.y;
        }
    };
    let whole = |cam: &mut OrbitCamera| frame(cam, scene.min, scene.max);
    if let Some(zone) = c.look_at_zone {
        match zone_bounds(scene, zone) {
            Some((min, max)) => frame(cam, min, max),
            None => bail!("zone {zone} is not part of what this view shows, so it cannot be looked at"),
        }
    } else if let Some(entity) = &c.look_at_entity {
        match find_marker(scene, entity, view.zone) {
            Ok(marker) => {
                cam.target = marker_centre(marker);
                cam.distance = if cam.ortho { 420.0 } else { 300.0 };
            }
            // A warp may lead to a place that cannot be shown as a point.
            Err(e) if fresh => {
                notes.push(format!("{e:#}; showing the whole view instead"));
                whole(cam);
            }
            Err(e) => return Err(e),
        }
    } else if let Some([x, z]) = c.look_at_tile {
        cam.target.x = (finite("look_at_tile", x)? + 0.5) * TILE_SIZE;
        cam.target.z = (finite("look_at_tile", z)? + 0.5) * TILE_SIZE;
        if fresh {
            cam.target.y = scene.center().y;
            cam.distance = 420.0;
        }
    } else if fresh {
        // A zone asked for together with its surroundings is what to look at.
        match view.zone.filter(|_| view.key.only_zone.is_none()).and_then(|z| zone_bounds(scene, z)) {
            Some((min, max)) => frame(cam, min, max),
            None => whole(cam),
        }
    } else if c.fit == Some(true) {
        whole(cam);
    }

    if let Some(target) = c.target {
        for v in target {
            finite("target", v)?;
        }
        cam.target = Vec3::from(target);
    }
    if let Some(distance) = c.distance {
        cam.distance = finite("distance", distance)?.clamp(*DISTANCE_RANGE.start(), *DISTANCE_RANGE.end());
    }
    Ok(())
}

/// Relative moves, applied after the absolute settings: turn, tilt, zoom,
/// then slide.
fn apply_relative(view: &mut MapView, p: &AdjustViewParams, notes: &mut Vec<String>) -> Result<()> {
    let aspect = view.aspect();
    let cam = &mut view.camera;
    if let Some(rotate) = p.rotate {
        cam.yaw = (cam.yaw + finite("rotate", rotate)?.to_radians()).rem_euclid(TAU);
    }
    if let Some(tilt) = p.tilt {
        if cam.ortho {
            notes.push("tilt does nothing in the top_down projection; switch projection to perspective first".into());
        } else {
            cam.pitch = (cam.pitch + finite("tilt", tilt)?.to_radians()).clamp(MIN_PITCH, FRAC_PI_2);
        }
    }
    if let Some(zoom) = p.zoom {
        if !(zoom.is_finite() && zoom > 0.0) {
            bail!("zoom must be greater than 0: 2 comes twice as close, 0.5 moves twice as far away");
        }
        cam.distance = (cam.distance / zoom).clamp(*DISTANCE_RANGE.start(), *DISTANCE_RANGE.end());
    }
    if let Some([right, up]) = p.pan {
        let height = cam.view_height();
        let ahead = Vec3::new(cam.yaw.sin(), 0.0, -cam.yaw.cos());
        // A step up the picture covers more ground the flatter the camera looks.
        let stretch = if cam.ortho { 1.0 } else { 1.0 / cam.pitch.sin().max(0.2) };
        cam.target += cam.right() * (finite("pan", right)? * height * aspect) + ahead * (finite("pan", up)? * height * stretch);
    }
    if let Some([east, south]) = p.move_by {
        cam.target.x += finite("move_by", east)?;
        cam.target.z += finite("move_by", south)?;
    }
    Ok(())
}

// ------------------------------------------------------------- geometry

/// The box around a zone's cells and what stands on them.
fn zone_bounds(scene: &Scene, zone: u32) -> Option<(Vec3, Vec3)> {
    let layout = scene.layout.as_ref()?;
    let span = layout.chunk_span;
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for c in layout.cells.iter().filter(|c| c.zone == Some(zone) && !c.hidden) {
        min = min.min(Vec3::new(c.x as f32 * span, 0.0, c.y as f32 * span));
        max = max.max(Vec3::new((c.x + 1) as f32 * span, 0.0, (c.y + 1) as f32 * span));
    }
    if min.x > max.x {
        return None;
    }
    let (mut low, mut high) = (f32::MAX, f32::MIN);
    for node in &scene.nodes {
        let of_zone = match node.kind {
            NodeKind::Chunk { zone: z, .. } | NodeKind::Building { zone: z, .. } => z == Some(zone),
            _ => false,
        };
        if of_zone {
            let model = &scene.models[node.model];
            low = low.min(node.transform.w_axis.y + model.min.y);
            high = high.max(node.transform.w_axis.y + model.max.y);
        }
    }
    if low > high {
        (low, high) = (scene.min.y, scene.max.y);
    }
    (min.y, max.y) = (low, high);
    Some((min, max))
}

fn find_marker<'a>(scene: &'a Scene, entity: &EntityRef, view_zone: Option<u32>) -> Result<&'a Marker> {
    let of_kind = |m: &&Marker| marker_kind(m.kind) == entity.kind && m.index == entity.index;
    let found: Vec<&Marker> = match entity.zone.or(view_zone) {
        Some(zone) => scene.markers.iter().filter(of_kind).filter(|m| m.zone == zone).collect(),
        None => scene.markers.iter().filter(of_kind).collect(),
    };
    let name = kind_name(entity.kind);
    match found.as_slice() {
        [one] => Ok(one),
        [] => bail!(
            "there is no {name} {} with a position in what this view shows (list_entities lists the entities of a zone)",
            entity.index
        ),
        _ => bail!("several zones of this view have a {name} {}; say which zone", entity.index),
    }
}

fn marker_centre(marker: &Marker) -> Vec3 {
    let (min, size) = marker.bounds();
    min + size * 0.5
}

/// Pixel a point is drawn at, if it lies in the picture.
fn project(camera: &Camera, point: Vec3, width: u32, height: u32) -> Option<[f32; 2]> {
    let clip = camera.view_proj() * point.extend(1.0);
    if clip.w <= 1e-5 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if ndc.x.abs() > 1.0 || ndc.y.abs() > 1.0 || !(0.0..=1.0).contains(&ndc.z) {
        return None;
    }
    Some([(ndc.x * 0.5 + 0.5) * width as f32, (0.5 - ndc.y * 0.5) * height as f32])
}

fn tile(point: Vec3) -> Value {
    json!([(point.x / TILE_SIZE).floor() as i64, (point.z / TILE_SIZE).floor() as i64])
}

fn node_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Chunk { .. } => "terrain",
        NodeKind::Building { .. } => "building",
        NodeKind::Door { .. } => "door",
        NodeKind::Model => "model",
    }
}

/// Adds the map cell, chunk and zone a point lies in.
fn place(scene: &Scene, world: &World, point: Vec3, value: &mut Value) {
    let Some(layout) = &scene.layout else { return };
    if point.x < 0.0 || point.z < 0.0 {
        return;
    }
    let (x, y) = ((point.x / layout.chunk_span) as usize, (point.z / layout.chunk_span) as usize);
    value["cell"] = json!([x, y]);
    if let Some(cell) = layout.cells.iter().find(|c| c.x == x && c.y == y) {
        value["chunk"] = json!(cell.chunk_id);
        if let Some(zone) = cell.zone {
            value["zone"] = json!(zone);
            if let Some(name) = world.location_name(zone) {
                value["zone_name"] = json!(name);
            }
        }
        if cell.hidden {
            value["zone_hidden"] = json!(true);
        }
    }
}

fn marker_value(m: &Marker) -> Value {
    json!({
        "kind": kind_name(marker_kind(m.kind)),
        "zone": m.zone,
        "index": m.index,
        "label": m.label,
        "position": vec3(m.pos),
        "tile": tile(m.pos),
    })
}

// ------------------------------------------------------------- describing

fn degrees(radians: f32) -> f64 {
    round(radians.to_degrees(), 1)
}

/// The view in words and numbers: what is shown, with which settings, from
/// where.
fn describe(view: &MapView, world: &World, scene: &Scene) -> Value {
    let key = &view.key;
    let mut map = json!({ "matrix": key.matrix });
    if let Some(layout) = &scene.layout {
        map["size_in_chunks"] = json!([layout.width, layout.height]);
    }
    if let Some(zone) = view.zone {
        map["zone"] = json!(zone);
        if let Some(name) = world.location_name(zone) {
            map["zone_name"] = json!(name);
        }
    }
    if !view.grid_zones.is_empty() {
        let visible = view.visible_zones();
        let hidden: Vec<u32> = view.grid_zones.iter().copied().filter(|z| !visible.contains(z)).collect();
        map["visible_zones"] = json!(visible);
        map["hidden_zones"] = json!(hidden);
        map["unassigned_cells"] = json!(key.only_zone.is_none() && key.unassigned);
    }

    let o = &view.options;
    let background = match o.background {
        None => "sky".to_string(),
        Some(c) => {
            let [r, g, b] = c.map(|v| (v * 255.0).round() as u8);
            format!("#{r:02x}{g:02x}{b:02x}")
        }
    };
    let options = json!({
        "terrain": key.terrain,
        "buildings": key.buildings,
        "markers": o.markers,
        "npcs": o.marker_kinds[0],
        "warps": o.marker_kinds[1],
        "triggers": o.marker_kinds[2],
        "interactables": o.marker_kinds[3],
        "lighting": o.lighting,
        "textures": o.textures,
        "vertex_colors": o.vertex_colors,
        "animations": key.animations,
        "time": round(view.time, 3),
        "season": SEASONS[key.season as usize & 3],
        "time_of_day": TIMES_OF_DAY[(key.day_part as usize).min(4)],
        "background": background,
    });

    let cam = &view.camera;
    let visible_height = cam.view_height();
    let camera = json!({
        "projection": if cam.ortho { "top_down" } else { "perspective" },
        "yaw": degrees(cam.yaw.rem_euclid(TAU)),
        "pitch": degrees(cam.pitch),
        "distance": round(cam.distance, 1),
        "fov": degrees(cam.fov_y),
        "target": vec3(cam.target),
        "target_tile": tile(cam.target),
        "eye": vec3(cam.eye()),
        // World units the picture spans at the distance of the target.
        "spans_at_target": [round(visible_height * view.aspect(), 0), round(visible_height, 0)],
    });

    let count = |kind: EntityKind| scene.markers.iter().filter(|m| marker_kind(m.kind) == kind).count();
    let mut out = json!({
        "map": map,
        "options": options,
        "camera": camera,
        "picture": { "width": view.width, "height": view.height },
        "scene": {
            "triangles": scene.triangle_count(),
            "placed_models": scene.nodes.len(),
            "npcs": count(EntityKind::Npc),
            "warps": count(EntityKind::Warp),
            "triggers": count(EntityKind::Trigger),
            "interactables": count(EntityKind::Interactable),
            "animated_parts": scene.animations.len(),
            "bounds_min": vec3(scene.min),
            "bounds_max": vec3(scene.max),
        },
    });
    if !scene.warnings.is_empty() {
        let mut warnings: Vec<String> = scene.warnings.iter().take(8).cloned().collect();
        if scene.warnings.len() > warnings.len() {
            warnings.push(format!("and {} more", scene.warnings.len() - warnings.len()));
        }
        out["data_warnings"] = json!(warnings);
    }
    out
}

/// Zones whose centre lies in the picture, with the pixel it is drawn at.
fn zones_in_picture(view: &MapView, world: &World, scene: &Scene) -> Value {
    let camera = view.camera.camera(view.aspect());
    let mut zones: Vec<u32> = match &scene.layout {
        Some(layout) => layout.cells.iter().filter(|c| !c.hidden).filter_map(|c| c.zone).collect(),
        None => Vec::new(),
    };
    zones.sort_unstable();
    zones.dedup();
    let listed: Vec<Value> = zones
        .into_iter()
        .filter_map(|zone| {
            let (min, max) = zone_bounds(scene, zone)?;
            let centre = Vec3::new((min.x + max.x) * 0.5, min.y.max(0.0), (min.z + max.z) * 0.5);
            let pixel = project(&camera, centre, view.width, view.height)?;
            Some(json!({
                "zone": zone,
                "name": world.location_name(zone),
                "pixel": [pixel[0].round() as i64, pixel[1].round() as i64],
                "centre_tile": tile(centre),
            }))
        })
        .collect();
    json!(listed)
}

/// Markers that lie in the picture, with the pixel each is drawn at.
fn visible_markers(view: &MapView, scene: &Scene, notes: &mut Vec<String>) -> Value {
    let (aspect, o) = (view.aspect(), &view.options);
    let camera = view.camera.camera(aspect);
    let mut out = Vec::new();
    let mut left_out = 0;
    for m in scene.markers.iter().filter(|m| o.markers && o.marker_kinds[m.kind.index()]) {
        let centre = marker_centre(m);
        let Some(pixel) = project(&camera, centre, view.width, view.height) else { continue };
        if out.len() >= LISTED_MARKERS {
            left_out += 1;
            continue;
        }
        let ndc = [pixel[0] / view.width as f32 * 2.0 - 1.0, 1.0 - pixel[1] / view.height as f32 * 2.0];
        let (origin, dir) = view.camera.ray(ndc, aspect);
        let mut entry = json!({
            "kind": kind_name(marker_kind(m.kind)),
            "zone": m.zone,
            "index": m.index,
            "label": m.label,
            "pixel": [pixel[0].round() as i64, pixel[1].round() as i64],
            "tile": tile(m.pos),
        });
        // Something drawn clearly in front of the marker covers it.
        let distance = (centre - origin).dot(dir);
        if let Some(hit) = scene.ray_cast(origin, dir).filter(|h| h.distance < distance - 14.0) {
            entry["covered_by"] = json!(node_name(&scene.nodes[hit.node].kind));
        }
        out.push(entry);
    }
    if left_out > 0 {
        notes.push(format!("{left_out} more markers are in the picture than visible_markers lists; zoom in to get them all"));
    }
    json!(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(grid: &[u32], only_zone: Option<u32>) -> MapView {
        MapView {
            key: SceneKey {
                matrix: 0,
                only_zone,
                season: 0,
                day_part: 1,
                terrain: true,
                buildings: true,
                animations: true,
                hidden: Vec::new(),
                unassigned: true,
            },
            zone: only_zone,
            grid_zones: grid.to_vec(),
            options: RenderOptions::default(),
            time: 0.0,
            camera: OrbitCamera::default(),
            width: 960,
            height: 640,
        }
    }

    fn zones(view: &mut MapView, changes: ViewChanges) -> Vec<String> {
        let mut notes = Vec::new();
        apply_zone_changes(view, &changes, &mut notes);
        notes
    }

    #[test]
    fn zones_are_hidden_shown_and_singled_out() {
        let mut v = view(&[1, 2, 3, 4], None);
        let notes = zones(&mut v, ViewChanges { hide_zones: Some(vec![2, 4, 99]), ..Default::default() });
        assert_eq!(v.key.hidden, [2, 4]);
        assert!(v.key.unassigned, "hiding zones leaves the cells without a zone alone");
        assert_eq!(notes.len(), 1, "zone 99 is not on the map: {notes:?}");

        zones(&mut v, ViewChanges { show_zones: Some(vec![2]), ..Default::default() });
        assert_eq!(v.key.hidden, [4]);

        zones(&mut v, ViewChanges { only_zones: Some(vec![3]), ..Default::default() });
        assert_eq!(v.visible_zones(), [3]);
        assert!(!v.key.unassigned, "only_zones leaves out the cells without a zone");

        zones(&mut v, ViewChanges { show_all_zones: Some(true), ..Default::default() });
        assert!(v.key.hidden.is_empty() && v.key.unassigned);

        // The cells without a zone can be switched on their own, and an
        // explicit wish wins over what only_zones implies.
        zones(&mut v, ViewChanges { unassigned_cells: Some(false), ..Default::default() });
        assert!(v.key.hidden.is_empty() && !v.key.unassigned);
        zones(&mut v, ViewChanges { only_zones: Some(vec![1]), unassigned_cells: Some(true), ..Default::default() });
        assert_eq!(v.visible_zones(), [1]);
        assert!(v.key.unassigned);
    }

    #[test]
    fn a_view_of_one_zone_grows_by_showing_another() {
        let mut v = view(&[1, 2, 3], Some(2));
        assert_eq!(v.visible_zones(), [2]);
        zones(&mut v, ViewChanges { show_zones: Some(vec![3]), ..Default::default() });
        assert_eq!(v.key.only_zone, None);
        assert_eq!(v.key.hidden, [1]);
        assert!(!v.key.unassigned, "a view of one zone had none of the cells without a zone");
    }

    #[test]
    fn a_map_of_one_zone_has_nothing_to_hide() {
        let mut v = view(&[], None);
        let notes = zones(&mut v, ViewChanges { hide_zones: Some(vec![5]), ..Default::default() });
        assert!(v.key.hidden.is_empty());
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn switches_keep_their_value_unless_named() {
        let mut v = view(&[], None);
        let mut notes = Vec::new();
        let changes = ViewChanges { markers: Some(true), npcs: Some(false), buildings: Some(false), ..Default::default() };
        apply_settings(&mut v, &changes, &mut notes).unwrap();
        assert_eq!(v.options.marker_kinds, [false, true, true, true]);
        assert!(!v.key.buildings && v.key.terrain && v.options.lighting);
        apply_settings(&mut v, &ViewChanges { lighting: Some(false), ..Default::default() }, &mut notes).unwrap();
        assert_eq!(v.options.marker_kinds, [false, true, true, true]);
        assert!(!v.key.buildings && !v.options.lighting);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn relative_moves_follow_the_camera() {
        let mut v = view(&[], None);
        let mut notes = Vec::new();
        v.camera.ortho = true;
        v.camera.pitch = FRAC_PI_2;
        v.camera.distance = 1000.0;
        let height = v.camera.view_height();
        // Facing north: right is east (+X), up the picture is north (-Z).
        let pan = AdjustViewParams { pan: Some([0.5, 0.25]), ..Default::default() };
        apply_relative(&mut v, &pan, &mut notes).unwrap();
        assert!((v.camera.target.x - 0.5 * height * 1.5).abs() < 0.01);
        assert!((v.camera.target.z + 0.25 * height).abs() < 0.01);

        // Turned to face east, "up the picture" is east.
        v.camera.target = Vec3::ZERO;
        let turn = AdjustViewParams { rotate: Some(90.0), pan: Some([0.0, 1.0]), zoom: Some(2.0), ..Default::default() };
        apply_relative(&mut v, &turn, &mut notes).unwrap();
        assert!((v.camera.distance - 500.0).abs() < 0.01);
        assert!((v.camera.target.x - height / 2.0).abs() < 0.01, "{:?}", v.camera.target);
        assert!(v.camera.target.z.abs() < 0.01);

        let moved = AdjustViewParams { move_by: Some([16.0, 32.0]), ..Default::default() };
        v.camera.target = Vec3::ZERO;
        apply_relative(&mut v, &moved, &mut notes).unwrap();
        assert_eq!(v.camera.target, Vec3::new(16.0, 0.0, 32.0));

        assert!(apply_relative(&mut v, &AdjustViewParams { zoom: Some(0.0), ..Default::default() }, &mut notes).is_err());
    }

    #[test]
    fn a_point_projects_to_the_pixel_whose_ray_passes_through_it() {
        let mut v = view(&[], None);
        v.camera.target = Vec3::new(300.0, 20.0, 400.0);
        v.camera.yaw = 0.7;
        let point = Vec3::new(340.0, 0.0, 380.0);
        let camera = v.camera.camera(v.aspect());
        let pixel = project(&camera, point, v.width, v.height).expect("in the picture");
        let ndc = [pixel[0] / v.width as f32 * 2.0 - 1.0, 1.0 - pixel[1] / v.height as f32 * 2.0];
        let (origin, dir) = v.camera.ray(ndc, v.aspect());
        let along = (point - origin).dot(dir);
        assert!((origin + dir * along - point).length() < 0.05);
        // Behind the camera there is no pixel.
        assert!(project(&camera, v.camera.eye() - v.camera.forward() * 50.0, v.width, v.height).is_none());
    }
}
