//! The desktop window. Everything it shows comes from `vmap-world`, and all
//! drawing goes through `vmap-render`; this crate only arranges panels and
//! handles input.

mod asset_tab;
mod map_tab;
mod viewport;
mod widgets;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use eframe::egui;
use vmap_fs::SourceKind;
use vmap_world::World;

use asset_tab::AssetTab;
use map_tab::MapTab;
use viewport::Gfx;

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Map,
    Assets,
}

struct Session {
    world: World,
    map: MapTab,
    assets: AssetTab,
}

struct App {
    gfx: Gfx,
    session: Option<Session>,
    tab: Tab,
    error: Option<String>,
    pending_open: Option<PathBuf>,
    script: Option<UiScript>,
    frame: u32,
    shot_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Drives the window without a person: selects something, saves a picture of
/// the window and exits. Used to check the interface automatically.
#[derive(Debug, Clone, Default)]
pub struct UiScript {
    /// Where to save the picture of the window.
    pub screenshot: PathBuf,
    /// Show the asset tab instead of the map tab.
    pub assets: bool,
    /// Text to search the map list for; the first match is opened.
    pub map: Option<String>,
    pub top_down: bool,
    /// Archive number, file index and optional sub-file to preview.
    pub asset: Option<(u32, usize, Option<usize>)>,
    /// Select this warp of the opened map (counting from 0).
    pub warp: Option<usize>,
    /// Go where the selected warp leads.
    pub follow: bool,
    /// Zones of the opened map to hide.
    pub hide_zones: Vec<u32>,
    /// Hide the cells of the opened map that belong to no zone.
    pub hide_unassigned: bool,
    /// Places to click with the mouse, one after the other, as pixels of
    /// the screenshot. They are clicked after everything above is done.
    pub clicks: Vec<[f32; 2]>,
}

/// Opens the viewer window. `initial` is a game to open straight away;
/// without it the last opened game is used.
pub fn run(initial: Option<PathBuf>) -> Result<()> {
    run_with(initial, None)
}

/// Like [`run`], optionally driven by a [`UiScript`].
pub fn run_with(initial: Option<PathBuf>, script: Option<UiScript>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("vmap").with_inner_size([1500.0, 920.0]),
        ..Default::default()
    };
    let initial = initial.or_else(last_opened);
    eframe::run_native(
        "vmap",
        options,
        Box::new(move |cc| {
            let rs = cc
                .wgpu_render_state
                .clone()
                .ok_or("this window was created without a wgpu renderer")?;
            let renderer = vmap_render::Renderer::new(&rs.device);
            add_fallback_font(&cc.egui_ctx);
            Ok(Box::new(App {
                gfx: Gfx { rs, renderer },
                session: None,
                tab: Tab::Map,
                error: None,
                pending_open: initial,
                script,
                frame: 0,
                shot_done: Default::default(),
            }))
        }),
    )
    .map_err(|e| anyhow!("the window could not be opened: {e}"))
}

/// Game text uses characters the built-in fonts lack (Japanese, full-width
/// forms), so one of the system's wide-coverage fonts is added as a fallback.
fn add_fallback_font(ctx: &egui::Context) {
    use egui::epaint::text::{FontData, FontFamily, FontInsert, FontPriority, InsertFontFamily};
    let windows = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:/Windows")).join("Fonts");
    let candidates = [
        windows.join("YuGothM.ttc"),
        windows.join("meiryo.ttc"),
        windows.join("msgothic.ttc"),
        PathBuf::from("/System/Library/Fonts/Hiragino Sans GB.ttc"),
        PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc"),
    ];
    let Some(data) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else { return };
    let family = |family| InsertFontFamily { family, priority: FontPriority::Lowest };
    ctx.add_font(FontInsert::new(
        "system-fallback",
        FontData::from_owned(data),
        vec![family(FontFamily::Proportional), family(FontFamily::Monospace)],
    ));
}

fn settings_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("XDG_CONFIG_HOME")).map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
    })?;
    Some(base.join("vmap").join("last_opened.txt"))
}

fn last_opened() -> Option<PathBuf> {
    let text = std::fs::read_to_string(settings_file()?).ok()?;
    let path = PathBuf::from(text.trim());
    path.exists().then_some(path)
}

fn remember(path: &Path) {
    if let Some(file) = settings_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, path.to_string_lossy().as_bytes());
    }
}

impl App {
    fn open(&mut self, path: &Path) {
        match World::open(path) {
            Ok(world) => {
                let mut map = MapTab::new(&world);
                map.open_first(&world, &self.gfx);
                let assets = AssetTab::new(&world);
                self.session = Some(Session { world, map, assets });
                self.error = None;
                remember(path);
            }
            Err(e) => self.error = Some(format!("Could not open {}: {e:#}", path.display())),
        }
    }

    /// Reads the game again and shows what was on screen before.
    fn reload(&mut self, ctx: &egui::Context) {
        let Some(old) = self.session.take() else { return };
        let path = old.world.fs.opened_path().to_path_buf();
        match World::open(&path) {
            Ok(world) => {
                let mut map = MapTab::new(&world);
                map.restore_from(&old.map, &world, &self.gfx);
                let mut assets = old.assets;
                assets.reload(&world, &self.gfx, ctx);
                self.session = Some(Session { world, map, assets });
                self.error = None;
            }
            Err(e) => {
                self.error = Some(format!("Could not read {} again: {e:#}", path.display()));
                self.session = Some(old);
            }
        }
    }

    fn open_buttons(&mut self, ui: &mut egui::Ui) {
        if ui.button("Open project or ROM…").clicked() {
            let picked = rfd::FileDialog::new()
                .add_filter("CTRMap project or DS ROM", &["cmproj", "nds"])
                .add_filter("All files", &["*"])
                .pick_file();
            if let Some(p) = picked {
                self.pending_open = Some(p);
            }
        }
        if ui.button("Open extracted folder…").clicked() {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                self.pending_open = Some(p);
            }
        }
    }
}

impl App {
    fn run_script(&mut self, ctx: &egui::Context) {
        let Some(script) = self.script.clone() else { return };
        self.frame += 1;
        ctx.request_repaint();
        if self.frame == 3 {
            if let Some(s) = &mut self.session {
                if script.assets {
                    self.tab = Tab::Assets;
                }
                if let Some(text) = &script.map {
                    s.map.open_matching(&s.world, &self.gfx, text);
                }
                if script.top_down {
                    s.map.set_top_down(true);
                }
                if !script.hide_zones.is_empty() || script.hide_unassigned {
                    s.map.script_hide(&s.world, &self.gfx, &script.hide_zones, script.hide_unassigned);
                }
                if let Some(n) = script.warp {
                    s.map.script_warp(&s.world, &self.gfx, n, script.follow);
                }
                if let Some((archive, member, sub)) = script.asset {
                    s.assets.select(&s.world, &self.gfx, ctx, archive, member, sub);
                }
            }
        }
        if self.frame == Self::FIRST_CLICK_FRAME + script.clicks.len() as u32 * Self::CLICK_FRAMES + 12 {
            let done = self.shot_done.clone();
            let path = script.screenshot.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let _ = image::save_buffer(&path, img.as_raw(), img.size[0] as u32, img.size[1] as u32, image::ColorType::Rgba8);
                done.store(true, std::sync::atomic::Ordering::SeqCst);
            })));
        }
        if self.shot_done.load(std::sync::atomic::Ordering::SeqCst) || self.frame > 600 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl App {
    /// Frame of a scripted run at which the first click starts, and how
    /// many frames each click takes (move, press, release, settle).
    const FIRST_CLICK_FRAME: u32 = 8;
    const CLICK_FRAMES: u32 = 6;
}

impl eframe::App for App {
    /// Feeds the mouse clicks of a scripted run to the window, as if
    /// they came from the mouse.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let Some(script) = &self.script else { return };
        let Some(since) = self.frame.checked_sub(Self::FIRST_CLICK_FRAME) else { return };
        let Some(&[x, y]) = script.clicks.get((since / Self::CLICK_FRAMES) as usize) else { return };
        let scale = ctx.pixels_per_point();
        let pos = egui::pos2(x / scale, y / scale);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        match since % Self::CLICK_FRAMES {
            0 => raw_input.events.push(egui::Event::PointerMoved(pos)),
            2 => raw_input.events.push(button(true)),
            3 => raw_input.events.push(button(false)),
            _ => {}
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(path) = self.pending_open.take() {
            self.open(&path);
        }
        let ctx = ui.ctx().clone();
        self.run_script(&ctx);

        let mut reload = false;
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Map, "Map");
                ui.selectable_value(&mut self.tab, Tab::Assets, "Assets");
                ui.separator();
                self.open_buttons(ui);
                if let Some(s) = &self.session {
                    if ui.button("Reload").on_hover_text("Read the files again, for example after saving in CTRMap").clicked() {
                        reload = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let source = match s.world.fs.kind() {
                            SourceKind::CtrmapProject => "CTRMap project",
                            SourceKind::ExtractedFolder => "extracted folder",
                            SourceKind::RomImage => "ROM image",
                        };
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {source} · {}",
                                s.world.game.name(),
                                s.world.fs.opened_path().display()
                            ))
                            .weak(),
                        );
                    });
                }
            });
            ui.add_space(3.0);
        });

        if reload {
            self.reload(&ctx);
        }

        if let Some(err) = &self.error {
            egui::Panel::top("error_bar").show(ui, |ui| {
                ui.colored_label(ui.visuals().error_fg_color, err);
            });
        }

        match &mut self.session {
            Some(s) => match self.tab {
                Tab::Map => s.map.ui(ui, &s.world, &self.gfx),
                Tab::Assets => s.assets.ui(ui, &s.world, &self.gfx),
            },
            None => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.3);
                        ui.heading("vmap");
                        ui.label("Open a CTRMap project (.cmproj), a DS ROM (.nds) or an extracted ROM folder.");
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.add_space((ui.available_width() - 360.0).max(0.0) * 0.5);
                            self.open_buttons(ui);
                        });
                    });
                });
            }
        }
    }
}
