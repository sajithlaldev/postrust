use super::{theme, widgets};
use crate::app::{RespTab, Tab};
use egui::{RichText, Ui};

fn human_size(n: usize) -> String {
    const KB: f64 = 1024.0;
    let n = n as f64;
    if n < KB {
        format!("{n} B")
    } else if n < KB * KB {
        format!("{:.1} KB", n / KB)
    } else {
        format!("{:.1} MB", n / KB / KB)
    }
}

/// Draws the response area. Returns true when the user asked to save the body.
pub fn draw(ui: &mut Ui, tab: &mut Tab) -> bool {
    let mut save = false;
    let dark = ui.visuals().dark_mode;
    ui.add_space(4.0);
    if tab.pending.is_some() {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.spinner();
            ui.label("Sending request...");
        });
        return false;
    }
    let Some(outcome) = &tab.response else {
        widgets::empty_state(
            ui,
            "No response yet",
            &format!("Press Send or {}Enter", theme::modifier()),
        );
        return false;
    };
    let resp = match outcome {
        Err(e) => {
            ui.add_space(8.0);
            widgets::pill(ui, "Request failed", theme::error(dark));
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(e).color(theme::error(dark))).wrap());
            return false;
        }
        Ok(r) => r,
    };
    let d = &resp.data;

    ui.horizontal(|ui| {
        widgets::pill(
            ui,
            format!("{} {}", d.status, d.reason).trim().to_string(),
            theme::status_color(d.status, dark),
        );
        ui.label(format!("{} ms", d.elapsed_ms));
        ui.label(human_size(d.size));
        if let Some(ct) = d.content_type() {
            ui.weak(ct);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Save body").clicked() {
                save = true;
            }
            if resp.is_text && ui.button("Copy").clicked() {
                // Copy what the user is looking at: the pretty text, or the body exactly as received
                // (Raw view). The viewer text is truncated for big bodies, so copy the whole
                // downloaded text instead whenever the view was cut or Raw is selected.
                let raw_view = !tab.pretty && !resp.raw.is_empty();
                ui.ctx().copy_text(if raw_view || resp.display_cut {
                    resp.data.text()
                } else {
                    resp.display.clone()
                });
            }
        });
    });
    ui.horizontal(|ui| {
        ui.selectable_value(&mut tab.resp_tab, RespTab::Body, "Body");
        ui.selectable_value(
            &mut tab.resp_tab,
            RespTab::Headers,
            format!("Headers ({})", d.headers.len()),
        );
        if !resp.raw.is_empty() && tab.resp_tab == RespTab::Body {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.selectable_value(&mut tab.pretty, false, "Raw");
                ui.selectable_value(&mut tab.pretty, true, "Pretty");
            });
        }
    });
    ui.separator();

    match tab.resp_tab {
        RespTab::Body => {
            if d.truncated || resp.display_cut {
                ui.colored_label(
                    theme::warning(dark),
                    "Body is too large to show in full. Use \"Save body\" for the downloaded part.",
                );
            }
            if d.body.is_empty() {
                ui.weak("Empty body.");
            } else if !resp.is_text {
                ui.label(format!(
                    "Binary body ({}). Use \"Save body\" to write it to a file.",
                    human_size(d.size)
                ));
            } else {
                egui::ScrollArea::both()
                    .id_salt("resp_body")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut text: &str = if tab.pretty || resp.raw.is_empty() {
                            &resp.display
                        } else {
                            &resp.raw
                        };
                        let mut layouter = super::json::layouter();
                        let mut edit = egui::TextEdit::multiline(&mut text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(8);
                        if resp.is_json {
                            edit = edit.layouter(&mut layouter);
                        }
                        ui.add(edit);
                    });
            }
        }
        RespTab::Headers => {
            egui::ScrollArea::vertical()
                .id_salt("resp_headers")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("headers_grid")
                        .striped(true)
                        .num_columns(2)
                        .show(ui, |ui| {
                            for (k, v) in &d.headers {
                                ui.label(RichText::new(k).strong());
                                ui.add(egui::Label::new(v).wrap());
                                ui.end_row();
                            }
                        });
                });
        }
    }
    save
}
