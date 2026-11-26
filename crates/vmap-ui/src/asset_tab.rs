//! The asset viewer: browse every archive of the game and preview models,
//! textures, containers and text.

use std::collections::HashMap;

use eframe::egui;
use vmap_formats::gen5::TextFile;
use vmap_formats::gf::GfContainer;
use vmap_formats::nds::compress;
use vmap_formats::nitro::{Mdl0, NitroFile, Tex0};
use vmap_render::GpuScene;
use vmap_world::{ArchiveInfo, FileKind, Scene, World};

use crate::viewport::{Gfx, Viewport};

#[derive(Clone)]
struct MemberInfo {
    kind: FileKind,
    size: usize,
    modified: bool,
}

struct TexturePreview {
    name: String,
    info: String,
    palette: String,
    handle: egui::TextureHandle,
    size: [usize; 2],
}

struct ModelPreview {
    scene: Scene,
    gpu: GpuScene,
    generation: u64,
    info: Vec<String>,
}

enum Preview {
    None,
    Message(String),
    Model(Box<ModelPreview>),
    Textures(Vec<TexturePreview>),
    Text(Vec<String>),
    /// Plain lines describing a file.
    Info(Vec<String>),
    Hex(String),
}

/// A file inside a container that can itself be previewed.
struct SubFile {
    label: String,
    kind: FileKind,
    size: usize,
}

pub struct AssetTab {
    archives: Vec<ArchiveInfo>,
    archive_filter: String,
    selected_archive: Option<usize>,
    members: Vec<Option<MemberInfo>>,
    kind_filter: Option<FileKind>,
    only_modified: bool,
    selected_member: Option<usize>,
    /// Sub-files of the selected member when it is a container.
    sub_files: Vec<SubFile>,
    selected_sub: Option<usize>,
    preview: Preview,
    header: String,
    viewport: Viewport,
    texture_zoom: f32,
    animate: bool,
    generation: u64,
    chunk_areas: Option<HashMap<u32, usize>>,
}

impl AssetTab {
    pub fn new(world: &World) -> Self {
        let mut archives = world.archives();
        // Archives with a known role first, then the rest in order.
        archives.sort_by_key(|a| (a.known.is_none(), a.id));
        let mut viewport = Viewport::default();
        viewport.options.markers = false;
        viewport.options.background = Some([0.16, 0.17, 0.19]);
        Self {
            archives,
            archive_filter: String::new(),
            selected_archive: None,
            members: Vec::new(),
            kind_filter: None,
            only_modified: false,
            selected_member: None,
            sub_files: Vec::new(),
            selected_sub: None,
            preview: Preview::None,
            header: String::new(),
            viewport,
            texture_zoom: 2.0,
            animate: true,
            generation: 0,
            chunk_areas: None,
        }
    }

    pub fn reload(&mut self, world: &World, gfx: &Gfx, ctx: &egui::Context) {
        let (archive, member, sub) = (self.selected_archive, self.selected_member, self.selected_sub);
        *self = Self::new(world);
        if let Some(a) = archive {
            self.select_archive(world, a);
            if let Some(m) = member {
                self.select_member(world, gfx, ctx, m, sub);
            }
        }
    }

    /// Selects a file by archive number and index.
    pub fn select(&mut self, world: &World, gfx: &Gfx, ctx: &egui::Context, archive: u32, member: usize, sub: Option<usize>) {
        if let Some(i) = self.archives.iter().position(|a| a.id == archive) {
            self.select_archive(world, i);
            self.select_member(world, gfx, ctx, member, sub);
        }
    }

    fn select_archive(&mut self, _world: &World, index: usize) {
        self.selected_archive = Some(index);
        self.members = vec![None; self.archives[index].len];
        self.selected_member = None;
        self.sub_files.clear();
        self.selected_sub = None;
        self.preview = Preview::None;
        self.header.clear();
    }

    fn member_info(&mut self, world: &World, index: usize) -> Option<MemberInfo> {
        let archive = &self.archives[self.selected_archive?];
        if let Some(Some(info)) = self.members.get(index) {
            return Some(info.clone());
        }
        let info = match world.archive_member(archive.id, index) {
            Some(data) => MemberInfo {
                kind: world.member_kind(archive.id, &data),
                size: data.len(),
                modified: world.fs.is_overridden_path(&archive.path, index),
            },
            None => MemberInfo { kind: FileKind::Empty, size: 0, modified: false },
        };
        if let Some(slot) = self.members.get_mut(index) {
            *slot = Some(info.clone());
        }
        Some(info)
    }

    fn select_member(&mut self, world: &World, gfx: &Gfx, ctx: &egui::Context, index: usize, sub: Option<usize>) {
        let Some(ai) = self.selected_archive else { return };
        let archive = self.archives[ai].clone();
        self.selected_member = Some(index);
        self.sub_files.clear();
        self.selected_sub = None;
        let Some(data) = world.archive_member(archive.id, index) else {
            self.preview = Preview::Message("This file could not be read.".into());
            return;
        };
        let mut kind = world.member_kind(archive.id, &data);
        let modified = world.fs.is_overridden_path(&archive.path, index);
        self.header = format!(
            "{} #{index} · {} · {}{}",
            archive.path,
            kind.label(),
            human_size(data.len()),
            if modified { " · changed by the project" } else { "" }
        );

        // Containers: list what is inside and preview one sub-file.
        let mut bytes: Vec<u8> = data.to_vec();
        if kind == FileKind::Compressed {
            if let Ok(d) = compress::lz_decompress(&bytes) {
                bytes = d;
                kind = world.member_kind(archive.id, &bytes);
                self.header.push_str(&format!(" · unpacked: {}, {}", kind.label(), human_size(bytes.len())));
            }
        }
        if let FileKind::Container(_) = &kind {
            if let Ok(c) = GfContainer::parse(&bytes) {
                for (i, r) in c.files.iter().enumerate() {
                    let sub = &bytes[r.clone()];
                    let sub_kind = FileKind::sniff(sub);
                    // Models are easier to tell apart by name.
                    let name = (sub_kind == FileKind::Model)
                        .then(|| Mdl0::from_bmd(sub).ok().and_then(|m| m.models.first().map(|m| m.name.clone())))
                        .flatten();
                    let label = match name {
                        Some(n) => format!("{i}: {n}"),
                        None => format!("{i}: {}", sub_kind.label()),
                    };
                    self.sub_files.push(SubFile { label, kind: sub_kind, size: r.len() });
                }
                let pick = sub
                    .filter(|s| *s < self.sub_files.len())
                    .or_else(|| self.sub_files.iter().position(|s| s.kind == FileKind::Model))
                    .or_else(|| self.sub_files.iter().position(|s| s.kind == FileKind::Textures))
                    .or((!self.sub_files.is_empty()).then_some(0));
                if let Some(s) = pick {
                    self.selected_sub = Some(s);
                    let sub_bytes = bytes[c.files[s].clone()].to_vec();
                    let sub_kind = self.sub_files[s].kind.clone();
                    self.preview = self.make_preview(world, gfx, ctx, &archive, index, &sub_kind, &sub_bytes, Some(&bytes));
                    return;
                }
            }
        }
        self.preview = self.make_preview(world, gfx, ctx, &archive, index, &kind, &bytes, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn make_preview(
        &mut self,
        world: &World,
        gfx: &Gfx,
        ctx: &egui::Context,
        archive: &ArchiveInfo,
        index: usize,
        kind: &FileKind,
        data: &[u8],
        container: Option<&[u8]>,
    ) -> Preview {
        match kind {
            FileKind::Model => {
                let areas = self.chunk_areas.get_or_insert_with(|| world.chunk_areas());
                let extra = world.textures_for(archive.id, index, areas);
                let refs: Vec<&Tex0> = extra.iter().collect();
                // A building inside a bundle brings its animations along.
                let built = container
                    .and_then(|c| world.build_building_scene(c, data, &refs).ok())
                    .map(Ok)
                    .unwrap_or_else(|| world.build_model_scene(data, &refs));
                match built {
                    Ok(scene) => {
                        self.generation += 1;
                        let gpu = GpuScene::new(&gfx.rs.device, &gfx.rs.queue, &gfx.renderer, &scene);
                        self.viewport.camera = vmap_render::OrbitCamera { pitch: 0.5, yaw: 0.6, ..Default::default() };
                        self.viewport.camera.frame(scene.min, scene.max, self.viewport.aspect());
                        self.viewport.camera.distance *= 1.4;
                        self.viewport.ground_y = scene.center().y;
                        let mut info = Vec::new();
                        if let Ok(mdl) = Mdl0::from_bmd(data) {
                            for m in &mdl.models {
                                info.push(format!(
                                    "Model \"{}\": {} triangles, {} quads, {} joints, {} materials, {} meshes",
                                    m.name,
                                    m.triangle_count,
                                    m.quad_count,
                                    m.nodes.len(),
                                    m.materials.len(),
                                    m.shapes.len()
                                ));
                                for mat in &m.materials {
                                    info.push(format!(
                                        "   {}: texture {}, palette {}",
                                        mat.name,
                                        mat.texture.as_deref().unwrap_or("none"),
                                        mat.palette.as_deref().unwrap_or("none")
                                    ));
                                }
                            }
                            if let Some(t) = &mdl.textures {
                                info.push(format!("Embedded textures: {}", t.textures.len()));
                            }
                        }
                        let a = &scene.animations;
                        if !a.is_empty() {
                            info.push(format!(
                                "Animated: {} sliding textures, {} swapping textures, {} sets of moving joints",
                                a.tex_srt.len(),
                                a.tex_pattern.len(),
                                a.joints.len()
                            ));
                        }
                        let missing = scene.materials.iter().filter(|m| m.texture.is_none()).count();
                        if missing > 0 {
                            info.push(format!("{missing} of {} materials are drawn without a texture.", scene.materials.len()));
                        }
                        Preview::Model(Box::new(ModelPreview { scene, gpu, generation: self.generation, info }))
                    }
                    Err(e) => Preview::Message(format!("This model could not be read: {e:#}")),
                }
            }
            FileKind::Textures => match Tex0::from_btx(data) {
                Ok(tex) => Preview::Textures(texture_previews(ctx, &tex, &format!("{}#{index}", archive.path))),
                Err(e) => Preview::Message(format!("These textures could not be read: {e:#}")),
            },
            FileKind::Text => match TextFile::parse(data) {
                Ok(t) => Preview::Text(
                    t.sections.iter().flat_map(|s| s.iter().cloned()).collect(),
                ),
                Err(e) => Preview::Message(format!("This text file could not be read: {e:#}")),
            },
            FileKind::JointAnimation | FileKind::TextureSrtAnimation | FileKind::TexturePatternAnimation => {
                let mut lines = vmap_world::catalog::describe_animation(kind, data);
                lines.push(String::new());
                lines.push("Animations play on the model they belong to: open a map, or a building inside a".into());
                lines.push("building bundle, to see them move.".into());
                Preview::Info(lines)
            }
            FileKind::Empty => Preview::Message("This file is empty.".into()),
            _ => {
                let mut text = String::new();
                if let Ok(f) = NitroFile::parse(data) {
                    if NitroFile::sniff(data).is_some() {
                        let blocks: Vec<String> =
                            f.blocks.iter().map(|(m, b)| format!("{} ({})", String::from_utf8_lossy(m), human_size(b.len()))).collect();
                        text.push_str(&format!("Blocks: {}\n\n", blocks.join(", ")));
                    }
                }
                text.push_str(&hex_dump(data, 1024));
                Preview::Hex(text)
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, world: &World, gfx: &Gfx) {
        let ctx = ui.ctx().clone();
        let mut pick_archive = None;
        let mut pick_member: Option<(usize, Option<usize>)> = None;

        egui::Panel::left("asset_archives").default_size(250.0).resizable(true).show(ui, |ui| {
            ui.add_space(4.0);
            ui.add(egui::TextEdit::singleline(&mut self.archive_filter).hint_text("Search archives").desired_width(f32::INFINITY));
            ui.add_space(4.0);
            let filter = self.archive_filter.to_lowercase();
            let visible: Vec<usize> = (0..self.archives.len())
                .filter(|&i| filter.is_empty() || self.archives[i].title().to_lowercase().contains(&filter))
                .collect();
            let row = crate::widgets::row_height(true);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row, visible.len(), |ui, range| {
                for &i in &visible[range] {
                    let a = &self.archives[i];
                    let title = match a.known {
                        Some(k) => k.label().to_string(),
                        None => format!("Archive {}", a.id),
                    };
                    let mut detail = format!("{} · {} files", a.path, a.len);
                    if a.modified > 0 {
                        detail.push_str(&format!(" · {} changed", a.modified));
                    }
                    if crate::widgets::list_row(ui, self.selected_archive == Some(i), &title, Some(&detail), None, row).clicked() {
                        pick_archive = Some(i);
                    }
                }
            });
        });

        egui::Panel::left("asset_members").default_size(310.0).resizable(true).show(ui, |ui| {
            ui.add_space(4.0);
            let Some(ai) = self.selected_archive else {
                ui.label("Pick an archive.");
                return;
            };
            let archive = self.archives[ai].clone();
            ui.strong(archive.title());
            ui.label(
                egui::RichText::new(format!(
                    "{} files{}",
                    archive.len,
                    if archive.modified > 0 { format!(", {} changed by the project", archive.modified) } else { String::new() }
                ))
                .weak(),
            );
            ui.horizontal(|ui| {
                let label = self.kind_filter.as_ref().map(|k| k.label()).unwrap_or("All kinds".into());
                egui::ComboBox::from_id_salt("kind_filter").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.kind_filter, None, "All kinds");
                    for k in [FileKind::Model, FileKind::Textures, FileKind::JointAnimation, FileKind::Text] {
                        let l = k.label();
                        ui.selectable_value(&mut self.kind_filter, Some(k), l);
                    }
                });
                ui.checkbox(&mut self.only_modified, "Changed");
            });
            ui.add_space(2.0);

            // Filtering needs the kind of every file, so classify on demand.
            let filtering = self.kind_filter.is_some() || self.only_modified;
            let visible: Vec<usize> = if filtering {
                let overridden = world.fs.overridden_members(&archive.path);
                let wanted = self.kind_filter.clone();
                let only_modified = self.only_modified;
                let mut list = Vec::new();
                for i in 0..archive.len {
                    if only_modified && !overridden.contains(&i) {
                        continue;
                    }
                    if let Some(k) = &wanted {
                        if !self.member_info(world, i).is_some_and(|m| container_matches(&m.kind, k)) {
                            continue;
                        }
                    }
                    list.push(i);
                }
                list
            } else {
                (0..archive.len).collect()
            };
            let row = crate::widgets::row_height(false);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row, visible.len(), |ui, range| {
                for &i in &visible[range] {
                    let Some(info) = self.member_info(world, i) else { continue };
                    let text = format!("{i}  ·  {}  ·  {}", info.kind.label(), human_size(info.size));
                    let tag = info.modified.then_some("changed");
                    if crate::widgets::list_row(ui, self.selected_member == Some(i), &text, None, tag, row).clicked() {
                        pick_member = Some((i, None));
                    }
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if self.selected_member.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label("Pick a file to preview it. Files your project has changed are marked \"changed\".");
                });
                return;
            }
            ui.label(egui::RichText::new(&self.header).strong());
            if !self.sub_files.is_empty() {
                let current = self.selected_sub;
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Inside:").weak());
                    if self.sub_files.len() <= 8 {
                        for (i, s) in self.sub_files.iter().enumerate() {
                            let text = format!("{} ({})", s.label, human_size(s.size));
                            if ui.selectable_label(current == Some(i), text).clicked() {
                                pick_member = self.selected_member.map(|m| (m, Some(i)));
                            }
                        }
                        return;
                    }
                    // Many files: a list of the ones worth opening, with
                    // buttons to step through them.
                    let shown: Vec<usize> = (0..self.sub_files.len())
                        .filter(|&i| !matches!(self.sub_files[i].kind, FileKind::Unknown | FileKind::Empty))
                        .collect();
                    let label = current.and_then(|i| self.sub_files.get(i)).map(|s| s.label.clone()).unwrap_or_default();
                    let mut pick = None;
                    egui::ComboBox::from_id_salt("sub_file").selected_text(label).width(220.0).show_ui(ui, |ui| {
                        for &i in &shown {
                            let s = &self.sub_files[i];
                            let text = format!("{} ({})", s.label, human_size(s.size));
                            if ui.selectable_label(current == Some(i), text).clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                    let at = current.and_then(|c| shown.iter().position(|&i| i == c));
                    if ui.add_enabled(at.is_some_and(|p| p > 0), egui::Button::new("Previous")).clicked() {
                        pick = at.map(|p| shown[p - 1]);
                    }
                    if ui.add_enabled(at.is_some_and(|p| p + 1 < shown.len()), egui::Button::new("Next")).clicked() {
                        pick = at.map(|p| shown[p + 1]);
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{} of the {} files inside can be previewed",
                            shown.len(),
                            self.sub_files.len()
                        ))
                        .weak(),
                    );
                    if let Some(i) = pick {
                        pick_member = self.selected_member.map(|m| (m, Some(i)));
                    }
                });
            }
            ui.separator();
            // Advance animations in steps of one game frame.
            if let Preview::Model(p) = &mut self.preview {
                if self.animate && p.gpu.has_animations() {
                    let fps = vmap_world::ANIMATION_FPS as f64;
                    let time = ((ui.input(|i| i.time) * fps).floor() / fps) as f32;
                    p.gpu.animate(&gfx.rs.queue, time);
                    self.viewport.time = time;
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(1.0 / fps));
                }
            }
            match &self.preview {
                Preview::None => {}
                Preview::Message(m) => {
                    ui.label(m);
                }
                Preview::Model(p) => {
                    egui::Panel::bottom("model_info").resizable(true).default_size(120.0).show(ui, |ui| {
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            for line in &p.info {
                                ui.label(egui::RichText::new(line).monospace().small());
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.viewport.options.textures, "Textures");
                        ui.checkbox(&mut self.viewport.options.lighting, "Lighting");
                        ui.checkbox(&mut self.viewport.options.vertex_colors, "Vertex colours");
                        if p.gpu.has_animations() {
                            ui.checkbox(&mut self.animate, "Animations");
                        }
                        if ui.button("Fit").clicked() {
                            self.viewport.camera.frame(p.scene.min, p.scene.max, self.viewport.aspect());
                            self.viewport.camera.distance *= 1.4;
                        }
                        ui.label(egui::RichText::new("Left drag: move · Right drag: rotate · Wheel: zoom").weak().small());
                    });
                    self.viewport.show(ui, gfx, Some((&p.gpu, p.generation)));
                }
                Preview::Textures(list) => {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} textures", list.len()));
                        ui.add(egui::Slider::new(&mut self.texture_zoom, 1.0..=8.0).text("zoom").step_by(1.0));
                    });
                    let zoom = self.texture_zoom;
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        // Flow layout: fill each row up to the panel's width.
                        const GAP: f32 = 14.0;
                        const LABEL_H: f32 = 44.0;
                        let cell_w = |t: &TexturePreview| (t.size[0] as f32 * zoom).max(132.0);
                        let width = ui.available_width();
                        let mut rows: Vec<std::ops::Range<usize>> = Vec::new();
                        let (mut start, mut used) = (0, 0.0f32);
                        for (i, t) in list.iter().enumerate() {
                            let w = cell_w(t) + GAP;
                            if i > start && used + w > width {
                                rows.push(start..i);
                                (start, used) = (i, 0.0);
                            }
                            used += w;
                        }
                        if start < list.len() {
                            rows.push(start..list.len());
                        }
                        for row in rows {
                            let items = &list[row];
                            let row_h = items.iter().map(|t| t.size[1] as f32 * zoom).fold(0.0, f32::max) + LABEL_H;
                            let row_w: f32 = items.iter().map(|t| cell_w(t) + GAP).sum();
                            let (row_rect, _) = ui.allocate_exact_size(egui::vec2(row_w, row_h), egui::Sense::hover());
                            if !ui.is_rect_visible(row_rect) {
                                continue;
                            }
                            let mut x = row_rect.left();
                            for t in items {
                                let size = egui::vec2(t.size[0] as f32 * zoom, t.size[1] as f32 * zoom);
                                let rect = egui::Rect::from_min_size(egui::pos2(x, row_rect.top()), size);
                                checkerboard(ui.painter(), rect);
                                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                                ui.painter().image(t.handle.id(), rect, uv, egui::Color32::WHITE);
                                let cell = egui::Rect::from_min_size(
                                    egui::pos2(x, row_rect.top()),
                                    egui::vec2(cell_w(t), row_h),
                                );
                                let text = ui.painter().with_clip_rect(cell);
                                let y = rect.bottom() + 3.0;
                                text.text(
                                    egui::pos2(x, y),
                                    egui::Align2::LEFT_TOP,
                                    &t.name,
                                    egui::FontId::proportional(11.5),
                                    ui.visuals().strong_text_color(),
                                );
                                text.text(
                                    egui::pos2(x, y + 13.0),
                                    egui::Align2::LEFT_TOP,
                                    &t.info,
                                    egui::FontId::proportional(10.5),
                                    ui.visuals().weak_text_color(),
                                );
                                text.text(
                                    egui::pos2(x, y + 25.0),
                                    egui::Align2::LEFT_TOP,
                                    &t.palette,
                                    egui::FontId::proportional(10.5),
                                    ui.visuals().weak_text_color(),
                                );
                                x += cell_w(t) + GAP;
                            }
                        }
                    });
                }
                Preview::Text(lines) => {
                    let row = ui.text_style_height(&egui::TextStyle::Monospace);
                    egui::ScrollArea::both().auto_shrink([false, false]).show_rows(ui, row, lines.len(), |ui, range| {
                        for i in range {
                            let line = lines[i].replace('\n', " ⏎ ");
                            ui.label(egui::RichText::new(format!("{i:>4}  {line}")).monospace());
                        }
                    });
                }
                Preview::Info(lines) => {
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        for line in lines {
                            ui.label(egui::RichText::new(line).monospace());
                        }
                    });
                }
                Preview::Hex(text) => {
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        ui.label(egui::RichText::new(text).monospace());
                    });
                }
            }
        });

        if let Some(a) = pick_archive {
            self.select_archive(world, a);
        }
        if let Some((m, sub)) = pick_member {
            self.select_member(world, gfx, &ctx, m, sub);
        }
    }
}

/// A container counts as a match if it is what the filter asks for, or if
/// it is a container (which usually wraps a model).
fn container_matches(kind: &FileKind, wanted: &FileKind) -> bool {
    kind == wanted || (matches!(kind, FileKind::Container(_)) && *wanted == FileKind::Model)
}

fn texture_previews(ctx: &egui::Context, tex: &Tex0, key: &str) -> Vec<TexturePreview> {
    let mut out = Vec::new();
    for (i, t) in tex.textures.iter().enumerate() {
        let palette = tex.guess_palette(i);
        let Some(image) = tex.decode(i, palette) else { continue };
        let color = egui::ColorImage::from_rgba_unmultiplied([image.width as usize, image.height as usize], &image.rgba);
        let handle = ctx.load_texture(format!("{key}/{}", t.name), color, egui::TextureOptions::NEAREST);
        let pal = palette.and_then(|p| tex.palettes.get(p)).map(|p| p.name.as_str()).unwrap_or("none");
        out.push(TexturePreview {
            name: t.name.clone(),
            info: format!("{}x{} · {}", t.width, t.height, t.format.name()),
            palette: format!("palette {pal}"),
            handle,
            size: [image.width as usize, image.height as usize],
        });
    }
    out
}

fn checkerboard(painter: &egui::Painter, rect: egui::Rect) {
    let cell = 8.0;
    painter.rect_filled(rect, 0.0, egui::Color32::from_gray(60));
    let cols = (rect.width() / cell).ceil() as i32;
    let rows = (rect.height() / cell).ceil() as i32;
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 {
                let r = egui::Rect::from_min_size(rect.min + egui::vec2(x as f32 * cell, y as f32 * cell), egui::vec2(cell, cell));
                painter.rect_filled(r.intersect(rect), 0.0, egui::Color32::from_gray(85));
            }
        }
    }
}

pub fn human_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn hex_dump(data: &[u8], limit: usize) -> String {
    let mut out = String::new();
    for (row, chunk) in data.iter().take(limit).collect::<Vec<_>>().chunks(16).enumerate() {
        out.push_str(&format!("{:06X}  ", row * 16));
        for b in chunk {
            out.push_str(&format!("{b:02X} "));
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
        }
        out.push(' ');
        for b in chunk {
            out.push(if b.is_ascii_graphic() || **b == b' ' { **b as char } else { '.' });
        }
        out.push('\n');
    }
    if data.len() > limit {
        out.push_str(&format!("… {} more bytes\n", data.len() - limit));
    }
    out
}
