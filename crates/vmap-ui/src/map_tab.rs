//! The map viewer: a list of maps, the stitched 3D view with its overview,
//! and details about whatever is selected.

use std::collections::HashMap;

use eframe::egui::{self, Color32};
use glam::Vec3;
use vmap_render::GpuScene;
use vmap_world::{MapRequest, Marker, MarkerKind, Scene, World};

use crate::viewport::{Gfx, Viewport};

struct MapEntry {
    matrix: usize,
    /// Zone to show on matrices that several zones share without a zone grid.
    zone: Option<u32>,
    /// A zone on a shared map: opening the entry opens the whole map and
    /// moves the view to this zone. Only listed while searching.
    jump_to: Option<u32>,
    title: String,
    detail: String,
    search: String,
}

struct LoadedMap {
    entry: usize,
    scene: Scene,
    gpu: GpuScene,
    generation: u64,
    /// Zone number to the centre of its cells.
    zone_centres: Vec<(u32, Vec3)>,
}

#[derive(Clone, PartialEq)]
enum Selection {
    Marker(usize),
    Cell { x: usize, y: usize },
}

pub struct MapTab {
    entries: Vec<MapEntry>,
    filter: String,
    loaded: Option<LoadedMap>,
    viewport: Viewport,
    season: u8,
    day_part: u16,
    buildings: bool,
    /// Zones of the open map that are not drawn.
    hidden_zones: Vec<u32>,
    /// The cells of the open map that belong to no zone are not drawn.
    hide_unassigned: bool,
    animate: bool,
    show_labels: bool,
    show_overview: bool,
    selection: Option<Selection>,
    hover: String,
    error: Option<String>,
    generation: u64,
    /// A warp to focus once the map it leads to has loaded.
    pending_focus: Option<(u32, u16)>,
}

const SEASONS: [&str; 4] = ["Spring", "Summer", "Autumn", "Winter"];
const DAY_PARTS: [&str; 5] = ["Morning", "Day", "Evening", "Night", "Late night"];

impl MapTab {
    pub fn new(world: &World) -> Self {
        let mut entries = Vec::new();
        for info in world.matrices() {
            let zoned = world.matrix(info.id).map(|m| m.zones.is_some()).unwrap_or(false);
            let users = world.zones_of_matrix(info.id);
            let size = format!("{}x{}", info.width, info.height);
            if zoned || users.len() <= 1 {
                let title = if info.name.is_empty() { format!("Matrix {}", info.id) } else { info.name.clone() };
                let zones = match info.zones.len() {
                    0 => "unused".to_string(),
                    1 => format!("zone {}", info.zones[0]),
                    n => format!("{n} zones"),
                };
                let detail = format!("matrix {} · {size} · {zones}", info.id);
                let search = format!(
                    "{title} {detail} {}",
                    info.zones.iter().map(|z| world.zone_label(*z)).collect::<Vec<_>>().join(" ")
                )
                .to_lowercase();
                entries.push(MapEntry {
                    matrix: info.id,
                    zone: if zoned { None } else { users.first().copied() },
                    jump_to: None,
                    title,
                    detail,
                    search,
                });
                if zoned && info.zones.len() > 1 {
                    for &z in &info.zones {
                        let title = world.location_name(z).map(str::to_string).unwrap_or(format!("Zone {z}"));
                        let detail = format!("zone {z} · part of matrix {}", info.id);
                        let search = format!("{title} {detail}").to_lowercase();
                        entries.push(MapEntry { matrix: info.id, zone: None, jump_to: Some(z), title, detail, search });
                    }
                }
            } else {
                // One room per zone: same layout, own textures and entities.
                for z in users {
                    let title = world.location_name(z).map(str::to_string).unwrap_or(format!("Zone {z}"));
                    let detail = format!("matrix {} · {size} · zone {z}", info.id);
                    let search = format!("{title} {detail}").to_lowercase();
                    entries.push(MapEntry { matrix: info.id, zone: Some(z), jump_to: None, title, detail, search });
                }
            }
        }
        let mut tab = Self {
            entries,
            filter: String::new(),
            loaded: None,
            viewport: Viewport::default(),
            season: 0,
            day_part: 1,
            buildings: true,
            hidden_zones: Vec::new(),
            hide_unassigned: false,
            animate: true,
            show_labels: true,
            show_overview: true,
            selection: None,
            hover: String::new(),
            error: None,
            generation: 0,
            pending_focus: None,
        };
        tab.viewport.set_top_down(false);
        tab
    }

    /// Entry showing a zone: its own room, or the shared map it lies on.
    fn entry_for_zone(&self, world: &World, zone: u32) -> Option<usize> {
        let matrix = world.zones.get(zone as usize)?.matrix_id as usize;
        self.entries
            .iter()
            .position(|e| e.matrix == matrix && e.zone == Some(zone))
            .or_else(|| self.entries.iter().position(|e| e.matrix == matrix && e.jump_to.is_none()))
    }

    pub fn open_first(&mut self, world: &World, gfx: &Gfx) {
        if self.loaded.is_none() && !self.entries.is_empty() {
            self.load(world, gfx, 0, true);
        }
    }

    /// Opens the first map whose name or details contain `text`.
    pub fn open_matching(&mut self, world: &World, gfx: &Gfx, text: &str) {
        let text = text.to_lowercase();
        // A name match beats a match in the details (a big map lists every
        // zone it contains there).
        let by_title = self.entries.iter().position(|e| e.title.to_lowercase().contains(&text));
        if let Some(i) = by_title.or_else(|| self.entries.iter().position(|e| e.search.contains(&text))) {
            self.load(world, gfx, i, true);
        }
    }

    /// Takes over what another tab was showing, after the game was read
    /// again: same map, same view, same switches.
    pub fn restore_from(&mut self, old: &MapTab, world: &World, gfx: &Gfx) {
        self.filter = old.filter.clone();
        (self.season, self.day_part, self.buildings, self.animate) =
            (old.season, old.day_part, old.buildings, old.animate);
        (self.show_labels, self.show_overview) = (old.show_labels, old.show_overview);
        self.hidden_zones = old.hidden_zones.clone();
        self.hide_unassigned = old.hide_unassigned;
        self.viewport.camera = old.viewport.camera;
        self.viewport.options = old.viewport.options;
        let entry = old.loaded.as_ref().map(|l| &old.entries[l.entry]).and_then(|o| {
            self.entries.iter().position(|e| e.matrix == o.matrix && e.zone == o.zone && e.jump_to == o.jump_to)
        });
        match entry {
            Some(e) => self.load(world, gfx, e, false),
            None => self.open_first(world, gfx),
        }
    }

    /// Selects a warp of the loaded map and moves the view to it.
    fn select_warp(&mut self, zone: u32, warp: u16) {
        let found = self.loaded.as_ref().and_then(|l| {
            l.scene
                .markers
                .iter()
                .position(|m| m.kind == MarkerKind::Warp && m.zone == zone && m.index == warp as usize)
                .map(|i| (i, l.scene.markers[i].pos))
        });
        if let Some((i, pos)) = found {
            self.selection = Some(Selection::Marker(i));
            self.focus(pos, 400.0);
        }
    }

    /// Selects the `n`-th warp of the loaded map and optionally goes where it
    /// leads, as clicking it and pressing "Follow warp" would.
    pub fn script_warp(&mut self, world: &World, gfx: &Gfx, n: usize, follow: bool) {
        let warp = self.loaded.as_ref().and_then(|l| {
            l.scene.markers.iter().filter(|m| m.kind == MarkerKind::Warp).nth(n).map(|m| (m.zone, m.index, m.target))
        });
        let Some((zone, index, target)) = warp else { return };
        self.select_warp(zone, index as u16);
        if let (true, Some((to_zone, to_warp))) = (follow, target) {
            if let Some(entry) = self.entry_for_zone(world, to_zone as u32) {
                self.load(world, gfx, entry, true);
                self.select_warp(to_zone as u32, to_warp);
            }
        }
    }

    /// Hides zones of the open map, as clicking their eyes would.
    pub fn script_hide(&mut self, world: &World, gfx: &Gfx, zones: &[u32], unassigned: bool) {
        self.hidden_zones = zones.to_vec();
        self.hide_unassigned = unassigned;
        if let Some(entry) = self.loaded.as_ref().map(|l| l.entry) {
            self.load(world, gfx, entry, false);
        }
    }

    pub fn set_top_down(&mut self, on: bool) {
        self.viewport.set_top_down(on);
        if let Some(l) = &self.loaded {
            self.viewport.camera.frame(l.scene.min, l.scene.max, self.viewport.aspect());
            self.viewport.camera.target.y = 0.0;
        }
    }

    fn load(&mut self, world: &World, gfx: &Gfx, entry: usize, reset_camera: bool) {
        // Hidden zones belong to one map; another map starts with all shown.
        if self.loaded.as_ref().is_some_and(|l| self.entries[l.entry].matrix != self.entries[entry].matrix) {
            self.hidden_zones.clear();
            self.hide_unassigned = false;
        }
        let e = &self.entries[entry];
        let request = MapRequest {
            matrix: e.matrix,
            only_zone: e.zone,
            season: self.season,
            day_part: self.day_part,
            terrain: true,
            buildings: self.buildings,
            markers: true,
            animations: true,
            hidden_zones: self.hidden_zones.clone(),
            hide_unassigned: self.hide_unassigned,
        };
        match world.build_map(&request) {
            Ok(scene) => {
                self.error = None;
                self.generation += 1;
                let gpu = GpuScene::new(&gfx.rs.device, &gfx.rs.queue, &gfx.renderer, &scene);

                let mut sums: HashMap<u32, (Vec3, f32)> = HashMap::new();
                if let Some(layout) = &scene.layout {
                    for c in &layout.cells {
                        if let Some(z) = c.zone {
                            let p = Vec3::new(
                                (c.x as f32 + 0.5) * layout.chunk_span,
                                0.0,
                                (c.y as f32 + 0.5) * layout.chunk_span,
                            );
                            let s = sums.entry(z).or_insert((Vec3::ZERO, 0.0));
                            s.0 += p;
                            s.1 += 1.0;
                        }
                    }
                }
                let mut zone_centres: Vec<(u32, Vec3)> = sums.into_iter().map(|(z, (p, n))| (z, p / n)).collect();
                zone_centres.sort_by_key(|(z, _)| *z);

                if reset_camera {
                    self.viewport.camera.frame(scene.min, scene.max, self.viewport.aspect());
                    self.viewport.camera.target.y = 0.0;
                    self.selection = None;
                }
                // A zone opened from the search starts with the view on it.
                let jump = self.entries[entry]
                    .jump_to
                    .filter(|_| reset_camera)
                    .and_then(|z| zone_centres.iter().find(|c| c.0 == z))
                    .map(|c| c.1);
                self.loaded = Some(LoadedMap { entry, scene, gpu, generation: self.generation, zone_centres });
                if let Some(centre) = jump {
                    self.focus(centre, 900.0);
                }
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn focus(&mut self, point: Vec3, view_height: f32) {
        let cam = &mut self.viewport.camera;
        cam.target = Vec3::new(point.x, 0.0, point.z);
        cam.distance = view_height / (2.0 * (cam.fov_y * 0.5).tan());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, world: &World, gfx: &Gfx) {
        let mut to_load: Option<(usize, bool)> = None;

        egui::Panel::left("map_list").default_size(270.0).resizable(true).show(ui, |ui| {
            ui.add_space(4.0);
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search maps and zones").desired_width(f32::INFINITY));
            ui.add_space(4.0);
            let filter = self.filter.to_lowercase();
            let visible: Vec<usize> = (0..self.entries.len())
                .filter(|&i| {
                    let e = &self.entries[i];
                    if filter.is_empty() { e.jump_to.is_none() } else { e.search.contains(&filter) }
                })
                .collect();
            // A zone opened through search shows its whole map, so that map's
            // entry is highlighted too.
            let current = self.loaded.as_ref().map(|l| l.entry);
            let current_map = current.filter(|&c| self.entries[c].jump_to.is_some()).and_then(|c| {
                let matrix = self.entries[c].matrix;
                self.entries.iter().position(|e| e.matrix == matrix && e.jump_to.is_none())
            });
            let row = crate::widgets::row_height(true);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row, visible.len(), |ui, range| {
                for &i in &visible[range] {
                    let e = &self.entries[i];
                    let selected = current == Some(i) || current_map == Some(i);
                    if crate::widgets::list_row(ui, selected, &e.title, Some(&e.detail), None, row).clicked() {
                        to_load = Some((i, true));
                    }
                }
            });
        });

        egui::Panel::right("map_details").default_size(290.0).resizable(true).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.details(ui, world, &mut to_load);
            });
        });

        egui::Panel::bottom("map_status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let text = if self.hover.is_empty() {
                    "Left drag: move · Right drag: rotate · Wheel: zoom · Click: select"
                } else {
                    &self.hover
                };
                ui.label(egui::RichText::new(text).small());
            });
        });

        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let mut rebuild = false;
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let top = self.viewport.is_top_down();
                    if ui.selectable_label(!top, "3D").clicked() && top {
                        self.viewport.set_top_down(false);
                    }
                    if ui.selectable_label(top, "Top-down").clicked() && !top {
                        self.viewport.set_top_down(true);
                    }
                    if ui.button("Fit").clicked() {
                        if let Some(l) = &self.loaded {
                            self.viewport.camera.frame(l.scene.min, l.scene.max, self.viewport.aspect());
                            self.viewport.camera.target.y = 0.0;
                        }
                    }
                    ui.separator();
                    ui.checkbox(&mut self.viewport.options.markers, "Markers");
                    rebuild |= ui.checkbox(&mut self.buildings, "Buildings").changed();
                    ui.checkbox(&mut self.viewport.options.lighting, "Lighting");
                    let animated = self.loaded.as_ref().is_some_and(|l| l.gpu.has_animations());
                    ui.add_enabled(animated, egui::Checkbox::new(&mut self.animate, "Animations"))
                        .on_disabled_hover_text("Nothing on this map moves by itself.");
                    ui.checkbox(&mut self.show_labels, "Zone names");
                    ui.checkbox(&mut self.show_overview, "Overview");
                    ui.separator();
                    egui::ComboBox::from_id_salt("season").selected_text(SEASONS[self.season as usize & 3]).show_ui(ui, |ui| {
                        for (i, name) in SEASONS.iter().enumerate() {
                            rebuild |= ui.selectable_value(&mut self.season, i as u8, *name).changed();
                        }
                    });
                    egui::ComboBox::from_id_salt("day_part")
                        .selected_text(DAY_PARTS[(self.day_part as usize).min(4)])
                        .show_ui(ui, |ui| {
                            for (i, name) in DAY_PARTS.iter().enumerate() {
                                rebuild |= ui.selectable_value(&mut self.day_part, i as u16, *name).changed();
                            }
                        });
                });
            });
            if rebuild {
                if let Some(entry) = self.loaded.as_ref().map(|l| l.entry) {
                    to_load = Some((entry, false));
                }
            }

            if let Some(err) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
            // Advance animations in steps of one game frame.
            if let Some(l) = self.loaded.as_mut().filter(|l| self.animate && l.gpu.has_animations()) {
                let fps = vmap_world::ANIMATION_FPS as f64;
                let time = ((ui.input(|i| i.time) * fps).floor() / fps) as f32;
                l.gpu.animate(&gfx.rs.queue, time);
                self.viewport.time = time;
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(1.0 / fps));
            }
            let scene = self.loaded.as_ref().map(|l| (&l.gpu, l.generation));
            let out = self.viewport.show(ui, gfx, scene);
            self.viewport_overlays(ui, world, &out.response, out.rect);
        });

        if let Some((entry, reset)) = to_load {
            self.load(world, gfx, entry, reset);
            if let Some((zone, warp)) = self.pending_focus.take() {
                self.select_warp(zone, warp);
            }
        }
    }

    /// Hover text, picking, zone names and the overview, drawn over the view.
    fn viewport_overlays(&mut self, ui: &mut egui::Ui, world: &World, response: &egui::Response, rect: egui::Rect) {
        let Some(loaded) = &self.loaded else { return };
        let painter = ui.painter_at(rect);
        let span = loaded.scene.layout.as_ref().map(|l| l.chunk_span).unwrap_or(512.0);

        // Overview first, so it can claim the pointer.
        let mut over_overview = false;
        if self.show_overview {
            if let Some(layout) = loaded.scene.layout.as_ref().filter(|l| l.cells.len() > 1) {
                let max_side = 220.0f32.min(rect.width() * 0.35).min(rect.height() * 0.45);
                let scale = max_side / layout.width.max(layout.height) as f32;
                let size = egui::vec2(layout.width as f32 * scale, layout.height as f32 * scale);
                let mini = egui::Rect::from_min_size(rect.right_bottom() - size - egui::vec2(12.0, 12.0), size);
                let resp = ui.interact(mini, ui.scope_id().with("overview"), egui::Sense::click_and_drag());
                over_overview = resp.hovered() || resp.dragged();
                painter.rect_filled(mini.expand(4.0), 4.0, Color32::from_black_alpha(170));
                for c in &layout.cells {
                    let r = egui::Rect::from_min_size(
                        mini.min + egui::vec2(c.x as f32 * scale, c.y as f32 * scale),
                        egui::vec2(scale, scale),
                    );
                    let col = c.zone.map(zone_color).unwrap_or(Color32::from_gray(120));
                    let col = if c.hidden { col.gamma_multiply(0.25) } else { col };
                    painter.rect_filled(r.shrink(0.5), 0.0, col);
                }
                // Camera position.
                let t = self.viewport.camera.target;
                let p = mini.min + egui::vec2(t.x / span * scale, t.z / span * scale);
                if mini.expand(2.0).contains(p) {
                    painter.circle_stroke(p, 5.0, egui::Stroke::new(2.0, Color32::WHITE));
                    painter.circle_filled(p, 2.0, Color32::BLACK);
                }
                if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.clicked() || resp.dragged()) {
                    let rel = pos - mini.min;
                    self.viewport.camera.target.x = rel.x / scale * span;
                    self.viewport.camera.target.z = rel.y / scale * span;
                }
            }
        }
        let Some(loaded) = &self.loaded else { return };

        // Zone names at the centre of each zone's cells.
        if self.show_labels && loaded.zone_centres.len() > 1 {
            let font = egui::FontId::proportional(12.0);
            for (zone, centre) in &loaded.zone_centres {
                if self.hidden_zones.contains(zone) {
                    continue;
                }
                let Some(pos) = self.viewport.project(rect, *centre) else { continue };
                if !rect.contains(pos) {
                    continue;
                }
                let text = world.location_name(*zone).map(str::to_string).unwrap_or(format!("Zone {zone}"));
                let galley = painter.layout_no_wrap(text, font.clone(), Color32::WHITE);
                let r = egui::Rect::from_center_size(pos, galley.size()).expand2(egui::vec2(5.0, 2.0));
                painter.rect_filled(r, 4.0, Color32::from_black_alpha(150));
                painter.galley(r.min + egui::vec2(5.0, 2.0), galley, Color32::WHITE);
            }
        }

        // Outline of the selected marker.
        if let Some(Selection::Marker(i)) = &self.selection {
            if let Some(m) = loaded.scene.markers.get(*i) {
                let (min, size) = m.bounds();
                let top = min + Vec3::new(size.x * 0.5, size.y, size.z * 0.5);
                if let Some(p) = self.viewport.project(rect, top) {
                    painter.circle_stroke(p, 9.0, egui::Stroke::new(2.5, Color32::WHITE));
                    painter.circle_stroke(p, 11.0, egui::Stroke::new(1.0, Color32::BLACK));
                }
            }
        }

        // Hover text and click selection.
        self.hover.clear();
        if over_overview {
            return;
        }
        let Some(pos) = response.hover_pos() else { return };
        let (origin, dir) = self.viewport.ray(rect, pos);
        let hit_marker = if self.viewport.options.markers {
            loaded
                .scene
                .markers
                .iter()
                .enumerate()
                .filter_map(|(i, m)| m.ray_hit(origin, dir).map(|t| (i, t)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        } else {
            None
        };
        let ground = self.viewport.pick_ground(rect, pos);
        let mut cell = None;
        if let Some(g) = ground {
            let (cx, cy) = ((g.x / span).floor(), (g.z / span).floor());
            let mut text = format!("x {:.0}  z {:.0} · tile {}, {}", g.x, g.z, (g.x / 16.0).floor(), (g.z / 16.0).floor());
            if let Some(layout) = &loaded.scene.layout {
                if cx >= 0.0 && cy >= 0.0 {
                    if let Some(c) = layout.cells.iter().find(|c| c.x == cx as usize && c.y == cy as usize) {
                        text.push_str(&format!(" · chunk {} at {}, {}", c.chunk_id, c.x, c.y));
                        if let Some(z) = c.zone {
                            text.push_str(&format!(" · {}", world.zone_label(z)));
                        }
                        if c.hidden {
                            text.push_str(" (hidden)");
                        }
                        cell = Some((c.x, c.y));
                    }
                }
            }
            self.hover = text;
        }
        if let Some(i) = hit_marker {
            self.hover = format!("{} · {}", loaded.scene.markers[i].label, self.hover);
        }
        if response.clicked() {
            self.selection = match (hit_marker, cell) {
                (Some(i), _) => Some(Selection::Marker(i)),
                (None, Some((x, y))) => Some(Selection::Cell { x, y }),
                _ => None,
            };
        }
    }

    fn details(&mut self, ui: &mut egui::Ui, world: &World, to_load: &mut Option<(usize, bool)>) {
        let Some(loaded) = &self.loaded else {
            ui.label("Pick a map on the left.");
            return;
        };
        let entry = &self.entries[loaded.entry];
        let scene = &loaded.scene;
        ui.add_space(4.0);
        ui.heading(&entry.title);
        ui.label(egui::RichText::new(&entry.detail).weak());
        ui.add_space(6.0);

        let mut focus: Option<(Vec3, f32)> = None;
        let mut follow: Option<(u16, u16)> = None;

        egui::Grid::new("map_stats").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(egui::RichText::new(k).weak());
                ui.label(v);
                ui.end_row();
            };
            if let Some(l) = &scene.layout {
                let hidden = l.cells.iter().filter(|c| c.hidden).count();
                let note = if hidden > 0 { format!(", {hidden} hidden") } else { String::new() };
                row("Chunks", format!("{} ({} x {}){note}", l.cells.len(), l.width, l.height));
            }
            row("Triangles", format!("{}", scene.triangle_count()));
            row("Placed models", format!("{}", scene.nodes.len()));
            row("Textures", format!("{}", scene.textures.len()));
            let count = |k: MarkerKind| scene.markers.iter().filter(|m| m.kind == k).count();
            row("NPCs", format!("{}", count(MarkerKind::Npc)));
            row("Warps", format!("{}", count(MarkerKind::Warp)));
            row("Triggers", format!("{}", count(MarkerKind::Trigger)));
            row("Interactables", format!("{}", count(MarkerKind::Furniture)));
        });

        if let Some(sel) = &self.selection {
            ui.add_space(8.0);
            ui.separator();
            match sel {
                Selection::Marker(i) => {
                    if let Some(m) = scene.markers.get(*i) {
                        marker_details(ui, world, m, &mut focus, &mut follow);
                    }
                }
                Selection::Cell { x, y } => {
                    let cell = scene.layout.as_ref().and_then(|l| l.cells.iter().find(|c| c.x == *x && c.y == *y));
                    if let Some(c) = cell {
                        ui.strong(format!("Chunk {}", c.chunk_id));
                        ui.label(format!("Cell {}, {} of matrix {}", c.x, c.y, entry.matrix));
                        if let Some(z) = c.zone.or(entry.zone) {
                            zone_details(ui, world, z, self.season);
                        }
                    }
                }
            }
        }

        let mut toggle_zone: Option<u32> = None;
        let mut toggle_unassigned = false;
        let mut show_all = false;
        if loaded.zone_centres.len() > 1 {
            ui.add_space(8.0);
            ui.separator();
            let hidden = &self.hidden_zones;
            let hide_unassigned = self.hide_unassigned;
            let title = if hidden.is_empty() {
                format!("Zones on this map ({})", loaded.zone_centres.len())
            } else {
                format!("Zones on this map ({}, {} hidden)", loaded.zone_centres.len(), hidden.len())
            };
            // Cells that no zone owns: the sea and forest between the zones.
            let unassigned = loaded
                .scene
                .layout
                .as_ref()
                .map_or(0, |l| l.cells.iter().filter(|c| c.zone.is_none()).count());
            egui::CollapsingHeader::new(title).id_salt("zones_on_map").default_open(true).show(ui, |ui| {
                if (!hidden.is_empty() || hide_unassigned) && ui.small_button("Show all").clicked() {
                    show_all = true;
                }
                if unassigned > 0 {
                    ui.horizontal(|ui| {
                        if crate::widgets::eye_button(ui, !hide_unassigned, "the cells without a zone").clicked() {
                            toggle_unassigned = true;
                        }
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        let color = Color32::from_gray(120);
                        ui.painter().rect_filled(dot, 2.0, if hide_unassigned { color.gamma_multiply(0.35) } else { color });
                        let label = egui::RichText::new(format!("Without a zone ({unassigned} cells)"));
                        ui.label(if hide_unassigned { label.weak() } else { label })
                            .on_hover_text("Cells of the map that belong to no zone: the sea and forest between the zones.");
                    });
                }
                for (z, centre) in &loaded.zone_centres {
                    let visible = !hidden.contains(z);
                    ui.horizontal(|ui| {
                        if crate::widgets::eye_button(ui, visible, "this zone").clicked() {
                            toggle_zone = Some(*z);
                        }
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        let color = zone_color(*z);
                        ui.painter().rect_filled(dot, 2.0, if visible { color } else { color.gamma_multiply(0.35) });
                        let label = egui::RichText::new(world.zone_label(*z));
                        if ui.link(if visible { label } else { label.weak() }).clicked() {
                            focus = Some((*centre, 900.0));
                        }
                    });
                }
            });
        } else if let Some(z) = entry.zone.or(loaded.zone_centres.first().map(|z| z.0)) {
            ui.add_space(8.0);
            ui.separator();
            zone_details(ui, world, z, self.season);
        }

        if !scene.warnings.is_empty() {
            ui.add_space(8.0);
            egui::CollapsingHeader::new(format!("Notes from loading ({})", scene.warnings.len())).show(ui, |ui| {
                for w in &scene.warnings {
                    ui.label(egui::RichText::new(w).small());
                }
            });
        }

        let open_entry = loaded.entry;
        if let Some((p, h)) = focus {
            self.focus(p, h);
        }
        if show_all || toggle_unassigned || toggle_zone.is_some() {
            if show_all {
                self.hidden_zones.clear();
                self.hide_unassigned = false;
            }
            if toggle_unassigned {
                self.hide_unassigned = !self.hide_unassigned;
            }
            if let Some(z) = toggle_zone {
                match self.hidden_zones.iter().position(|h| *h == z) {
                    Some(i) => {
                        self.hidden_zones.remove(i);
                    }
                    None => self.hidden_zones.push(z),
                }
            }
            // Markers are numbered per build, so a selected one would change.
            if matches!(self.selection, Some(Selection::Marker(_))) {
                self.selection = None;
            }
            *to_load = Some((open_entry, false));
        }
        if let Some((zone, warp)) = follow {
            if let Some(e) = self.entry_for_zone(world, zone as u32) {
                self.pending_focus = Some((zone as u32, warp));
                *to_load = Some((e, true));
            }
        }
    }
}

fn marker_details(
    ui: &mut egui::Ui,
    world: &World,
    m: &Marker,
    focus: &mut Option<(Vec3, f32)>,
    follow: &mut Option<(u16, u16)>,
) {
    ui.strong(&m.label);
    ui.label(format!("{} {} of {}", m.kind.label(), m.index, world.zone_label(m.zone)));
    ui.label(format!(
        "Position x {:.0}, y {:.0}, z {:.0} (tile {}, {})",
        m.pos.x,
        m.pos.y,
        m.pos.z,
        (m.pos.x / 16.0).floor(),
        (m.pos.z / 16.0).floor()
    ));
    ui.horizontal(|ui| {
        if ui.button("Centre view").clicked() {
            *focus = Some((m.pos, 400.0));
        }
        if let Some((zone, warp)) = m.target {
            if (zone as usize) < world.zones.len() && ui.button("Follow warp").clicked() {
                *follow = Some((zone, warp));
            }
        }
    });
}

fn zone_details(ui: &mut egui::Ui, world: &World, zone: u32, season: u8) {
    let Some(h) = world.zones.get(zone as usize) else { return };
    ui.strong(world.zone_label(zone));
    egui::Grid::new(("zone_grid", zone)).num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(egui::RichText::new(k).weak());
            ui.label(v);
            ui.end_row();
        };
        row("Matrix", h.matrix_id.to_string());
        let area = world.seasonal_area(zone, season);
        row("Area", match area {
            Some(a) if a != h.area_id as usize => format!("{} (seasonal: {a})", h.area_id),
            _ => h.area_id.to_string(),
        });
        if let Some(a) = area.and_then(|a| world.areas.get(a)) {
            row("Texture set", a.textures_id.to_string());
            row("Kind", if a.is_exterior { "Exterior".into() } else { "Interior".into() });
            row("Light file", a.light_index.to_string());
        }
        row("Scripts", h.scripts_id.to_string());
        row("Text file", h.text_file_id.to_string());
        row("Entities", h.entities_id.to_string());
        row("Encounters", if h.encounter_id < 0 { "none".into() } else { h.encounter_id.to_string() });
        row("Parent zone", h.parent_zone_id.to_string());
        row("Map type", h.map_type.to_string());
        row("Weather", h.weather.to_string());
        row("Music (spring)", h.bgm_spring.to_string());
        row("Can fly", yes_no(h.can_fly));
        row("Can cycle", yes_no(h.can_cycle));
    });
}

fn yes_no(v: bool) -> String {
    if v { "yes".into() } else { "no".into() }
}

/// Stable, well-spread colour for a zone number.
pub fn zone_color(zone: u32) -> Color32 {
    let h = (zone as f32 * 0.618_034) % 1.0;
    let c: Color32 = egui::ecolor::Hsva::new(h, 0.55, 0.85, 1.0).into();
    c
}
