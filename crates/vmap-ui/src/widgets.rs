//! Small shared widgets.

use eframe::egui::{self, Align2, Color32, FontId};

/// A full-width, left-aligned, selectable list row with an optional second
/// line of detail and an optional short marker at the right edge. Text
/// that does not fit is clipped.
pub fn list_row(
    ui: &mut egui::Ui,
    selected: bool,
    title: &str,
    detail: Option<&str>,
    tag: Option<&str>,
    height: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let bg = if selected {
            visuals.selection.bg_fill
        } else if response.hovered() {
            visuals.widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        let title_color = if selected { visuals.selection.stroke.color } else { visuals.text_color() };
        let detail_color = if selected { visuals.selection.stroke.color.gamma_multiply(0.75) } else { visuals.weak_text_color() };
        ui.painter().rect_filled(rect, 4.0, bg);
        let mut inner = rect.shrink2(egui::vec2(8.0, 0.0));
        if let Some(tag) = tag {
            let color = if selected { title_color } else { visuals.warn_fg_color };
            let drawn =
                ui.painter().text(inner.right_center(), Align2::RIGHT_CENTER, tag, FontId::proportional(11.0), color);
            inner.set_right(drawn.left() - 6.0);
        }
        let painter = ui.painter().with_clip_rect(inner);
        match detail {
            Some(d) => {
                painter.text(
                    egui::pos2(inner.left(), rect.center().y - 1.0),
                    Align2::LEFT_BOTTOM,
                    title,
                    FontId::proportional(13.5),
                    title_color,
                );
                painter.text(
                    egui::pos2(inner.left(), rect.center().y + 1.0),
                    Align2::LEFT_TOP,
                    d,
                    FontId::proportional(11.0),
                    detail_color,
                );
            }
            None => {
                painter.text(inner.left_center(), Align2::LEFT_CENTER, title, FontId::proportional(13.0), title_color);
            }
        }
    }
    response
}

/// Row height for [`list_row`] with and without a detail line.
pub fn row_height(detail: bool) -> f32 {
    if detail { 38.0 } else { 22.0 }
}

/// A small eye that toggles whether something is shown. It is drawn struck
/// through while `visible` is false.
pub fn eye_button(ui: &mut egui::Ui, visible: bool, what: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(20.0, 16.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let color = if response.hovered() {
            visuals.strong_text_color()
        } else if visible {
            visuals.text_color()
        } else {
            visuals.weak_text_color()
        };
        let stroke = egui::Stroke::new(1.3, color);
        let c = rect.center();
        let (half_width, half_height) = (7.5, 4.2);
        // Each lid is an arc between the two corners of the eye.
        let lid = |side: f32| -> Vec<egui::Pos2> {
            (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    let x = -half_width + 2.0 * half_width * t;
                    let y = side * half_height * (std::f32::consts::PI * t).sin();
                    egui::pos2(c.x + x, c.y + y)
                })
                .collect()
        };
        let painter = ui.painter();
        painter.add(egui::Shape::line(lid(-1.0), stroke));
        painter.add(egui::Shape::line(lid(1.0), stroke));
        painter.circle_filled(c, 2.0, color);
        if !visible {
            let slash = [c + egui::vec2(-6.5, 6.0), c + egui::vec2(6.5, -6.0)];
            // A gap in the background colour first, so the slash reads clearly.
            painter.line_segment(slash, egui::Stroke::new(3.6, visuals.panel_fill));
            painter.line_segment(slash, egui::Stroke::new(1.5, color));
        }
    }
    response.on_hover_text(if visible { format!("Hide {what}") } else { format!("Show {what}") })
}
