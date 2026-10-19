mod request;
mod response;
mod sidebar;
mod windows;

use crate::app::App;
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
            if ui.small_button("✕").on_hover_text("Remove").clicked() {
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

    // Global shortcuts.
    let send = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
    let save = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S));
    let new_tab = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::T));
    let close = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::W));
    if send {
        app.send(app.active, &ctx);
    }
    if save {
        app.save_tab(app.active);
    }
    if new_tab {
        app.new_tab(Default::default(), None);
    }
    if close {
        app.close_tab(app.active);
    }

    egui::Panel::top("top").show(ui, |ui| windows::top_bar(app, ui));
    egui::Panel::left("sidebar")
        .default_size(260.0)
        .min_size(180.0)
        .show(ui, |ui| sidebar::draw(app, ui));
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(app.status.clone().unwrap_or_else(|| "Ready".into()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let m = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
                ui.weak(format!("{m}Enter send · {m}S save · {m}T new tab · {m}W close tab"));
            });
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        request::tab_bar(app, ui);
        ui.separator();
        request::draw(app, ui, &ctx);
    });

    windows::env_window(app, &ctx);
    windows::io_window(app, &ctx);
    windows::rename_window(app, &ctx);
}
