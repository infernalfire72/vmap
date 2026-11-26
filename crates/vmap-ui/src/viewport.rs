//! A 3D view embedded in the window: owns the offscreen render target and
//! turns mouse input into camera movement.

use eframe::egui;
use eframe::egui_wgpu;
use vmap_render::{wgpu, GpuScene, OrbitCamera, RenderOptions, RenderTarget, Renderer};

/// GPU objects shared by every view in the window.
pub struct Gfx {
    pub rs: egui_wgpu::RenderState,
    pub renderer: Renderer,
}

pub struct Viewport {
    pub camera: OrbitCamera,
    pub options: RenderOptions,
    target: Option<RenderTarget>,
    texture: Option<egui::TextureId>,
    /// State of the last rendered frame; rendering is skipped while nothing
    /// changes.
    rendered: Option<(OrbitCamera, RenderOptions, u64, f32)>,
    /// Animation time the scene was last put into; a change redraws.
    pub time: f32,
    /// Height the picking and panning plane sits at.
    pub ground_y: f32,
}

pub struct ViewportResponse {
    pub response: egui::Response,
    pub rect: egui::Rect,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            camera: OrbitCamera::default(),
            options: RenderOptions::default(),
            target: None,
            texture: None,
            rendered: None,
            time: 0.0,
            ground_y: 0.0,
        }
    }
}

impl Viewport {
    pub fn aspect(&self) -> f32 {
        self.target.as_ref().map(|t| t.aspect()).unwrap_or(1.6)
    }

    /// Normalised device coordinates (-1..=1, Y up) of a screen position.
    pub fn ndc(rect: egui::Rect, pos: egui::Pos2) -> [f32; 2] {
        [
            (pos.x - rect.left()) / rect.width().max(1.0) * 2.0 - 1.0,
            1.0 - (pos.y - rect.top()) / rect.height().max(1.0) * 2.0,
        ]
    }

    /// Screen position of a world point, if it is in front of the camera.
    pub fn project(&self, rect: egui::Rect, world: glam::Vec3) -> Option<egui::Pos2> {
        let clip = self.camera.camera(self.aspect()).view_proj() * world.extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.z < 0.0 || ndc.z > 1.0 {
            return None;
        }
        Some(egui::pos2(
            rect.left() + (ndc.x * 0.5 + 0.5) * rect.width(),
            rect.top() + (0.5 - ndc.y * 0.5) * rect.height(),
        ))
    }

    /// Ground point under a screen position.
    pub fn pick_ground(&self, rect: egui::Rect, pos: egui::Pos2) -> Option<glam::Vec3> {
        self.camera.pick_ground(Self::ndc(rect, pos), self.aspect(), self.ground_y)
    }

    pub fn ray(&self, rect: egui::Rect, pos: egui::Pos2) -> (glam::Vec3, glam::Vec3) {
        self.camera.ray(Self::ndc(rect, pos), self.aspect())
    }

    pub fn set_top_down(&mut self, on: bool) {
        if on {
            self.camera.pitch = std::f32::consts::FRAC_PI_2;
            self.camera.yaw = 0.0;
            self.camera.ortho = true;
        } else {
            self.camera.pitch = 0.95;
            self.camera.ortho = false;
        }
    }

    pub fn is_top_down(&self) -> bool {
        self.camera.ortho
    }

    /// Shows the view, filling the available space. `scene` carries a
    /// generation number that changes whenever the scene is rebuilt.
    pub fn show(&mut self, ui: &mut egui::Ui, gfx: &Gfx, scene: Option<(&GpuScene, u64)>) -> ViewportResponse {
        let size = ui.available_size().max(egui::vec2(16.0, 16.0));
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let w = (rect.width() * ppp).round().clamp(1.0, 8192.0) as u32;
        let h = (rect.height() * ppp).round().clamp(1.0, 8192.0) as u32;

        if self.target.as_ref().map(|t| (t.width, t.height)) != Some((w, h)) {
            let target = RenderTarget::new(&gfx.rs.device, &gfx.renderer, w, h);
            let mut egui_renderer = gfx.rs.renderer.write();
            match self.texture {
                Some(id) => egui_renderer.update_egui_texture_from_wgpu_texture(
                    &gfx.rs.device,
                    &target.color_view,
                    wgpu::FilterMode::Linear,
                    id,
                ),
                None => {
                    self.texture = Some(egui_renderer.register_native_texture(
                        &gfx.rs.device,
                        &target.color_view,
                        wgpu::FilterMode::Linear,
                    ));
                }
            }
            self.target = Some(target);
            self.rendered = None;
        }

        self.handle_input(ui, &response, rect);

        if let (Some(target), Some((gpu_scene, generation))) = (&self.target, scene) {
            let state = (self.camera, self.options, generation, self.time);
            if self.rendered != Some(state) {
                let cam = self.camera.camera(target.aspect());
                gfx.renderer.render(&gfx.rs.device, &gfx.rs.queue, gpu_scene, target, &cam, &self.options);
                self.rendered = Some(state);
            }
            if let Some(id) = self.texture {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().image(id, rect, uv, egui::Color32::WHITE);
            }
        } else {
            ui.painter().rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        }
        ViewportResponse { response, rect }
    }

    fn handle_input(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect) {
        use egui::PointerButton::{Middle, Primary, Secondary};
        let delta = response.drag_delta();

        // Drag with the left or middle button: slide the map under the cursor.
        if (response.dragged_by(Primary) || response.dragged_by(Middle)) && delta != egui::Vec2::ZERO {
            if let Some(pos) = response.interact_pointer_pos() {
                let before = self.pick_ground(rect, pos - delta);
                let after = self.pick_ground(rect, pos);
                if let (Some(a), Some(b)) = (before, after) {
                    let shift = a - b;
                    if shift.length() < self.camera.distance * 4.0 {
                        self.camera.target += glam::Vec3::new(shift.x, 0.0, shift.z);
                    }
                }
            }
        }

        // Drag with the right button: orbit.
        if response.dragged_by(Secondary) && delta != egui::Vec2::ZERO {
            if self.camera.ortho {
                self.camera.ortho = false;
            }
            self.camera.yaw += delta.x * 0.006;
            self.camera.pitch = (self.camera.pitch + delta.y * 0.006).clamp(0.12, std::f32::consts::FRAC_PI_2);
        }

        // Wheel: zoom towards the cursor.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta().y);
            let pinch = ui.input(|i| i.zoom_delta());
            if scroll != 0.0 || pinch != 1.0 {
                let pos = response.hover_pos().unwrap_or(rect.center());
                let before = self.pick_ground(rect, pos);
                let factor = (-scroll * 0.0025).exp() / pinch;
                self.camera.distance = (self.camera.distance * factor).clamp(8.0, 200_000.0);
                if let (Some(a), Some(b)) = (before, self.pick_ground(rect, pos)) {
                    let shift = a - b;
                    if shift.length() < self.camera.distance * 4.0 {
                        self.camera.target += glam::Vec3::new(shift.x, 0.0, shift.z);
                    }
                }
            }
        }
    }
}
