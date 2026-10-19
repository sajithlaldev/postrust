use super::{kv_table, response, theme, widgets};
use crate::app::{App, ReqTab};
use crate::model::{Auth, BodyKind, Method, SplitLayout};
use egui::{Color32, Context, CornerRadius, Frame, Margin, RichText, Sense, Stroke, Ui, vec2};

#[derive(PartialEq)]
enum Action {
    /// A `curl ...` command was pasted into the URL field.
    ImportCurl(Box<crate::curl::Imported>),
    UndoImport,
    /// Show a message in the status bar.
    Status(String),
    UndoBeautify,
    Send,
    Cancel,
    Save,
    Curl,
    Beautify,
}

/// Stable widget id of a tab's URL field (tests and the paste handler both need it).
pub fn url_field_id(tab_id: u64) -> egui::Id {
    egui::Id::unique(("url_edit", tab_id))
}

pub fn tab_bar(app: &mut App, ui: &mut Ui) {
    let dark = ui.visuals().dark_mode;
    let accent = theme::accent(dark);
    let mut select = None;
    let mut close = None;
    let mut add = false;
    egui::ScrollArea::horizontal().id_salt("tabs").show(ui, |ui| {
        ui.horizontal(|ui| {
            for i in 0..app.tabs.len() {
                let active = i == app.active;
                let dirty = app.is_tab_dirty(i);
                let tab = &app.tabs[i];
                let title: String = {
                    let t = tab.request.title();
                    if t.chars().count() > 24 {
                        t.chars().take(23).collect::<String>() + "..."
                    } else {
                        t
                    }
                };
                let fill = if active {
                    ui.visuals().widgets.inactive.weak_bg_fill
                } else {
                    Color32::TRANSPARENT
                };
                let out = Frame::new()
                    .fill(fill)
                    .corner_radius(CornerRadius {
                        nw: 6,
                        ne: 6,
                        sw: 0,
                        se: 0,
                    })
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // The badge selects the tab too, like the title next to it.
                            if widgets::method_badge(ui, tab.request.method)
                                .interact(Sense::click())
                                .clicked()
                            {
                                select = Some(i);
                            }
                            let label = egui::Label::new(RichText::new(title).strong().color(if active {
                                ui.visuals().text_color()
                            } else {
                                ui.visuals().weak_text_color()
                            }))
                            .sense(Sense::click());
                            if ui.add(label).clicked() {
                                select = Some(i);
                            }
                            if dirty {
                                let (rect, resp) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                ui.painter().circle_filled(rect.center(), 3.5, accent);
                                resp.on_hover_text("Unsaved changes");
                            }
                            if ui.small_button("\u{d7}").on_hover_text("Close tab").clicked() {
                                close = Some(i);
                            }
                        });
                    });
                if active {
                    let r = out.response.rect;
                    ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(2.0, accent));
                }
            }
            if ui.button("+").on_hover_text("New tab").clicked() {
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
        app.request_close_tab(i);
    }
}

pub fn draw(app: &mut App, ui: &mut Ui, ctx: &Context) {
    let idx = app.active;
    let missing = app.missing_vars(&app.tabs[idx].request);
    let split = app.store.ui.split;

    let mut save_body = false;
    // Cap the response panel so the request bar (method, URL and its buttons) always keeps room,
    // whatever size was saved earlier or however small the window is.
    let (panel, cap) = match split {
        SplitLayout::Below => {
            let max = (ui.available_height() - 220.0).max(160.0);
            let panel = egui::Panel::bottom("response_below")
                .resizable(true)
                .default_size(app.store.ui.response_height.unwrap_or(320.0).min(max))
                .min_size(140.0)
                .max_size(max);
            (panel, max)
        }
        SplitLayout::Side => {
            let max = (ui.available_width() - 480.0).max(260.0);
            let panel = egui::Panel::right("response_side")
                .resizable(true)
                .default_size(app.store.ui.response_width.unwrap_or(460.0).min(max))
                .min_size(260.0)
                .max_size(max);
            (panel, max)
        }
    };
    // The panel's full size (margins included) is the space it takes from the area around it.
    // `InnerResponse::response.rect` is only the content rectangle, so it can't be used.
    let space = |ui: &Ui| match split {
        SplitLayout::Below => ui.available_height(),
        SplitLayout::Side => ui.available_width(),
    };
    let before = space(ui);
    panel.show(ui, |ui| {
        // egui shrinks a resizable panel to its content after the first frame, so a short
        // "No response yet" message would collapse it. Claim the whole panel up front. Unlike
        // allocating space afterwards, this adds no item spacing, so a full panel can't creep
        // larger each frame.
        ui.set_min_size(ui.available_size());
        let tab = &mut app.tabs[idx];
        ui.push_id(tab.id, |ui| {
            save_body = response::draw(ui, tab);
        });
    });
    if ctx.input(|i| i.pointer.any_released()) {
        let size = before - space(ui);
        // A panel squeezed by a small window sits at its cap, below the size the user saved;
        // that temporary value must not replace the preference. A panel the user dragged up to
        // the cap is larger than what was saved, so it is kept.
        let saved = match split {
            SplitLayout::Below => app.store.ui.response_height,
            SplitLayout::Side => app.store.ui.response_width,
        };
        if size < cap - 1.0 || saved.is_none_or(|s| size > s) {
            app.remember_response_size(split, size);
        }
    }

    let mut action = None;
    let dark = ui.visuals().dark_mode;
    let tab = &mut app.tabs[idx];
    egui::ScrollArea::vertical().id_salt("request_scroll").show(ui, |ui| {
        ui.push_id(tab.id, |ui| {
            let r = &mut tab.request;
            ui.add(
                egui::TextEdit::singleline(&mut r.name)
                    .hint_text("Untitled request")
                    .font(egui::TextStyle::Heading)
                    .frame(Frame::NONE)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(2.0);

            // Request bar: method, URL and the main actions in one rounded strip.
            Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("method")
                            .selected_text(
                                RichText::new(r.method.as_str())
                                    .strong()
                                    .color(theme::method_color(r.method, dark)),
                            )
                            .width(86.0)
                            .show_ui(ui, |ui| {
                                for m in Method::ALL {
                                    ui.selectable_value(
                                        &mut r.method,
                                        m,
                                        RichText::new(m.as_str()).strong().color(theme::method_color(m, dark)),
                                    );
                                }
                            });
                        let w = (ui.available_width() - 250.0).max(120.0);
                        let url_id = url_field_id(tab.id);
                        // Pasting a `curl ...` command into the URL field fills in the whole
                        // request. The paste event is removed so the raw text isn't also typed in.
                        if ui.memory(|m| m.has_focus(url_id)) {
                            let pasted = ui.input(|i| {
                                i.events.iter().find_map(|e| match e {
                                    egui::Event::Paste(t) if crate::curl::looks_like_curl(t) => Some(t.clone()),
                                    _ => None,
                                })
                            });
                            if let Some(text) = pasted {
                                match crate::curl::parse(&text) {
                                    Ok(imported) => {
                                        ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Paste(_))));
                                        action = Some(Action::ImportCurl(Box::new(imported)));
                                    }
                                    // Unparseable: don't dump a long multi-line command into the one-line
                                    // field; keep it as it was and say why. The text is still on the
                                    // clipboard.
                                    Err(e) => {
                                        ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Paste(_))));
                                        action = Some(Action::Status(format!(
                                            "Could not import cURL: {e} (the URL field was left unchanged)"
                                        )));
                                    }
                                }
                            }
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut r.url)
                                .id(url_id)
                                .hint_text("https://api.example.com/resource  ({{variables}} work here)")
                                .font(egui::TextStyle::Monospace)
                                .frame(Frame::NONE)
                                // Same height as the buttons beside it, with the text centred in
                                // that height (it was drawn at the top of the taller box).
                                .min_size(vec2(0.0, 28.0))
                                .align(egui::Align2::LEFT_CENTER)
                                .desired_width(w),
                        );
                        if tab.pending.is_some() {
                            ui.spinner();
                            if ui.button("Cancel").clicked() {
                                action = Some(Action::Cancel);
                            }
                        } else {
                            let send = egui::Button::new(RichText::new("Send").strong().color(theme::on_accent(dark)))
                                .fill(theme::accent(dark))
                                .min_size(vec2(72.0, 28.0));
                            if ui.add(send).clicked() {
                                action = Some(Action::Send);
                            }
                        }
                        if ui.button("Save").clicked() {
                            action = Some(Action::Save);
                        }
                        if ui
                            .button("cURL")
                            .on_hover_text("Copy this request as a curl command (for sh, bash and zsh)")
                            .clicked()
                        {
                            action = Some(Action::Curl);
                        }
                    });
                });
            // Undo only while the request is still exactly what the import produced; once it has
            // been edited the saved copies are dropped. (While untouched this is one equality
            // check per frame, which stops at the first difference or is a fast memcmp.)
            if tab.import_undo.as_ref().is_some_and(|(_, after)| after != &*r) {
                tab.import_undo = None;
            }
            if tab.import_undo.is_some() {
                ui.horizontal(|ui| {
                    ui.weak("Filled in from a cURL command.");
                    if ui
                        .button("Undo import")
                        .on_hover_text("Restore the request from before the paste")
                        .clicked()
                    {
                        action = Some(Action::UndoImport);
                    }
                });
            }
            if !missing.is_empty() {
                ui.colored_label(
                    theme::warning(dark),
                    format!("Undefined variables: {}", missing.join(", ")),
                );
            }

            ui.add_space(6.0);
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
                            if ui
                                .button("Beautify JSON")
                                .on_hover_text("Pretty-print the body. {{variables}} are kept as written.")
                                .clicked()
                            {
                                action = Some(Action::Beautify);
                            }
                            // Undo only while the body is still exactly what Beautify produced.
                            if tab.beautify_undo.as_ref().is_some_and(|(_, after)| *after == r.body)
                                && ui
                                    .button("Undo")
                                    .on_hover_text("Restore the body from before Beautify")
                                    .clicked()
                            {
                                action = Some(Action::UndoBeautify);
                            }
                            let mut layouter = super::json::layouter();
                            ui.add(
                                egui::TextEdit::multiline(&mut r.body)
                                    .code_editor()
                                    .layouter(&mut layouter)
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
    match action {
        Some(Action::Send) => app.send(idx, ctx),
        Some(Action::Cancel) => app.cancel(idx),
        Some(Action::Save) => app.save_tab(idx),
        Some(Action::Curl) => app.copy_curl(idx, ctx),
        Some(Action::Beautify) => app.beautify_body(idx),
        Some(Action::UndoBeautify) => app.undo_beautify(idx),
        Some(Action::ImportCurl(imported)) => {
            app.apply_import(idx, *imported);
        }
        Some(Action::UndoImport) => app.undo_import(idx),
        Some(Action::Status(msg)) => app.status = Some(msg),
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
