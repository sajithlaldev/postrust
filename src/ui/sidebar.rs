use super::widgets;
use crate::app::{App, RenameTarget};
use crate::model::Request;
use egui::{Key, RichText, Ui};

enum Action {
    Open(usize, usize),
    OpenHistory(Request),
    DeleteRequest(usize, usize),
    DeleteCollection(usize),
    Rename(RenameTarget, String),
    NewCollection,
    ClearHistory,
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    let mut action = None;
    let query = app.search.trim().to_lowercase();
    let filtering = !query.is_empty();

    ui.add_space(6.0);
    ui.add(
        egui::TextEdit::singleline(&mut app.search)
            .hint_text("Search requests")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(6.0);

    egui::ScrollArea::vertical()
        .id_salt("sidebar_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::section_title(ui, "Collections");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("+").on_hover_text("New collection").clicked() {
                        app.adding_collection = !app.adding_collection;
                        app.focus_new_collection = app.adding_collection;
                    }
                });
            });
            if app.adding_collection {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut app.new_collection)
                        .hint_text("Collection name, then Enter")
                        .desired_width(f32::INFINITY),
                );
                // Focus once, so clicking elsewhere isn't fought over every frame.
                if std::mem::take(&mut app.focus_new_collection) {
                    r.request_focus();
                }
                // egui's text box takes Enter/Escape itself and drops focus, so look for "lost focus
                // together with the key". Never while the palette owns the keyboard.
                if r.lost_focus() && app.palette.is_none() {
                    let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                    if enter && !app.new_collection.trim().is_empty() {
                        action = Some(Action::NewCollection);
                    } else if esc {
                        app.adding_collection = false;
                        app.new_collection.clear();
                    }
                }
            }

            let mut shown_any = false;
            for (ci, coll) in app.store.collections.iter().enumerate() {
                // A collection whose name matches shows all of its requests.
                let name_hit = filtering && coll.name.to_lowercase().contains(&query);
                let visible: Vec<usize> = (0..coll.requests.len())
                    .filter(|&ri| !filtering || name_hit || coll.requests[ri].matches(&query))
                    .collect();
                if filtering && visible.is_empty() {
                    continue;
                }
                shown_any = true;
                let header = egui::CollapsingHeader::new(
                    RichText::new(format!("{} ({})", coll.name, coll.requests.len())).strong(),
                )
                // Filtering forces headers open; use separate state so that never overwrites the user's
                // own expanded/collapsed choices.
                .id_salt(("coll", ci, &coll.name, filtering))
                .default_open(true)
                .open(filtering.then_some(true))
                .show(ui, |ui| {
                    for &ri in &visible {
                        let req = &coll.requests[ri];
                        let open = app.tabs.get(app.active).is_some_and(|t| t.source == Some((ci, ri)));
                        let resp = widgets::list_row(ui, open, req.method, &req.title());
                        if resp.clicked() {
                            action = Some(Action::Open(ci, ri));
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Rename").clicked() {
                                action = Some(Action::Rename(RenameTarget::Request(ci, ri), req.name.clone()));
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                action = Some(Action::DeleteRequest(ci, ri));
                                ui.close();
                            }
                        });
                    }
                });
                header.header_response.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        action = Some(Action::Rename(RenameTarget::Collection(ci), coll.name.clone()));
                        ui.close();
                    }
                    if ui.button("Delete collection").clicked() {
                        action = Some(Action::DeleteCollection(ci));
                        ui.close();
                    }
                });
            }
            if app.store.collections.is_empty() {
                widgets::empty_state(ui, "No collections yet", "Save a request, or press + to add one");
            } else if filtering && !shown_any {
                widgets::empty_state(ui, "No matching collections", "Try a different search");
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                widgets::section_title(ui, "History");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !app.store.history.is_empty() && ui.small_button("Clear").clicked() {
                        action = Some(Action::ClearHistory);
                    }
                });
            });
            let mut history_shown = false;
            for req in app.store.history.iter().filter(|r| !filtering || r.matches(&query)) {
                history_shown = true;
                let title = if req.url.is_empty() {
                    req.title()
                } else {
                    req.url.clone()
                };
                if widgets::list_row(ui, false, req.method, &title).clicked() {
                    action = Some(Action::OpenHistory(req.clone()));
                }
            }
            if !history_shown {
                let hint = if filtering {
                    "No matching history"
                } else {
                    "Sent requests show up here"
                };
                ui.weak(hint);
            }
        });

    match action {
        Some(Action::Open(c, r)) => app.open_saved(c, r),
        Some(Action::OpenHistory(req)) => app.new_tab(req, None),
        Some(Action::DeleteRequest(c, r)) => app.delete_request(c, r),
        Some(Action::DeleteCollection(c)) => app.delete_collection(c),
        Some(Action::Rename(t, name)) => app.rename = Some((t, name)),
        Some(Action::ClearHistory) => app.clear_history(),
        Some(Action::NewCollection) => {
            let name = std::mem::take(&mut app.new_collection);
            app.add_collection(&name);
            app.adding_collection = false;
        }
        None => {}
    }
}
