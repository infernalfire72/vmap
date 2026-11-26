//! Tools that answer with data: the game, its maps, zones, entities, chunks,
//! buildings and texts.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use vmap_formats::gen5::entities::Placement;
use vmap_formats::gen5::{Arc, BuildingBundle, MapChunk, TextFile, ZoneEntities, TILE_SIZE};
use vmap_formats::nitro::Mdl0;
use vmap_world::{MarkerKind, World};

use crate::params::{
    BuildingsParams, ChunkParams, EntityKind, ListEntitiesParams, ListMapsParams, ListZonesParams, MapParams,
    OpenGameParams, ReadTextParams, ZoneParams,
};
use crate::reply::Reply;
use crate::state::State;

pub fn marker_kind(kind: MarkerKind) -> EntityKind {
    match kind {
        MarkerKind::Npc => EntityKind::Npc,
        MarkerKind::Warp => EntityKind::Warp,
        MarkerKind::Trigger => EntityKind::Trigger,
        MarkerKind::Furniture => EntityKind::Interactable,
    }
}

pub fn kind_name(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Npc => "npc",
        EntityKind::Warp => "warp",
        EntityKind::Trigger => "trigger",
        EntityKind::Interactable => "interactable",
    }
}

/// A page of a list: the entries from `offset` on, at most `limit` of them.
fn page<T>(items: Vec<T>, offset: Option<usize>, limit: Option<usize>, default_limit: usize) -> (Vec<T>, Value) {
    let total = items.len();
    let offset = offset.unwrap_or(0).min(total);
    let limit = limit.unwrap_or(default_limit).max(1);
    let shown: Vec<T> = items.into_iter().skip(offset).take(limit).collect();
    let mut info = json!({ "total": total, "offset": offset, "returned": shown.len() });
    if offset + shown.len() < total {
        info["next_offset"] = json!(offset + shown.len());
    }
    (shown, info)
}

fn matches(text: &str, search: &Option<String>) -> bool {
    match search {
        Some(s) => text.to_lowercase().contains(&s.trim().to_lowercase()),
        None => true,
    }
}

fn zone_brief(world: &World, zone: u32) -> Value {
    json!({ "zone": zone, "name": world.location_name(zone) })
}

// ------------------------------------------------------------------ the game

fn game_value(world: &World, path: &Path) -> Value {
    let fs = &world.fs;
    let source = match fs.kind() {
        vmap_fs::SourceKind::CtrmapProject => "CTRMap project",
        vmap_fs::SourceKind::ExtractedFolder => "extracted ROM folder",
        vmap_fs::SourceKind::RomImage => "ROM image",
    };
    let changed: usize = world.archives().iter().map(|a| a.modified).sum();
    json!({
        "game": world.game.name(),
        "game_code": fs.header().game_code,
        "source": source,
        "path": path.display().to_string(),
        "zones": world.zones.len(),
        "areas": world.areas.len(),
        "map_matrices": world.matrix_count,
        "files_changed_by_project": changed,
        "units": "World units: one tile is 16, one map chunk is 512 (32 tiles). X grows eastwards, Z southwards, Y upwards. \
                  A tile [x, z] counts from the north-west corner of the map matrix.",
    })
}

pub fn open_game(state: &mut State, p: OpenGameParams) -> Result<Reply> {
    let path = Path::new(p.path.trim()).to_path_buf();
    if !path.exists() {
        bail!("{} does not exist", path.display());
    }
    let game = state.open(&path)?;
    Ok(Reply::json(&game_value(&game.world, &game.path)))
}

pub fn reload_game(state: &mut State) -> Result<Reply> {
    let path = state.game()?.path.clone();
    // Opened anew, so everything saved since is seen. The map view is kept;
    // what was built for it is not.
    let view = state.game()?.view.take();
    let game = match state.open(&path) {
        Ok(_) => state.game()?,
        Err(e) => return Err(e.context("the game on disk could not be opened again; the previous state is still in use")),
    };
    game.view = view;
    let mut out = game_value(&game.world, &game.path);
    out["reloaded"] = json!(true);
    Ok(Reply::json(&out))
}

pub fn game_info(state: &mut State) -> Result<Reply> {
    let game = state.game()?;
    Ok(Reply::json(&game_value(&game.world, &game.path)))
}

// ------------------------------------------------------------ maps and zones

pub fn list_maps(state: &mut State, p: ListMapsParams) -> Result<Reply> {
    let world = state.world()?;
    let maps: Vec<Value> = world
        .matrices()
        .into_iter()
        .filter(|m| {
            p.search.is_none()
                || matches(&m.name, &p.search)
                || m.zones.iter().any(|z| matches(world.location_name(*z).unwrap_or(""), &p.search))
        })
        .map(|m| {
            let zones: Vec<Value> = m.zones.iter().take(12).map(|z| zone_brief(world, *z)).collect();
            let mut v = json!({
                "matrix": m.id,
                "name": m.name,
                "size_in_chunks": [m.width, m.height],
                "chunks": m.cells,
                "zone_count": m.zones.len(),
                "zones": zones,
            });
            if m.zones.len() > 12 {
                v["more_zones"] = json!(format!("{} more; get_map lists them all", m.zones.len() - 12));
            }
            v
        })
        .collect();
    let (shown, mut info) = page(maps, p.offset, p.limit, 60);
    info["maps"] = json!(shown);
    Ok(Reply::json(&info))
}

pub fn list_zones(state: &mut State, p: ListZonesParams) -> Result<Reply> {
    let world = state.world()?;
    let zones: Vec<Value> = world
        .zones
        .iter()
        .enumerate()
        .filter(|(i, z)| {
            p.matrix.is_none_or(|m| z.matrix_id as usize == m)
                && matches(world.location_name(*i as u32).unwrap_or(""), &p.search)
        })
        .map(|(i, z)| json!({ "zone": i, "name": world.location_name(i as u32), "matrix": z.matrix_id, "area": z.area_id }))
        .collect();
    let (shown, mut info) = page(zones, p.offset, p.limit, 100);
    info["zones"] = json!(shown);
    Ok(Reply::json(&info))
}

pub fn get_zone(state: &mut State, p: ZoneParams) -> Result<Reply> {
    let world = state.world()?;
    let zone = p.zone;
    let h = world
        .zones
        .get(zone as usize)
        .ok_or_else(|| anyhow!("there is no zone {zone}; the game has zones 0 to {}", world.zones.len().saturating_sub(1)))?;
    let matrix_id = h.matrix_id as usize;
    let mut out = json!({
        "zone": zone,
        "name": world.location_name(zone),
        "matrix": matrix_id,
    });

    // Where the zone lies on its map.
    if let Ok(matrix) = world.matrix(matrix_id) {
        let mut on_map = json!({ "size_in_chunks": [matrix.width, matrix.height] });
        if matrix.zones.is_some() {
            let cells: Vec<(usize, usize)> = (0..matrix.height)
                .flat_map(|y| (0..matrix.width).map(move |x| (x, y)))
                .filter(|&(x, y)| matrix.zone(x, y) == Some(zone))
                .collect();
            let others: Vec<u32> = matrix.zone_ids().into_iter().filter(|z| *z != zone).collect();
            on_map["shared_with_zones"] = json!(others);
            if let (Some(x0), Some(x1), Some(y0), Some(y1)) = (
                cells.iter().map(|c| c.0).min(),
                cells.iter().map(|c| c.0).max(),
                cells.iter().map(|c| c.1).min(),
                cells.iter().map(|c| c.1).max(),
            ) {
                on_map["cells_owned"] = json!(cells.len());
                on_map["cells_from"] = json!([x0, y0]);
                on_map["cells_to"] = json!([x1, y1]);
                on_map["tiles_from"] = json!([x0 * 32, y0 * 32]);
                on_map["tiles_to"] = json!([(x1 + 1) * 32 - 1, (y1 + 1) * 32 - 1]);
            } else {
                on_map["cells_owned"] = json!(0);
            }
        } else {
            on_map["zone_is_whole_map"] = json!(true);
            on_map["tiles_to"] = json!([matrix.width * 32 - 1, matrix.height * 32 - 1]);
        }
        out["on_map"] = on_map;
    }

    if let Some(area) = world.areas.get(h.area_id as usize) {
        let bundle = if world.game.is_bw2() {
            area.buildings_id as usize
        } else {
            vmap_formats::gen5::bw1_buildings_id(h.area_id as usize)
        };
        out["area"] = json!({
            "area": h.area_id,
            "exterior": area.is_exterior,
            "texture_set": area.textures_id,
            "building_bundle": bundle,
            "light_setup": area.light_index,
            "changes_with_season": world.seasonal_area(zone, 1) != world.seasonal_area(zone, 0),
        });
    } else {
        out["area"] = json!({ "area": h.area_id, "problem": "the area table has no such entry" });
    }

    match world.zone_entities(zone) {
        Some(Ok(e)) => {
            out["entities"] = json!({
                "file": h.entities_id,
                "npcs": e.npcs.len(),
                "warps": e.warps.len(),
                "triggers": e.triggers.len(),
                "interactables": e.furniture.len(),
            })
        }
        Some(Err(e)) => out["entities"] = json!({ "file": h.entities_id, "problem": format!("{e:#}") }),
        None => out["entities"] = json!({ "file": h.entities_id, "problem": "the file is missing" }),
    }

    out["header"] = json!({
        "map_type": h.map_type,
        "parent_zone": h.parent_zone_id,
        "location_name_id": h.location_name,
        "script_file": h.scripts_id,
        "init_script_file": h.init_scripts_id,
        "text_file": h.text_file_id,
        "encounter_file": h.encounter_id,
        "music": { "spring": h.bgm_spring, "summer": h.bgm_summer, "autumn": h.bgm_autumn, "winter": h.bgm_winter },
        "weather": h.weather,
        "camera": h.camera_index,
        "battle_background": h.battle_bg,
        "map_transition": h.map_transition,
        "name_icon": h.name_icon,
        "can_cycle": h.can_cycle,
        "can_run": h.can_run,
        "can_use_escape_rope": h.can_escape_rope,
        "can_fly": h.can_fly,
        "fly_position": h.fly_pos,
        "matrix_camera_boundary": h.matrix_camera_boundary,
    });
    Ok(Reply::json(&out))
}

pub fn get_map(state: &mut State, p: MapParams) -> Result<Reply> {
    let world = state.world()?;
    if p.matrix >= world.matrix_count {
        bail!("there is no map matrix {}; the game has matrices 0 to {}", p.matrix, world.matrix_count.saturating_sub(1));
    }
    let matrix = world.matrix(p.matrix)?;
    let grid_zones = matrix.zone_ids();
    let users = world.zones_of_matrix(p.matrix);

    let zones: Vec<Value> = if matrix.zones.is_some() {
        grid_zones
            .iter()
            .map(|&z| {
                let cells: Vec<(usize, usize)> = (0..matrix.height)
                    .flat_map(|y| (0..matrix.width).map(move |x| (x, y)))
                    .filter(|&(x, y)| matrix.zone(x, y) == Some(z))
                    .collect();
                let (x0, x1) = (cells.iter().map(|c| c.0).min().unwrap_or(0), cells.iter().map(|c| c.0).max().unwrap_or(0));
                let (y0, y1) = (cells.iter().map(|c| c.1).min().unwrap_or(0), cells.iter().map(|c| c.1).max().unwrap_or(0));
                json!({
                    "zone": z,
                    "name": world.location_name(z),
                    "cells": cells.len(),
                    "cells_from": [x0, y0],
                    "cells_to": [x1, y1],
                })
            })
            .collect()
    } else {
        users.iter().map(|&z| zone_brief(world, z)).collect()
    };

    // The grid as text: one row of the map per line, west to east.
    let width = matrix
        .chunks
        .iter()
        .flatten()
        .map(|c| c.to_string().len())
        .chain(grid_zones.iter().map(|z| z.to_string().len()))
        .max()
        .unwrap_or(1);
    let row = |y: usize, cell: &dyn Fn(usize, usize) -> Option<u32>| -> String {
        (0..matrix.width)
            .map(|x| match cell(x, y) {
                Some(v) => format!("{v:>width$}"),
                None => format!("{:>width$}", "."),
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let chunk_rows: Vec<String> = (0..matrix.height).map(|y| row(y, &|x, y| matrix.chunk(x, y))).collect();

    let mut out = json!({
        "matrix": p.matrix,
        "size_in_chunks": [matrix.width, matrix.height],
        "size_in_tiles": [matrix.width * 32, matrix.height * 32],
        "chunks": matrix.cell_count(),
        "has_zone_grid": matrix.zones.is_some(),
        "zones": zones,
        "grid_legend": "One line per row of cells from north to south, cells from west to east; a dot is an empty cell. \
                        Cell [x, y] covers tiles x*32 to x*32+31 and y*32 to y*32+31.",
        "chunk_grid": chunk_rows,
    });
    if matrix.zones.is_some() {
        let zone_rows: Vec<String> = (0..matrix.height).map(|y| row(y, &|x, y| matrix.zone(x, y))).collect();
        out["zone_grid"] = json!(zone_rows);
        let not_on_grid: Vec<u32> = users.iter().copied().filter(|z| !grid_zones.contains(z)).collect();
        if !not_on_grid.is_empty() {
            out["zones_using_the_map_without_cells"] = json!(not_on_grid);
        }
    }
    Ok(Reply::json(&out))
}

// ------------------------------------------------------------------ entities

fn placement(p: &Placement) -> Value {
    match *p {
        Placement::Grid { x, y, z } => json!({
            "position": [x, y, z],
            "tile": [(x / TILE_SIZE).floor() as i64, (z / TILE_SIZE).floor() as i64],
        }),
        Placement::Rail { line, front, side } => json!({ "on_rail": { "line": line, "front": front, "side": side } }),
    }
}

fn with_placement(mut value: Value, p: &Placement) -> Value {
    if let (Value::Object(v), Value::Object(place)) = (&mut value, placement(p)) {
        v.extend(place);
    }
    value
}

/// One entity of a zone with everything its record holds.
pub fn entity_value(world: &World, e: &ZoneEntities, kind: EntityKind, index: usize) -> Option<Value> {
    Some(match kind {
        EntityKind::Npc => {
            let n = e.npcs.get(index)?;
            with_placement(
                json!({
                    "index": index,
                    "id": n.uid,
                    "model": n.obj_code,
                    "movement": n.move_code,
                    "event_type": n.event_type,
                    "spawn_flag": n.spawn_flag,
                    "script": n.script,
                    "facing": n.face_direction,
                    "parameters": n.params,
                    "movement_area": [n.area_width, n.area_height],
                }),
                &n.placement,
            )
        }
        EntityKind::Warp => {
            let w = e.warps.get(index)?;
            with_placement(
                json!({
                    "index": index,
                    "to_zone": w.target_zone,
                    "to_zone_name": world.location_name(w.target_zone as u32),
                    "to_warp": w.target_warp,
                    "facing": w.face_direction,
                    "transition": w.transition,
                    "size_in_tiles": [w.width, w.height],
                }),
                &w.placement,
            )
        }
        EntityKind::Trigger => {
            let t = e.triggers.get(index)?;
            with_placement(
                json!({
                    "index": index,
                    "script": t.script,
                    "work_id": t.work_id,
                    "work_value": t.work_value,
                    "type": t.kind,
                    "size_in_tiles": [t.width, t.height],
                }),
                &t.placement,
            )
        }
        EntityKind::Interactable => {
            let f = e.furniture.get(index)?;
            with_placement(
                json!({
                    "index": index,
                    "script": f.script,
                    "condition": f.condition,
                    "interactibility": f.interactibility,
                }),
                &f.placement,
            )
        }
    })
}

fn zone_entities_value(world: &World, zone: u32, kinds: &[EntityKind]) -> Value {
    let mut out = zone_brief(world, zone);
    let e = match world.zone_entities(zone) {
        Some(Ok(e)) => e,
        Some(Err(e)) => {
            out["problem"] = json!(format!("the entity file could not be read: {e:#}"));
            return out;
        }
        None => {
            out["problem"] = json!("the zone has no entity file");
            return out;
        }
    };
    for (kind, name, count) in [
        (EntityKind::Npc, "npcs", e.npcs.len()),
        (EntityKind::Warp, "warps", e.warps.len()),
        (EntityKind::Trigger, "triggers", e.triggers.len()),
        (EntityKind::Interactable, "interactables", e.furniture.len()),
    ] {
        if kinds.contains(&kind) {
            let list: Vec<Value> = (0..count).filter_map(|i| entity_value(world, &e, kind, i)).collect();
            out[name] = json!(list);
        }
    }
    out
}

pub fn list_entities(state: &mut State, p: ListEntitiesParams) -> Result<Reply> {
    let world = state.world()?;
    let all = [EntityKind::Npc, EntityKind::Warp, EntityKind::Trigger, EntityKind::Interactable];
    let kinds = p.kinds.as_deref().filter(|k| !k.is_empty()).unwrap_or(&all);
    let note = "position is in world units, tile is [x, z] counted from the map's north-west corner. \
                The index of an entity is what look_at_entity, follow_warp and the markers in pictures refer to.";
    match (p.zone, p.matrix) {
        (Some(zone), _) => {
            if zone as usize >= world.zones.len() {
                bail!("there is no zone {zone}; the game has zones 0 to {}", world.zones.len().saturating_sub(1));
            }
            let mut out = zone_entities_value(world, zone, kinds);
            out["note"] = json!(note);
            Ok(Reply::json(&out))
        }
        (None, Some(matrix)) => {
            let info = world.matrix_info(matrix).ok_or_else(|| anyhow!("there is no map matrix {matrix}"))?;
            let zones: Vec<Value> = info.zones.iter().map(|&z| zone_entities_value(world, z, kinds)).collect();
            Ok(Reply::json(&json!({ "matrix": matrix, "note": note, "zones": zones })))
        }
        (None, None) => bail!("give a zone, or a matrix to get the entities of all its zones"),
    }
}

// ------------------------------------------------------- chunks and buildings

pub fn get_chunk(state: &mut State, p: ChunkParams) -> Result<Reply> {
    let world = state.world()?;
    let id = p.chunk;
    let data = world.fs.arc_file(Arc::MapChunks, id).ok_or_else(|| {
        anyhow!("there is no map chunk {id}; the game has chunks 0 to {}", world.fs.arc_len(Arc::MapChunks).saturating_sub(1))
    })?;
    let chunk = MapChunk::parse(&data).with_context(|| format!("reading map chunk {id}"))?;
    let mut out = json!({
        "chunk": id,
        "container": chunk.kind,
        "bytes": data.len(),
        "changed_by_project": world.fs.is_overridden(Arc::MapChunks, id),
        "archive": Arc::MapChunks.id(world.game),
    });
    match Mdl0::from_bmd(chunk.model) {
        Ok(mdl) => {
            let models: Vec<Value> = mdl
                .models
                .iter()
                .map(|m| {
                    json!({
                        "name": m.name,
                        "triangles": m.triangle_count,
                        "quads": m.quad_count,
                        "materials": m.materials.len(),
                    })
                })
                .collect();
            out["model"] = json!({ "bytes": chunk.model.len(), "models": models });
        }
        Err(e) => out["model"] = json!({ "bytes": chunk.model.len(), "problem": format!("{e}") }),
    }
    if let Some(t) = &chunk.terrain {
        out["terrain_grid_in_tiles"] = json!([t.width, t.height]);
    }
    let buildings: Vec<Value> = chunk
        .buildings
        .iter()
        .enumerate()
        .map(|(i, b)| {
            json!({
                "index": i,
                "building_model": b.model_uid,
                // Relative to the chunk's centre; here Z grows northwards.
                "offset_from_chunk_centre": [b.position.x, b.position.y, b.position.z],
                "rotation_degrees": b.rotation_deg,
            })
        })
        .collect();
    out["buildings"] = json!(buildings);

    // Where the chunk is used.
    let mut used = Vec::new();
    for m in 0..world.matrix_count {
        let Ok(matrix) = world.matrix(m) else { continue };
        for y in 0..matrix.height {
            for x in 0..matrix.width {
                if matrix.chunk(x, y) == Some(id as u32) && used.len() < 24 {
                    let mut at = json!({ "matrix": m, "cell": [x, y] });
                    if matrix.zones.is_some() {
                        at["zone"] = json!(matrix.zone(x, y));
                    } else {
                        at["zones_of_matrix"] = json!(world.zones_of_matrix(m));
                    }
                    used.push(at);
                }
            }
        }
    }
    out["used_at"] = json!(used);
    Ok(Reply::json(&out))
}

pub fn list_buildings(state: &mut State, p: BuildingsParams) -> Result<Reply> {
    let world = state.world()?;
    let interior = p.interior.unwrap_or(false);
    let arc = if interior { Arc::BuildingsInterior } else { Arc::BuildingsExterior };
    let data = world.fs.arc_file(arc, p.bundle).ok_or_else(|| {
        anyhow!(
            "there is no {} building bundle {}; there are bundles 0 to {}",
            if interior { "interior" } else { "exterior" },
            p.bundle,
            world.fs.arc_len(arc).saturating_sub(1)
        )
    })?;
    let bundle = BuildingBundle::parse(&data).with_context(|| format!("reading building bundle {}", p.bundle))?;
    // Where each file of the bundle starts, to tell which one a model is.
    let starts: Vec<usize> = vmap_formats::gf::GfContainer::parse(&data)
        .map(|c| c.files.iter().map(|r| r.start).collect())
        .unwrap_or_default();
    let base = data.as_ptr() as usize;
    let buildings: Vec<Value> = bundle
        .buildings
        .iter()
        .map(|r| {
            let offset = (r.model.as_ptr() as usize).wrapping_sub(base);
            let sub_file = starts.iter().position(|s| *s == offset);
            let model = Mdl0::from_bmd(r.model).ok();
            let first = model.as_ref().and_then(|m| m.models.first());
            let animations: Vec<String> =
                r.animations.iter().map(|a| String::from_utf8_lossy(&a[..4.min(a.len())]).into_owned()).collect();
            json!({
                "building_model": r.uid,
                // The file of the bundle that holds the model: what to give
                // render_model as sub_file.
                "sub_file": sub_file,
                "name": first.map(|m| m.name.clone()),
                "triangles": first.map(|m| m.triangle_count),
                "bytes": r.model.len(),
                "door_model": r.door_uid,
                "animations": animations,
                "animations_play": match r.anim_control {
                    0 => "none",
                    1 => "always, also on maps",
                    2 => "on demand (a door opening); only shown by render_model",
                    _ => "unknown mode",
                },
            })
        })
        .collect();
    Ok(Reply::json(&json!({
        "bundle": p.bundle,
        "interior": interior,
        "archive": arc.id(world.game),
        "changed_by_project": world.fs.is_overridden(arc, p.bundle),
        "hint": "building_model is the number map chunks place a building by. \
                 render_model with this archive, the bundle as file and a building's sub_file shows it.",
        "buildings": buildings,
    })))
}

// ---------------------------------------------------------------------- text

pub fn read_text(state: &mut State, p: ReadTextParams) -> Result<Reply> {
    let world = state.world()?;
    let script = p.script.unwrap_or(false);
    let arc = if script { Arc::ScriptText } else { Arc::SystemText };
    let data = world.fs.arc_file(arc, p.file).ok_or_else(|| {
        anyhow!(
            "there is no {} text file {}; there are files 0 to {}",
            if script { "script" } else { "system" },
            p.file,
            world.fs.arc_len(arc).saturating_sub(1)
        )
    })?;
    let text = TextFile::parse(&data).with_context(|| format!("reading text file {}", p.file))?;
    let lines = text.lines();
    let start = p.start.unwrap_or(0).min(lines.len());
    let count = p.count.unwrap_or(200).max(1);
    let shown: Vec<String> = lines
        .iter()
        .enumerate()
        .skip(start)
        .take(count)
        .map(|(i, line)| format!("{i}: {line}"))
        .collect();
    let mut out = json!({
        "file": p.file,
        "kind": if script { "script text" } else { "system text" },
        "changed_by_project": world.fs.is_overridden(arc, p.file),
        "lines_total": lines.len(),
        "first_line": start,
        "lines": shown,
    });
    if start + count < lines.len() {
        out["next_start"] = json!(start + count);
    }
    Ok(Reply::json(&out))
}
