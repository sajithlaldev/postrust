use super::{kv_table, response};
use crate::app::{App, ReqTab};
use crate::model::{Auth, BodyKind, Method};
use egui::{Color32, Context, RichText, Ui};

#[derive(PartialEq)]
enum Action {
    Send,
    Cancel,
    Save,
}

pub fn tab_bar(app: &mut App, ui: &mut Ui) {
    let mut select = None;
    let mut close = None;
    let mut add = false;
    egui::ScrollArea::horizontal().id_salt("tabs").show(ui, |ui| {
        ui.horizontal(|ui| {
            for (i, tab) in app.tabs.iter().enumerate() {
                let label = format!("{}  {}", tab.request.method.as_str(), tab.request.title());
                let label: String = if label.chars().count() > 28 {
                    label.chars().take(27).collect::<String>() + "…"
                } else {
                    label
                };
                if ui.selectable_label(i == app.active, label).clicked() {
                    select = Some(i);
                }
                if ui.small_button("✕").on_hover_text("Close tab").clicked() {
                    close = Some(i);
                }
                ui.add_space(6.0);
            }
            if ui.button("＋").on_hover_text("New tab").clicked() {
                add = true;
            }
        });
    });
    if let Some(i) = select {
        app.active = i;
    }
    if add {
        app.new_tab(Default::default(), None);
    } else if let Some(i) = close {
        app.close_tab(i);
    }
}

pub fn draw(app: &mut App, ui: &mut Ui, ctx: &Context) {
    let idx = app.active;
    let missing = app.missing_vars(&app.tabs[idx].request);

    let mut save_body = false;
    egui::Panel::bottom("response")
        .resizable(true)
        .default_size(320.0)
        .min_size(120.0)
        .show(ui, |ui| {
            let tab = &mut app.tabs[idx];
            ui.push_id(tab.id, |ui| {
                save_body = response::draw(ui, tab);
            });
        });

    let mut action = None;
    let mut format_failed = false;
    let tab = &mut app.tabs[idx];
    egui::ScrollArea::vertical().id_salt("request_scroll").show(ui, |ui| {
        ui.push_id(tab.id, |ui| {
            let r = &mut tab.request;
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(&mut r.name)
                        .hint_text("Untitled request")
                        .desired_width(240.0),
                );
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("method")
                    .selected_text(r.method.as_str())
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for m in Method::ALL {
                            ui.selectable_value(&mut r.method, m, m.as_str());
                        }
                    });
                let w = (ui.available_width() - 160.0).max(120.0);
                ui.add(
                    egui::TextEdit::singleline(&mut r.url)
                        .hint_text("https://api.example.com/resource  (supports {{variables}})")
                        .desired_width(w),
                );
                if tab.pending.is_some() {
                    if ui.button("Cancel").clicked() {
                        action = Some(Action::Cancel);
                    }
                    ui.spinner();
                } else if ui.button(RichText::new("Send").strong()).clicked() {
                    action = Some(Action::Send);
                }
                if ui.button("Save").clicked() {
                    action = Some(Action::Save);
                }
            });
            if !missing.is_empty() {
                ui.colored_label(
                    Color32::from_rgb(220, 140, 40),
                    format!("Undefined variables: {}", missing.join(", ")),
                );
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let count = |n: usize| if n > 0 { format!(" ({n})") } else { String::new() };
                let np = r.params.iter().filter(|p| p.enabled && !p.key.is_empty()).count();
                let nh = r.headers.iter().filter(|p| p.enabled && !p.key.is_empty()).count();
                ui.selectable_value(&mut tab.req_tab, ReqTab::Params, format!("Params{}", count(np)));
                ui.selectable_value(&mut tab.req_tab, ReqTab::Headers, format!("Headers{}", count(nh)));
                ui.selectable_value(&mut tab.req_tab, ReqTab::Body, "Body");
                ui.selectable_value(&mut tab.req_tab, ReqTab::Auth, "Auth");
            });
            ui.separator();

            match tab.req_tab {
                ReqTab::Params => drop(kv_table(ui, "params", &mut r.params)),
                ReqTab::Headers => drop(kv_table(ui, "headers", &mut r.headers)),
                ReqTab::Body => {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut r.body_kind, BodyKind::None, "None");
                        ui.radio_value(&mut r.body_kind, BodyKind::Raw, "Raw");
                        ui.radio_value(&mut r.body_kind, BodyKind::Form, "Form (urlencoded)");
                    });
                    match r.body_kind {
                        BodyKind::None => {
                            ui.weak("This request has no body.");
                        }
                        BodyKind::Raw => {
                            if ui.small_button("Format JSON").clicked() {
                                match serde_json::from_str::<serde_json::Value>(&r.body) {
                                    Ok(v) => {
                                        r.body = serde_json::to_string_pretty(&v).unwrap_or_else(|_| r.body.clone())
                                    }
                                    Err(_) => format_failed = true,
                                }
                            }
                            ui.add(
                                egui::TextEdit::multiline(&mut r.body)
                                    .code_editor()
                                    .desired_rows(10)
                                    .desired_width(f32::INFINITY),
                            );
                        }
                        BodyKind::Form => drop(kv_table(ui, "form", &mut r.form)),
                    }
                }
                ReqTab::Auth => auth_editor(ui, &mut r.auth),
            }
        });
    });

    if save_body {
        app.save_body(idx);
    }
    if format_failed {
        app.status = Some("Body is not valid JSON (put {{variables}} inside quotes to format it)".into());
    }
    match action {
        Some(Action::Send) => app.send(idx, ctx),
        Some(Action::Cancel) => app.cancel(idx),
        Some(Action::Save) => app.save_tab(idx),
        None => {}
    }
}

fn auth_editor(ui: &mut Ui, auth: &mut Auth) {
    let kind = match auth {
        Auth::None => "No auth",
        Auth::Bearer { .. } => "Bearer token",
        Auth::Basic { .. } => "Basic auth",
        Auth::ApiKey { .. } => "API key (header)",
    };
    egui::ComboBox::from_id_salt("auth_kind")
        .selected_text(kind)
        .show_ui(ui, |ui| {
            if ui.selectable_label(matches!(auth, Auth::None), "No auth").clicked() {
                *auth = Auth::None;
            }
            if ui
                .selectable_label(matches!(auth, Auth::Bearer { .. }), "Bearer token")
                .clicked()
                && !matches!(auth, Auth::Bearer { .. })
            {
                *auth = Auth::Bearer { token: String::new() };
            }
            if ui
                .selectable_label(matches!(auth, Auth::Basic { .. }), "Basic auth")
                .clicked()
                && !matches!(auth, Auth::Basic { .. })
            {
                *auth = Auth::Basic {
                    username: String::new(),
                    password: String::new(),
                };
            }
            if ui
                .selectable_label(matches!(auth, Auth::ApiKey { .. }), "API key (header)")
                .clicked()
                && !matches!(auth, Auth::ApiKey { .. })
            {
                *auth = Auth::ApiKey {
                    name: "X-API-Key".into(),
                    value: String::new(),
                };
            }
        });
    ui.add_space(4.0);
    match auth {
        Auth::None => {
            ui.weak("This request does not use any authorization.");
        }
        Auth::Bearer { token } => {
            ui.label("Token");
            ui.add(
                egui::TextEdit::singleline(token)
                    .password(false)
                    .desired_width(f32::INFINITY),
            );
        }
        Auth::Basic { username, password } => {
            ui.label("Username");
            ui.add(egui::TextEdit::singleline(username).desired_width(f32::INFINITY));
            ui.label("Password");
            ui.add(
                egui::TextEdit::singleline(password)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
        }
        Auth::ApiKey { name, value } => {
            ui.label("Header name");
            ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
            ui.label("Value");
            ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
        }
    }
}
