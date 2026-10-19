//! Small reusable pieces: method badges, status pills and the painted list rows.

use super::theme;
use crate::model::Method;
use egui::{Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Margin, Response, RichText, Sense, Ui, vec2};

/// A fixed-width coloured method label ("GET", "POST", ...).
pub fn method_badge(ui: &mut Ui, method: Method) -> Response {
    let dark = ui.visuals().dark_mode;
    let text = RichText::new(theme::method_short(method))
        .monospace()
        .size(11.0)
        .strong()
        .color(theme::method_color(method, dark));
    ui.add_sized(vec2(40.0, 18.0), egui::Label::new(text))
}

/// A rounded, tinted pill, e.g. for the response status.
pub fn pill(ui: &mut Ui, text: impl Into<String>, color: Color32) {
    Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text.into()).strong().color(color));
        });
}

/// Small upper-case section heading.
pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).small().strong().weak());
}

/// A full-width clickable row: method badge plus title, painted directly so it can be styled
/// as a Postman-like list item. Long titles are clipped at the row edge.
pub fn list_row(ui: &mut Ui, selected: bool, method: Method, title: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let dark = ui.visuals().dark_mode;
        let painter = ui.painter_at(rect);
        let bg = if selected {
            theme::accent(dark).gamma_multiply(0.22)
        } else if response.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect, CornerRadius::same(5), bg);
        painter.text(
            rect.left_center() + vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            theme::method_short(method),
            FontId::monospace(11.0),
            theme::method_color(method, dark),
        );
        painter.text(
            rect.left_center() + vec2(52.0, 0.0),
            Align2::LEFT_CENTER,
            title,
            FontId::proportional(14.0),
            ui.visuals().text_color(),
        );
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// An empty-state message, centred and muted.
pub fn empty_state(ui: &mut Ui, title: &str, hint: &str) {
    ui.add_space(14.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).strong().weak());
        ui.label(RichText::new(hint).small().weak());
    });
    ui.add_space(14.0);
}
