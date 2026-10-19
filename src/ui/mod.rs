mod json;
mod palette;
mod request;
pub use request::url_field_id;
mod response;
mod sidebar;
mod theme;
mod widgets;
mod windows;

use crate::app::App;
use crate::commands::Command;
use crate::model::KeyValue;
use egui::{Key, Modifiers, Ui};

/// Editable key/value rows with an enable checkbox and a delete button.
/// The trailing "new row" only joins the model once the user types in it, so merely
/// viewing a table never modifies (or dirties) a request. `salt` must be unique per table
/// (sibling tables in one `Ui` would otherwise share the half-typed row).
pub fn kv_table(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, rows: &mut Vec<KeyValue>) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (i, row) in rows.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= kv_row(ui, row);
            if ui.small_button("×").on_hover_text("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    let blank_id = ui.scope_id().with(("kv_blank", salt));
    let mut blank: KeyValue = ui.data_mut(|d| d.get_temp(blank_id)).unwrap_or_default();
    ui.horizontal(|ui| {
        kv_row(ui, &mut blank);
    });
    if !blank.key.is_empty() || !blank.value.is_empty() {
        rows.push(std::mem::take(&mut blank));
        changed = true;
    }
    ui.data_mut(|d| d.insert_temp(blank_id, blank));
    if let Some(i) = remove {
        rows.remove(i);
        changed = true;
    }
    changed
}

fn kv_row(ui: &mut Ui, row: &mut KeyValue) -> bool {
    let mut changed = ui.checkbox(&mut row.enabled, "").changed();
    let w = ((ui.available_width() - 48.0) / 2.0).max(60.0);
    changed |= ui
        .add(
            egui::TextEdit::singleline(&mut row.key)
                .hint_text("Key")
                .desired_width(w),
        )
        .changed();
    changed |= ui
        .add(
            egui::TextEdit::singleline(&mut row.value)
                .hint_text("Value")
                .desired_width(w),
        )
        .changed();
    changed
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();

    // Install the look once, then follow the user's System / Light / Dark choice.
    if app.applied_theme != Some(app.store.ui.theme) {
        if app.applied_theme.is_none() {
            theme::install(&ctx);
        }
        theme::apply_mode(&ctx, app.store.ui.theme);
        app.applied_theme = Some(app.store.ui.theme);
    }

    // Global shortcuts. While the palette is open it owns the keyboard.
    if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
        palette::toggle(app);
    }
    // Shortcuts pause while the palette or the unsaved-changes dialog has the user's attention.
    if app.palette.is_none() && app.confirm_close.is_none() {
        for (key, command) in [
            (Key::Enter, Command::Send),
            (Key::S, Command::Save),
            (Key::T, Command::NewTab),
            (Key::W, Command::CloseTab),
        ] {
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, key)) {
                app.run_command(command, &ctx);
            }
        }
    }

    egui::Panel::top("top").show(ui, |ui| windows::top_bar(app, ui));

    let sidebar = egui::Panel::left("sidebar")
        .default_size(app.store.ui.sidebar_width.unwrap_or(280.0))
        .min_size(200.0)
        .show(ui, |ui| sidebar::draw(app, ui));
    // Remember the width when the user lets go of the mouse (so merely viewing, or a window
    // resize, never rewrites the saved preference or dirties the store at startup).
    if ctx.input(|i| i.pointer.any_released()) {
        app.remember_sidebar_width(sidebar.response.rect.width());
    }

    egui::Panel::bottom("status").show(ui, |ui| {
        let m = theme::modifier(&ctx);
        let hints = if app.web_mode {
            // The browser keeps T and W for itself (new tab, close tab); use the buttons.
            format!("{m}K commands  |  {m}Enter send  |  {m}S save")
        } else {
            format!("{m}K commands  |  {m}Enter send  |  {m}S save  |  {m}T new tab  |  {m}W close tab")
        };
        let hints_width = ui.fonts_mut(|f| {
            f.layout_no_wrap(
                hints.clone(),
                egui::TextStyle::Body.resolve(ui.style()),
                egui::Color32::WHITE,
            )
            .size()
            .x
        });
        let status = app.status.clone().unwrap_or_else(|| "Ready".into());
        ui.horizontal(|ui| {
            // The message gets whatever room the hints leave and is cut with "..." instead of
            // running into them (the full text is in the tooltip).
            let room = (ui.available_width() - hints_width - ui.spacing().item_spacing.x * 3.0).max(40.0);
            let height = ui.spacing().interact_size.y;
            ui.allocate_ui_with_layout(
                egui::vec2(room, height),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add(egui::Label::new(status.as_str()).truncate())
                        .on_hover_text(status.as_str());
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(hints);
            });
        });
    });

    egui::CentralPanel::default()
        .frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::symmetric(14, 10)))
        .show(ui, |ui| {
            request::tab_bar(app, ui);
            ui.add_space(6.0);
            request::draw(app, ui, &ctx);
        });

    windows::env_window(app, &ctx);
    windows::io_window(app, &ctx);
    windows::rename_window(app, &ctx);
    windows::close_confirm_window(app, &ctx);
    palette::show(app, &ctx);
}
