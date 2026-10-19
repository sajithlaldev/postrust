use super::kv_table;
use crate::app::App;
use egui::{Context, Ui};

pub fn top_bar(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if app.logo.is_none()
            && let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon-64.png"))
        {
            let image =
                egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
            app.logo = Some(ui.ctx().load_texture("logo", image, egui::TextureOptions::LINEAR));
        }
        if let Some(logo) = &app.logo {
            ui.add(
                egui::Image::from_texture(egui::load::SizedTexture::from_handle(logo))
                    .fit_to_exact_size(egui::vec2(26.0, 26.0)),
            );
        }
        ui.heading("postrust");
        ui.separator();
        ui.label("Environment");
        let current = app
            .store
            .active_env
            .and_then(|i| app.store.environments.get(i))
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "No environment".into());
        let mut chosen = app.store.active_env;
        egui::ComboBox::from_id_salt("env_select")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut chosen, None, "No environment");
                for (i, e) in app.store.environments.iter().enumerate() {
                    ui.selectable_value(&mut chosen, Some(i), &e.name);
                }
            });
        if chosen != app.store.active_env {
            app.set_active_env(chosen);
        }
        if ui.button("Manage").clicked() {
            app.show_env = !app.show_env;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Import / Export").clicked() {
                app.show_io = !app.show_io;
            }
        });
    });
}

pub fn env_window(app: &mut App, ctx: &Context) {
    let mut open = app.show_env;
    egui::Window::new("Environments")
        .open(&mut open)
        .default_width(480.0)
        .show(ctx, |ui| {
            let mut changed = false;
            let mut delete = None;
            ui.horizontal(|ui| {
                ui.label("Request timeout (s)");
                let mut t = app.store.timeout_secs.unwrap_or(crate::http::DEFAULT_TIMEOUT_SECS);
                if ui.add(egui::DragValue::new(&mut t).range(1..=600)).changed() {
                    app.set_timeout(t);
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                for (i, e) in app.store.environments.iter().enumerate() {
                    if ui.selectable_label(app.env_sel == i, &e.name).clicked() {
                        app.env_sel = i;
                    }
                }
                if ui.button("＋ New").clicked() {
                    app.add_environment();
                }
            });
            ui.separator();
            let sel = app.env_sel;
            match app.store.environments.get_mut(sel) {
                None => {
                    ui.weak("Create an environment to define {{variables}}.");
                }
                Some(env) => {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        changed |= ui.text_edit_singleline(&mut env.name).changed();
                        if ui.button("Delete").clicked() {
                            delete = Some(sel);
                        }
                    });
                    changed |= kv_table(ui, ("env", sel), &mut env.vars);
                }
            }
            if let Some(i) = delete {
                app.delete_environment(i);
                changed = true;
            }
            if changed {
                app.mark_dirty();
            }
        });
    app.show_env = open;
}

pub fn io_window(app: &mut App, ctx: &Context) {
    let mut open = app.show_io;
    egui::Window::new("Import / Export collections")
        .open(&mut open)
        .default_width(480.0)
        .show(ctx, |ui| {
            ui.label("File path");
            ui.add(egui::TextEdit::singleline(&mut app.io_path).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                if ui.button("Export all collections").clicked() {
                    app.export_collections();
                }
                if ui.button("Import (append)").clicked() {
                    app.import_collections();
                }
            });
            ui.weak("Collections are plain JSON. Secrets (tokens, passwords) are stored unencrypted.");
        });
    app.show_io = open;
}

pub fn rename_window(app: &mut App, ctx: &Context) {
    let Some((target, mut name)) = app.rename.take() else {
        return;
    };
    let mut keep = true;
    let mut apply = false;
    egui::Window::new("Rename")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            let r = ui.text_edit_singleline(&mut name);
            r.request_focus();
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() || enter {
                    apply = true;
                    keep = false;
                }
                if ui.button("Cancel").clicked() {
                    keep = false;
                }
            });
        });
    if apply {
        app.rename(&target, &name);
    }
    if keep {
        app.rename = Some((target, name));
    }
}
