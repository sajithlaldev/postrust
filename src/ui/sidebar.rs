use crate::app::{App, RenameTarget};
use crate::model::Request;
use egui::{RichText, Ui};

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

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 70.0).max(60.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut app.new_collection)
                .hint_text("New collection")
                .desired_width(w),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("Add").clicked() || enter) && !app.new_collection.trim().is_empty() {
            action = Some(Action::NewCollection);
        }
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("sidebar_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(RichText::new("Collections").strong());
            if app.store.collections.is_empty() {
                ui.weak("No collections yet. Save a request to create one.");
            }
            for (ci, coll) in app.store.collections.iter().enumerate() {
                let header = egui::CollapsingHeader::new(format!("{} ({})", coll.name, coll.requests.len()))
                    .id_salt(("coll", ci, &coll.name))
                    .default_open(true)
                    .show(ui, |ui| {
                        for (ri, req) in coll.requests.iter().enumerate() {
                            let open = app.tabs.get(app.active).is_some_and(|t| t.source == Some((ci, ri)));
                            let label = format!("{:<7}{}", req.method.as_str(), req.title());
                            let resp = ui.selectable_label(open, RichText::new(label).monospace());
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

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("History").strong());
                if !app.store.history.is_empty() && ui.small_button("Clear").clicked() {
                    action = Some(Action::ClearHistory);
                }
            });
            if app.store.history.is_empty() {
                ui.weak("Sent requests show up here.");
            }
            for req in &app.store.history {
                let label = format!("{:<7}{}", req.method.as_str(), req.url);
                if ui.selectable_label(false, RichText::new(label).monospace()).clicked() {
                    action = Some(Action::OpenHistory(req.clone()));
                }
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
        }
        None => {}
    }
}
