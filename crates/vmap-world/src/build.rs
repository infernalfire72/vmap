//! Turns parsed game data into [`Scene`]s: single models for the asset viewer
//! and whole stitched map matrices for the map viewer.

use std::collections::HashMap;

use glam::{Mat4, Vec3};
use vmap_formats::gen5::{
    bw1_buildings_id, parse_area_patterns, Arc, AreaPattern, BuildingBundle, BuildingResource, LightEntry, MapChunk,
    ZoneEntities,
};
use vmap_formats::nitro::anim::{
    parse_joint, parse_tex_pattern, parse_tex_srt, JointAnimation, TexPatternAnimation, TexSrtAnimation,
};
use vmap_formats::nitro::{evaluate, Material, Mdl0, Model, NitroFile, Tex0};

use crate::scene::*;
use crate::World;

/// Incrementally assembles a [`Scene`], sharing textures and models.
#[derive(Default)]
pub struct SceneBuilder {
    pub scene: Scene,
    /// (texture set key, texture name, palette name) to scene texture index.
    textures: HashMap<(u64, String, String), Option<usize>>,
    models: HashMap<String, Option<usize>>,
    /// Material index in the source model to scene material, for the model
    /// added last.
    last_materials: Vec<(usize, usize)>,
}

impl SceneBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn finish(mut self) -> Scene {
        self.scene.update_bounds();
        self.scene
    }

    pub fn warn(&mut self, msg: String) {
        if self.scene.warnings.len() < 200 && !self.scene.warnings.contains(&msg) {
            self.scene.warnings.push(msg);
        }
    }

    fn texture(&mut self, set_key: u64, tex: &Tex0, name: &str, palette: Option<&str>) -> Option<usize> {
        let key = (set_key, name.to_string(), palette.unwrap_or("").to_string());
        if let Some(&t) = self.textures.get(&key) {
            return t;
        }
        let idx = tex.decode_by_name(name, palette).map(|image| {
            self.scene.textures.push(SceneTexture { name: name.to_string(), image });
            self.scene.textures.len() - 1
        });
        self.textures.insert(key, idx);
        idx
    }

    fn material(&mut self, m: &Material, sets: &[(u64, &Tex0)]) -> usize {
        let mut texture = None;
        let mut tex_translucent = false;
        if let Some(name) = &m.texture {
            for &(key, set) in sets {
                if let Some(ti) = set.find_texture(name) {
                    texture = self.texture(key, set, name, m.palette.as_deref());
                    tex_translucent = set.textures[ti].format.is_translucent();
                    break;
                }
            }
        }
        if tex_translucent {
            // Only blend if the texture really has in-between alpha.
            tex_translucent = texture.is_some_and(|t| self.scene.textures[t].image.has_partial_alpha());
        }
        let alpha = m.alpha();
        let wrap = |repeat: bool, flip: bool| match (repeat, flip) {
            (true, true) => Wrap::Mirror,
            (true, false) => Wrap::Repeat,
            _ => Wrap::Clamp,
        };
        let cull = match (m.draw_front(), m.draw_back()) {
            (true, true) => Cull::None,
            (true, false) => Cull::Back,
            (false, true) => Cull::Front,
            (false, false) => Cull::Both,
        };
        self.scene.materials.push(SceneMaterial {
            name: m.name.clone(),
            texture,
            diffuse: m.diffuse,
            ambient: m.ambient,
            specular: m.specular,
            emission: m.emission,
            alpha: if alpha == 0 { 1.0 } else { alpha as f32 / 31.0 },
            lights: m.lights(),
            cull,
            wrap_s: wrap(m.repeat_s(), m.flip_s()),
            wrap_t: wrap(m.repeat_t(), m.flip_t()),
            tex_matrix: tex_matrix(
                [m.tex_scale.x, m.tex_scale.y],
                m.tex_rot_sin,
                m.tex_rot_cos,
                [m.tex_translation.x, m.tex_translation.y],
            ),
            translucent: (alpha != 0 && alpha < 31) || tex_translucent,
        });
        self.scene.materials.len() - 1
    }

    /// Adds one model, resolving textures against `sets` in order.
    pub fn add_model(&mut self, model: &Model, sets: &[(u64, &Tex0)], pose: Option<&[Mat4]>) -> usize {
        let geo = evaluate(model, pose);
        let mut batches = Vec::with_capacity(geo.batches.len());
        self.last_materials.clear();
        for b in geo.batches {
            if b.vertices.is_empty() {
                continue;
            }
            let material = match model.materials.get(b.material) {
                Some(m) => self.material(m, sets),
                None => self.material(&fallback_material(), sets),
            };
            self.last_materials.push((b.material, material));
            batches.push(SceneBatch { material, vertices: b.vertices });
        }
        self.scene.models.push(SceneModel { name: model.name.clone(), batches, min: geo.min, max: geo.max });
        self.scene.models.len() - 1
    }

    /// Scene materials of the model added last whose source material
    /// satisfies `pick`.
    fn last_materials_where(&self, model: &Model, pick: impl Fn(&Material) -> bool) -> Vec<usize> {
        self.last_materials
            .iter()
            .filter(|(source, _)| model.materials.get(*source).is_some_and(&pick))
            .map(|&(_, scene)| scene)
            .collect()
    }

    /// Applies texture placement animations to the model added last. Tracks
    /// address materials by name.
    pub fn animate_tex_srt(&mut self, model: &Model, animations: &[TexSrtAnimation]) {
        for anim in animations {
            for track in &anim.tracks {
                for material in self.last_materials_where(model, |m| m.name == track.material) {
                    if self.scene.animations.tex_srt.iter().any(|a| a.material == material) {
                        continue;
                    }
                    self.scene.animations.tex_srt.push(TexSrtAnim {
                        material,
                        frames: anim.frame_count as f32,
                        track: track.clone(),
                    });
                }
            }
        }
    }

    /// Applies texture swap animations to the model added last. Tracks
    /// address materials by name and take their textures from `sets`.
    pub fn animate_tex_pattern(&mut self, model: &Model, animations: &[TexPatternAnimation], sets: &[(u64, &Tex0)]) {
        for anim in animations {
            for track in &anim.tracks {
                let targets = self.last_materials_where(model, |m| m.name == track.material);
                if targets.is_empty() {
                    continue;
                }
                let mut keys = Vec::with_capacity(track.keys.len());
                for k in &track.keys {
                    let found = sets.iter().find(|(_, set)| set.find_texture(&k.texture).is_some());
                    if let Some(&(key, set)) = found {
                        if let Some(t) = self.texture(key, set, &k.texture, Some(&k.palette)) {
                            keys.push((k.frame as f32, t));
                        }
                    }
                }
                self.push_pattern(&targets, anim.frame_count as f32, keys);
            }
        }
    }

    /// Applies an area's texture swap animations to the model added last.
    /// Each animation replaces the texture of every material that uses one
    /// of the textures it cycles through.
    pub fn animate_area_patterns(&mut self, model: &Model, patterns: &[(u64, AreaPattern)]) {
        for (set_key, pattern) in patterns {
            let tex = &pattern.textures;
            for group in &pattern.groups {
                let name = |index: u8| tex.textures.get(index as usize).map(|t| t.name.as_str());
                let targets = self.last_materials_where(model, |m| {
                    m.texture.as_deref().is_some_and(|t| group.iter().any(|k| name(k.texture) == Some(t)))
                });
                if targets.is_empty() {
                    continue;
                }
                let mut keys = Vec::with_capacity(group.len());
                for k in group {
                    let palette = tex.palettes.get(k.palette as usize).map(|p| p.name.as_str());
                    if let Some(texture) = name(k.texture) {
                        if let Some(t) = self.texture(*set_key, tex, texture, palette) {
                            keys.push((k.frame as f32, t));
                        }
                    }
                }
                self.push_pattern(&targets, pattern.frame_count as f32, keys);
            }
        }
    }

    fn push_pattern(&mut self, targets: &[usize], frames: f32, keys: Vec<(f32, usize)>) {
        if keys.len() < 2 {
            return;
        }
        for &material in targets {
            if !self.scene.animations.tex_pattern.iter().any(|a| a.material == material) {
                self.scene.animations.tex_pattern.push(TexPatternAnim { material, frames, keys: keys.clone() });
            }
        }
    }

    /// Makes the joints of a scene model move.
    pub fn animate_joints(&mut self, scene_model: usize, model: &Model, animation: &JointAnimation) {
        if animation.frame_count < 2 || animation.joints.is_empty() {
            return;
        }
        self.scene.animations.joints.push(JointAnim {
            model: scene_model,
            frames: animation.frame_count as f32,
            source: std::sync::Arc::new(model.clone()),
            animation: std::sync::Arc::new(animation.clone()),
        });
    }

    /// Applies a building's own animation files to the model added last, if
    /// the building plays them by itself.
    pub fn animate_building(&mut self, scene_model: usize, model: &Model, res: &BuildingResource, sets: &[(u64, &Tex0)]) {
        if res.plays_by_itself() {
            self.animate_files(scene_model, model, res.ambient_animations(), sets);
        }
    }

    /// Applies animation files (joint, texture placement, texture swap) to
    /// the model added last.
    pub fn animate_files(&mut self, scene_model: usize, model: &Model, files: &[&[u8]], sets: &[(u64, &Tex0)]) {
        for file in files {
            match NitroFile::sniff(file).as_ref() {
                Some(b"BCA0") => {
                    if let Some(a) = parse_joint(file).ok().and_then(|a| a.into_iter().next()) {
                        self.animate_joints(scene_model, model, &a);
                    }
                }
                Some(b"BTA0") => {
                    if let Ok(a) = parse_tex_srt(file) {
                        self.animate_tex_srt(model, &a);
                    }
                }
                Some(b"BTP0") => {
                    if let Ok(a) = parse_tex_pattern(file) {
                        self.animate_tex_pattern(model, &a, sets);
                    }
                }
                _ => {}
            }
        }
    }

    /// Adds a model once per `key`, building it on first use.
    fn cached_model(&mut self, key: String, build: impl FnOnce(&mut Self) -> Option<usize>) -> Option<usize> {
        if let Some(&m) = self.models.get(&key) {
            return m;
        }
        let m = build(self);
        self.models.insert(key, m);
        m
    }

    pub fn place(&mut self, model: usize, transform: Mat4, kind: NodeKind) {
        self.scene.nodes.push(SceneNode { model, transform, kind });
    }
}

fn fallback_material() -> Material {
    Material {
        name: "default".into(),
        texture: None,
        palette: None,
        diffuse: [0.8; 3],
        ambient: [0.4; 3],
        specular: [0.0; 3],
        emission: [0.0; 3],
        diffuse_is_vertex_color: false,
        poly_attr: 0x001F_00C0,
        tex_image_param: 0,
        flags: 0xE,
        tex_width: 0,
        tex_height: 0,
        tex_scale: glam::Vec2::ONE,
        tex_rot_sin: 0.0,
        tex_rot_cos: 1.0,
        tex_translation: glam::Vec2::ZERO,
    }
}

/// What to include when stitching a map.
#[derive(Debug, Clone)]
pub struct MapRequest {
    pub matrix: usize,
    /// Only build the cells owned by this zone.
    pub only_zone: Option<u32>,
    /// 0 spring, 1 summer, 2 autumn, 3 winter.
    pub season: u8,
    /// Part of the day used for lighting (0 morning .. 4 late night).
    pub day_part: u16,
    /// Include the ground models of the chunks.
    pub terrain: bool,
    pub buildings: bool,
    pub markers: bool,
    /// Include what moves by itself: water, flowers, windmills.
    pub animations: bool,
    /// Zones to leave out on a map that holds several. Their cells stay
    /// in the layout, marked hidden.
    pub hidden_zones: Vec<u32>,
    /// Leave out the cells that belong to no zone, on a map that holds
    /// several: the sea and forest that fill the gaps between them.
    pub hide_unassigned: bool,
}

impl MapRequest {
    pub fn matrix(matrix: usize) -> Self {
        Self {
            matrix,
            only_zone: None,
            season: 0,
            day_part: 1,
            terrain: true,
            buildings: true,
            markers: true,
            animations: true,
            hidden_zones: Vec::new(),
            hide_unassigned: false,
        }
    }
}

struct AreaAssets {
    /// Texture placement animations of the area's terrain materials.
    srt: Vec<TexSrtAnimation>,
    /// Texture swap animations of the area's terrain, with their set keys.
    patterns: Vec<(u64, AreaPattern)>,
    textures: Option<(u64, Tex0)>,
    building_tex: Option<(u64, Tex0)>,
    bundle_data: Option<vmap_fs::Bytes>,
    bundle_key: String,
}

impl World {
    /// Area actually used by a zone in a given season.
    pub fn seasonal_area(&self, zone: u32, season: u8) -> Option<usize> {
        let base = self.zones.get(zone as usize)?.area_id as usize;
        let seasonal_end = if self.game.is_bw2() { 282 } else { 210 };
        let id = if (2..seasonal_end).contains(&base) { base + (season as usize & 3) } else { base };
        (id < self.areas.len()).then_some(id).or((base < self.areas.len()).then_some(base))
    }

    fn area_assets(&self, area_id: usize) -> AreaAssets {
        let area = self.areas[area_id];
        let textures = self
            .fs
            .arc_file(Arc::MapTextures, area.textures_id as usize)
            .and_then(|d| Tex0::from_btx(&d).ok())
            .map(|t| (0x1_0000_0000 | area.textures_id as u64, t));

        let bundle_id =
            if self.game.is_bw2() { area.buildings_id as usize } else { bw1_buildings_id(area_id) };
        let (bundle_arc, tex_arc, tag) = if area.is_exterior {
            (Arc::BuildingsExterior, Arc::BuildingTexExterior, 2u64)
        } else {
            (Arc::BuildingsInterior, Arc::BuildingTexInterior, 3u64)
        };
        let building_tex = self
            .fs
            .arc_file(tex_arc, bundle_id)
            .and_then(|d| Tex0::from_btx(&d).ok())
            .map(|t| (tag << 32 | bundle_id as u64, t));
        // 255 means the area has no animation of that kind.
        let srt = (area.srt_anim != 255)
            .then(|| self.fs.arc_file(Arc::MapAnimSrt, area.srt_anim as usize))
            .flatten()
            .and_then(|d| parse_tex_srt(&d).ok())
            .unwrap_or_default();
        let patterns = (area.pattern_anim != 255)
            .then(|| self.fs.arc_file(Arc::MapAnimPattern, area.pattern_anim as usize))
            .flatten()
            .and_then(|d| parse_area_patterns(&d).ok())
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, p)| (0x6_0000_0000 | (area.pattern_anim as u64) << 8 | i as u64, p))
            .collect();
        AreaAssets {
            srt,
            patterns,
            textures,
            building_tex,
            bundle_data: self.fs.arc_file(bundle_arc, bundle_id),
            bundle_key: format!("{}:{bundle_id}", if area.is_exterior { "ext" } else { "int" }),
        }
    }

    /// Stitches every chunk of a map matrix into one scene.
    pub fn build_map(&self, req: &MapRequest) -> anyhow::Result<Scene> {
        let matrix = self.matrix(req.matrix)?;
        let span = self.chunk_span;
        let mut b = SceneBuilder::new();

        // Zones that use this matrix; for matrices without a zone grid every
        // cell belongs to the first (or requested) one.
        let users = self.zones_of_matrix(req.matrix);
        let default_zone = req.only_zone.or(users.first().copied());

        let mut areas: HashMap<usize, AreaAssets> = HashMap::new();
        let mut cells = Vec::new();
        let mut zones_seen: Vec<u32> = Vec::new();

        for y in 0..matrix.height {
            for x in 0..matrix.width {
                let Some(chunk_id) = matrix.chunk(x, y) else { continue };
                let zone = if matrix.zones.is_some() { matrix.zone(x, y) } else { default_zone };
                if let Some(only) = req.only_zone {
                    if matrix.zones.is_some() && zone != Some(only) {
                        continue;
                    }
                }
                let hidden = match zone {
                    Some(z) => req.hidden_zones.contains(&z),
                    None => req.hide_unassigned,
                };
                if matrix.zones.is_some() && hidden {
                    cells.push(MapCell { x, y, chunk_id, zone, hidden: true });
                    continue;
                }
                let area_zone = zone.or(default_zone);
                let Some(area_id) = area_zone.and_then(|z| self.seasonal_area(z, req.season)) else {
                    b.warn(format!("cell ({x},{y}) has no zone with a valid area"));
                    continue;
                };
                if let Some(z) = zone {
                    if !zones_seen.contains(&z) {
                        zones_seen.push(z);
                    }
                }
                let assets = areas.entry(area_id).or_insert_with(|| self.area_assets(area_id));

                let Some(data) = self.fs.arc_file(Arc::MapChunks, chunk_id as usize) else {
                    b.warn(format!("map chunk {chunk_id} is missing"));
                    continue;
                };
                let chunk = match MapChunk::parse(&data) {
                    Ok(c) => c,
                    Err(e) => {
                        b.warn(format!("map chunk {chunk_id}: {e}"));
                        continue;
                    }
                };
                cells.push(MapCell { x, y, chunk_id, zone, hidden: false });
                let origin = Vec3::new((x as f32 + 0.5) * span, 0.0, (y as f32 + 0.5) * span);

                // Terrain model.
                let key = format!("chunk:{chunk_id}:{area_id}");
                let model = b.cached_model(key, |b| {
                    let mdl = match Mdl0::from_bmd(chunk.model) {
                        Ok(m) => m,
                        Err(e) => {
                            b.warn(format!("map chunk {chunk_id} model: {e}"));
                            return None;
                        }
                    };
                    let model = mdl.models.first()?;
                    let mut sets: Vec<(u64, &Tex0)> = Vec::new();
                    if let Some((k, t)) = &assets.textures {
                        sets.push((*k, t));
                    }
                    if let Some(t) = &mdl.textures {
                        sets.push((0x4_0000_0000 | chunk_id as u64, t));
                    }
                    let added = b.add_model(model, &sets, None);
                    if req.animations {
                        b.animate_tex_srt(model, &assets.srt);
                        b.animate_area_patterns(model, &assets.patterns);
                    }
                    Some(added)
                });
                if let (true, Some(m)) = (req.terrain, model) {
                    b.place(m, Mat4::from_translation(origin), NodeKind::Chunk { x, y, chunk_id, zone });
                }

                // Buildings standing on this chunk.
                if !req.buildings || chunk.buildings.is_empty() {
                    continue;
                }
                let Some(bundle_data) = assets.bundle_data.clone() else { continue };
                let Ok(bundle) = BuildingBundle::parse(&bundle_data) else { continue };
                for cb in &chunk.buildings {
                    let pos = Vec3::new(origin.x + cb.position.x, cb.position.y, origin.z - cb.position.z);
                    let xf = Mat4::from_translation(pos) * Mat4::from_rotation_y(cb.rotation_deg.to_radians());
                    let Some(res) = bundle.by_uid(cb.model_uid) else {
                        b.warn(format!(
                            "chunk {chunk_id}: building {} is not in bundle {}",
                            cb.model_uid, assets.bundle_key
                        ));
                        continue;
                    };
                    let place = |b: &mut SceneBuilder, uid: u16, xf: Mat4, kind: NodeKind| {
                        let Some(res) = bundle.by_uid(uid) else { return };
                        let key = format!("bld:{}:{uid}:{area_id}", assets.bundle_key);
                        let model = b.cached_model(key, |b| {
                            let mdl = Mdl0::from_bmd(res.model).ok()?;
                            let model = mdl.models.first()?;
                            let mut sets: Vec<(u64, &Tex0)> = Vec::new();
                            if let Some((k, t)) = &assets.building_tex {
                                sets.push((*k, t));
                            }
                            if let Some(t) = &mdl.textures {
                                sets.push((0x5_0000_0000 | (area_id as u64) << 16 | uid as u64, t));
                            }
                            let added = b.add_model(model, &sets, None);
                            if req.animations {
                                b.animate_building(added, model, res, &sets);
                            }
                            Some(added)
                        });
                        if let Some(m) = model {
                            b.place(m, xf, kind);
                        }
                    };
                    place(&mut b, cb.model_uid, xf, NodeKind::Building { uid: cb.model_uid, zone });
                    if let Some(door) = res.door_uid {
                        let d = res.door_pos;
                        let dxf = xf * Mat4::from_translation(Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32));
                        place(&mut b, door, dxf, NodeKind::Door { uid: door });
                    }
                }
            }
        }

        // Entities of every zone on the map.
        if req.markers {
            let mut marker_zones = zones_seen.clone();
            if matrix.zones.is_none() {
                marker_zones = match req.only_zone {
                    Some(z) => vec![z],
                    None => users.clone(),
                };
            }
            for z in marker_zones {
                self.add_markers(&mut b, z);
            }
        }

        // Lighting from the area of the first zone that is shown.
        if let Some(area) = default_zone
            .filter(|z| !req.hidden_zones.contains(z))
            .or(zones_seen.first().copied())
            .or(default_zone)
            .and_then(|z| self.seasonal_area(z, req.season))
        {
            if let Some(l) = self.lighting(self.areas[area].light_index as usize, req.day_part) {
                b.scene.lighting = l;
            }
        }

        b.scene.layout = Some(MapLayout {
            matrix_id: req.matrix,
            width: matrix.width,
            height: matrix.height,
            chunk_span: span,
            cells,
        });
        Ok(b.finish())
    }

    /// The NPCs, warps, triggers and interactable objects of a zone.
    /// `None` if the zone has no entity file; an error if it cannot be read.
    pub fn zone_entities(&self, zone: u32) -> Option<anyhow::Result<ZoneEntities>> {
        let header = self.zones.get(zone as usize)?;
        let data = self.fs.arc_file(Arc::ZoneEntities, header.entities_id as usize)?;
        Some(ZoneEntities::parse(&data).map_err(Into::into))
    }

    fn add_markers(&self, b: &mut SceneBuilder, zone: u32) {
        let ents = match self.zone_entities(zone) {
            Some(Ok(ents)) => ents,
            Some(Err(_)) => {
                b.warn(format!("zone {zone}: entity file could not be read"));
                return;
            }
            None => return,
        };
        let tile = vmap_formats::gen5::TILE_SIZE;
        let mut push = |kind, pos: Option<[f32; 3]>, size: [f32; 2], index, label: String, target| {
            if let Some(p) = pos {
                b.scene.markers.push(Marker { kind, pos: Vec3::from(p), size, zone, index, label, target });
            }
        };
        for (i, n) in ents.npcs.iter().enumerate() {
            push(MarkerKind::Npc, n.placement.world(), [tile, tile], i, format!("NPC {i} (model {})", n.obj_code), None);
        }
        for (i, w) in ents.warps.iter().enumerate() {
            let size = [(w.width.max(1)) as f32 * tile, (w.height.max(1)) as f32 * tile];
            let target = self.zone_label(w.target_zone as u32);
            push(
                MarkerKind::Warp,
                w.placement.world(),
                size,
                i,
                format!("Warp {i} to {target}"),
                Some((w.target_zone, w.target_warp)),
            );
        }
        for (i, t) in ents.triggers.iter().enumerate() {
            let size = [t.width.max(1) as f32 * tile, t.height.max(1) as f32 * tile];
            push(MarkerKind::Trigger, t.placement.world(), size, i, format!("Trigger {i} (script {})", t.script), None);
        }
        for (i, f) in ents.furniture.iter().enumerate() {
            push(
                MarkerKind::Furniture,
                f.placement.world(),
                [tile, tile],
                i,
                format!("Interactable {i} (script {})", f.script),
                None,
            );
        }
    }

    /// Lighting setup of an area light file for a part of the day.
    pub fn lighting(&self, light_index: usize, day_part: u16) -> Option<Lighting> {
        let data = self.fs.arc_file(Arc::EnvLights, light_index)?;
        let entries = LightEntry::parse_file(&data).ok()?;
        let e = entries.iter().find(|e| e.day_part == day_part).or(entries.first())?;
        let mut l = Lighting::default();
        for i in 0..4 {
            l.lights[i] = Light {
                enabled: e.enabled[i],
                direction: e.directions[i].normalize_or(Vec3::NEG_Y),
                color: e.colors[i],
            };
        }
        l.clear_color = e.clear_color;
        if !l.lights.iter().any(|x| x.enabled) {
            return None;
        }
        Some(l)
    }

    /// Scene of one building of a bundle with its animations playing, also
    /// those the game only plays on demand (a door opening). `model` is the
    /// building's model file as stored in the bundle.
    pub fn build_building_scene(&self, bundle: &[u8], model: &[u8], extra: &[&Tex0]) -> anyhow::Result<Scene> {
        let bundle = BuildingBundle::parse(bundle)?;
        let res = bundle
            .buildings
            .iter()
            .find(|b| b.model == model)
            .ok_or_else(|| anyhow::anyhow!("the model is not part of this bundle"))?;
        let mdl = Mdl0::from_bmd(res.model)?;
        let mut b = SceneBuilder::new();
        let mut sets: Vec<(u64, &Tex0)> = Vec::new();
        if let Some(t) = &mdl.textures {
            sets.push((1, t));
        }
        for (i, t) in extra.iter().enumerate() {
            sets.push((2 + i as u64, t));
        }
        for model in &mdl.models {
            let m = b.add_model(model, &sets, None);
            b.animate_files(m, model, res.ambient_animations(), &sets);
            b.place(m, Mat4::IDENTITY, NodeKind::Model);
        }
        Ok(b.finish())
    }

    /// Scene holding every model of one `BMD0` file, for the asset viewer.
    /// `extra` supplies textures the file does not embed.
    pub fn build_model_scene(&self, bmd: &[u8], extra: &[&Tex0]) -> anyhow::Result<Scene> {
        let mdl = Mdl0::from_bmd(bmd)?;
        let mut b = SceneBuilder::new();
        let mut sets: Vec<(u64, &Tex0)> = Vec::new();
        if let Some(t) = &mdl.textures {
            sets.push((1, t));
        }
        for (i, t) in extra.iter().enumerate() {
            sets.push((2 + i as u64, t));
        }
        for model in &mdl.models {
            let m = b.add_model(model, &sets, None);
            b.place(m, Mat4::IDENTITY, NodeKind::Model);
        }
        Ok(b.finish())
    }
}
