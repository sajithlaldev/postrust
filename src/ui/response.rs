use crate::app::{RespTab, Tab};
use egui::{Color32, RichText, Ui};

fn status_color(code: u16) -> Color32 {
    match code {
        200..=299 => Color32::from_rgb(60, 170, 90),
        300..=399 => Color32::from_rgb(70, 140, 220),
        400..=499 => Color32::from_rgb(220, 140, 40),
        _ => Color32::from_rgb(210, 70, 70),
    }
}

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
    ui.add_space(4.0);
    if tab.pending.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Sending request…");
        });
        return false;
    }
    let Some(outcome) = &tab.response else {
        ui.weak("Hit Send to get a response.");
        return false;
    };
    let resp = match outcome {
        Err(e) => {
            ui.colored_label(Color32::from_rgb(210, 70, 70), RichText::new(e).strong());
            return false;
        }
        Ok(r) => r,
    };
    let d = &resp.data;

    ui.horizontal(|ui| {
        ui.colored_label(
            status_color(d.status),
            RichText::new(format!("{} {}", d.status, d.reason)).strong(),
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
                // The viewer text is truncated for big bodies; copy the whole downloaded text instead.
                ui.ctx().copy_text(if resp.display_cut {
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
    });
    ui.separator();

    match tab.resp_tab {
        RespTab::Body => {
            if d.truncated || resp.display_cut {
                ui.colored_label(
                    Color32::from_rgb(220, 140, 40),
                    "Body is too large to show in full. Use “Save body” for the downloaded part.",
                );
            }
            if d.body.is_empty() {
                ui.weak("Empty body.");
            } else if !resp.is_text {
                ui.label(format!(
                    "Binary body ({}). Use “Save body” to write it to a file.",
                    human_size(d.size)
                ));
            } else {
                egui::ScrollArea::both()
                    .id_salt("resp_body")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut text: &str = &resp.display;
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(8),
                        );
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
