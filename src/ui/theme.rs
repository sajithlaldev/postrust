//! Colours, spacing and the two egui themes. Everything visual that is a design decision lives
//! here so the look can be tuned in one place.

use crate::model::{Method, ThemeMode};
use egui::{Color32, Context, CornerRadius, FontId, Margin, Stroke, TextStyle, Theme, ThemePreference, vec2};

pub fn accent(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(45, 212, 191)
    } else {
        Color32::from_rgb(13, 148, 136)
    }
}

/// Text colour on top of a filled accent button.
pub fn on_accent(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(8, 30, 28)
    } else {
        Color32::WHITE
    }
}

pub fn method_color(method: Method, dark: bool) -> Color32 {
    let (d, l) = match method {
        Method::Get => ((74, 222, 128), (22, 128, 61)),
        Method::Post => ((251, 146, 60), (194, 85, 12)),
        Method::Put => ((96, 165, 250), (37, 99, 235)),
        Method::Patch => ((192, 132, 252), (126, 34, 206)),
        Method::Delete => ((248, 113, 113), (200, 40, 40)),
        Method::Head | Method::Options => ((156, 163, 175), (95, 105, 120)),
    };
    let (r, g, b) = if dark { d } else { l };
    Color32::from_rgb(r, g, b)
}

pub fn method_short(method: Method) -> &'static str {
    match method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Patch => "PATCH",
        Method::Delete => "DEL",
        Method::Head => "HEAD",
        Method::Options => "OPT",
    }
}

pub fn status_color(code: u16, dark: bool) -> Color32 {
    match code {
        200..=299 => {
            if dark {
                Color32::from_rgb(74, 222, 128)
            } else {
                Color32::from_rgb(22, 128, 61)
            }
        }
        300..=399 => {
            if dark {
                Color32::from_rgb(96, 165, 250)
            } else {
                Color32::from_rgb(37, 99, 235)
            }
        }
        400..=499 => warning(dark),
        _ => error(dark),
    }
}

pub fn warning(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(251, 191, 36)
    } else {
        Color32::from_rgb(180, 110, 0)
    }
}

pub fn error(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(248, 113, 113)
    } else {
        Color32::from_rgb(200, 40, 40)
    }
}

/// Colours for JSON syntax highlighting.
#[derive(Clone, Copy)]
pub struct SyntaxColors {
    pub key: Color32,
    pub string: Color32,
    pub number: Color32,
    pub literal: Color32,
    pub punct: Color32,
    pub var: Color32,
    pub plain: Color32,
}

pub fn syntax_colors(dark: bool) -> SyntaxColors {
    if dark {
        SyntaxColors {
            key: Color32::from_rgb(126, 190, 255),
            string: Color32::from_rgb(152, 195, 121),
            number: Color32::from_rgb(209, 154, 102),
            literal: Color32::from_rgb(198, 120, 221),
            punct: Color32::from_rgb(150, 150, 150),
            var: Color32::from_rgb(240, 170, 60),
            plain: Color32::from_rgb(210, 210, 210),
        }
    } else {
        SyntaxColors {
            key: Color32::from_rgb(20, 90, 170),
            string: Color32::from_rgb(30, 125, 50),
            number: Color32::from_rgb(170, 90, 10),
            literal: Color32::from_rgb(140, 40, 170),
            punct: Color32::from_rgb(110, 110, 110),
            var: Color32::from_rgb(200, 100, 0),
            plain: Color32::from_rgb(40, 40, 40),
        }
    }
}

/// The platform's shortcut modifier as shown to the user ("Cmd+" on macOS, "Ctrl+" elsewhere).
pub fn modifier() -> &'static str {
    if cfg!(target_os = "macos") { "Cmd+" } else { "Ctrl+" }
}

/// Applies the user's System / Light / Dark choice.
pub fn apply_mode(ctx: &Context, mode: ThemeMode) {
    ctx.set_theme(match mode {
        ThemeMode::System => ThemePreference::System,
        ThemeMode::Light => ThemePreference::Light,
        ThemeMode::Dark => ThemePreference::Dark,
    });
}

/// Installs the custom look for both themes. Call once at startup.
pub fn install(ctx: &Context) {
    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            let dark = theme == Theme::Dark;
            let acc = accent(dark);
            let v = &mut style.visuals;
            if dark {
                v.panel_fill = Color32::from_rgb(24, 26, 31);
                v.window_fill = Color32::from_rgb(31, 34, 40);
                v.extreme_bg_color = Color32::from_rgb(17, 19, 23);
                v.faint_bg_color = Color32::from_rgb(32, 35, 41);
                v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(48, 52, 60));
                v.widgets.inactive.weak_bg_fill = Color32::from_rgb(44, 48, 56);
                v.widgets.inactive.bg_fill = Color32::from_rgb(44, 48, 56);
                v.widgets.hovered.weak_bg_fill = Color32::from_rgb(57, 62, 72);
                v.widgets.hovered.bg_fill = Color32::from_rgb(57, 62, 72);
                v.widgets.active.weak_bg_fill = Color32::from_rgb(68, 74, 86);
                v.widgets.active.bg_fill = Color32::from_rgb(68, 74, 86);
                v.selection.bg_fill = acc.gamma_multiply(0.30);
            } else {
                v.panel_fill = Color32::from_rgb(246, 247, 249);
                v.window_fill = Color32::WHITE;
                v.extreme_bg_color = Color32::WHITE;
                v.faint_bg_color = Color32::from_rgb(238, 240, 243);
                v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(220, 224, 230));
                v.widgets.inactive.weak_bg_fill = Color32::from_rgb(232, 235, 240);
                v.widgets.inactive.bg_fill = Color32::from_rgb(232, 235, 240);
                v.widgets.hovered.weak_bg_fill = Color32::from_rgb(221, 226, 233);
                v.widgets.hovered.bg_fill = Color32::from_rgb(221, 226, 233);
                v.widgets.active.weak_bg_fill = Color32::from_rgb(208, 214, 224);
                v.widgets.active.bg_fill = Color32::from_rgb(208, 214, 224);
                v.selection.bg_fill = acc.gamma_multiply(0.25);
            }
            v.selection.stroke = Stroke::new(1.0, acc);
            v.hyperlink_color = acc;
            v.window_corner_radius = CornerRadius::same(10);
            v.menu_corner_radius = CornerRadius::same(8);
            let r = CornerRadius::same(6);
            v.widgets.noninteractive.corner_radius = r;
            v.widgets.inactive.corner_radius = r;
            v.widgets.hovered.corner_radius = r;
            v.widgets.active.corner_radius = r;
            v.widgets.open.corner_radius = r;

            style.spacing.item_spacing = vec2(8.0, 6.0);
            style.spacing.button_padding = vec2(10.0, 5.0);
            style.spacing.interact_size.y = 26.0;
            style.spacing.window_margin = Margin::same(12);
            style.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0));
            style.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
            style.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
            style.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
            style.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.0));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_method_has_a_short_label_and_distinct_colours_per_theme() {
        for m in Method::ALL {
            assert!(!method_short(m).is_empty() && method_short(m).len() <= 5);
            assert_ne!(method_color(m, true), method_color(m, false), "{m:?}");
        }
        assert_ne!(method_color(Method::Get, true), method_color(Method::Delete, true));
    }

    #[test]
    fn status_colours_follow_the_class_of_the_code() {
        assert_eq!(status_color(204, true), status_color(200, true));
        assert_ne!(status_color(200, true), status_color(404, true));
        assert_eq!(status_color(500, false), status_color(503, false));
        assert_eq!(status_color(404, true), warning(true));
    }
}
