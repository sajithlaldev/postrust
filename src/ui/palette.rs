//! The Cmd/Ctrl+K command palette.

use super::theme;
use crate::app::{App, PaletteState};
use crate::commands;
use egui::{Align2, Context, CornerRadius, Key, Modifiers, RichText, Sense, vec2};

pub fn toggle(app: &mut App) {
    app.palette = if app.palette.is_some() {
        None
    } else {
        Some(PaletteState::default())
    };
}

pub fn show(app: &mut App, ctx: &Context) {
    let Some(mut state) = app.palette.take() else { return };

    // Escape and the arrows don't depend on this frame's typing, so take them first (this also
    // keeps the arrows from moving the text cursor). Enter is handled after the text box has
    // applied this frame's typing, so it always acts on the list the user is looking at.
    let (esc, up, down) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::Escape),
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
        )
    });
    let shown = commands::filter(&state.query).len();
    if shown > 0 {
        if down {
            state.selected = (state.selected + 1) % shown;
        }
        if up {
            state.selected = (state.selected + shown - 1) % shown;
        }
    }

    let mut close = esc;
    let mut run = None;

    let dark = ctx.theme() == egui::Theme::Dark;
    let window = egui::Window::new("command_palette")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 70.0))
        .fixed_size(vec2(480.0, 0.0))
        .show(ctx, |ui| {
            let before = state.query.clone();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .hint_text("Type a command...")
                    .desired_width(f32::INFINITY),
            );
            if !state.focused {
                edit.request_focus();
                state.focused = true;
            }
            if state.query != before {
                state.selected = 0;
            }
            // Recomputed after typing so Enter and the list agree.
            let entries = commands::filter(&state.query);
            state.selected = state.selected.min(entries.len().saturating_sub(1));
            // With no match Enter does nothing and the palette stays open; egui's text box lets go
            // of focus on Enter, so take it back or the next keystrokes would go elsewhere.
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                match entries.get(state.selected) {
                    Some(entry) => {
                        run = Some(entry.command);
                        close = true;
                    }
                    None => edit.request_focus(),
                }
            }

            ui.add_space(4.0);
            if entries.is_empty() {
                ui.weak("No matching command");
            }
            for (i, entry) in entries.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                let selected = i == state.selected;
                if selected || response.hovered() {
                    let fill = if selected {
                        theme::accent(dark).gamma_multiply(0.22)
                    } else {
                        ui.visuals().widgets.hovered.weak_bg_fill
                    };
                    ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
                }
                ui.painter().text(
                    rect.left_center() + vec2(8.0, 0.0),
                    Align2::LEFT_CENTER,
                    entry.title,
                    egui::FontId::proportional(14.0),
                    ui.visuals().text_color(),
                );
                if let Some(key) = entry.shortcut {
                    ui.painter().text(
                        rect.right_center() - vec2(8.0, 0.0),
                        Align2::RIGHT_CENTER,
                        format!("{}{}", theme::modifier(), key),
                        egui::FontId::proportional(12.0),
                        ui.visuals().weak_text_color(),
                    );
                }
                if response.clicked() {
                    run = Some(entry.command);
                    close = true;
                }
            }
            ui.add_space(2.0);
            ui.label(
                RichText::new("Up/Down to choose, Enter to run, Esc to close")
                    .small()
                    .weak(),
            );
        });

    // A click anywhere outside the palette dismisses it.
    if let Some(window) = &window
        && ctx.input(|i| i.pointer.any_pressed())
        && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
        && !window.response.rect.contains(pos)
    {
        close = true;
    }

    if !close {
        app.palette = Some(state);
    }
    if let Some(command) = run {
        app.run_command(command, ctx);
    }
}
