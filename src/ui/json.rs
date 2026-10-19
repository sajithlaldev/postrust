//! JSON syntax highlighting for the response viewer and the raw body editor.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, TextBuffer, Ui};
use std::sync::Arc;

fn is_special(b: u8) -> bool {
    matches!(b, b'"' | b'{' | b'}' | b'[' | b']' | b',' | b':') || b.is_ascii_whitespace()
}

/// Colours JSON-looking text; anything else is returned as one plain section.
pub fn highlight(text: &str, dark: bool, size: f32, plain: Color32) -> LayoutJob {
    let mut pal = super::theme::syntax_colors(dark);
    pal.plain = plain; // follow the active egui theme for ordinary text
    let font = FontId::monospace(size);
    let mut job = LayoutJob::default();
    let mut push = |s: &str, color: Color32| {
        if !s.is_empty() {
            job.append(s, 0.0, TextFormat::simple(font.clone(), color));
        }
    };

    let trimmed = text.trim_start();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        push(text, pal.plain);
        return job;
    }

    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        let c = bytes[i];
        // Every token ends at an ASCII byte (or the end), so all slice bounds are char boundaries.
        let end = match c {
            b'"' => {
                let mut j = i + 1;
                while j < len {
                    match bytes[j] {
                        b'\\' => j += 2,
                        b'"' => {
                            j += 1;
                            break;
                        }
                        _ => j += 1,
                    }
                }
                let j = j.min(len);
                let mut k = j;
                while k < len && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                let base = if bytes.get(k) == Some(&b':') {
                    pal.key
                } else {
                    pal.string
                };
                // `{{variables}}` inside a string get their own colour.
                let slice = &text[i..j];
                let (mut from, mut at) = (0, 0);
                while at < slice.len() {
                    match crate::json::find_placeholder(slice, at) {
                        Some(e) => {
                            push(&slice[from..at], base);
                            push(&slice[at..e], pal.var);
                            (from, at) = (e, e);
                        }
                        None => at += 1,
                    }
                }
                push(&slice[from..], base);
                j
            }
            b'{' => match crate::json::find_placeholder(text, i) {
                Some(j) => {
                    push(&text[i..j], pal.var);
                    j
                }
                None => {
                    push("{", pal.punct);
                    i + 1
                }
            },
            b'}' | b'[' | b']' | b',' | b':' => {
                push(&text[i..i + 1], pal.punct);
                i + 1
            }
            b if b.is_ascii_whitespace() => {
                let mut j = i;
                while j < len && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                push(&text[i..j], pal.plain);
                j
            }
            _ => {
                let mut j = i;
                while j < len && !is_special(bytes[j]) {
                    j += 1;
                }
                let word = &text[i..j];
                let color = if matches!(word, "true" | "false" | "null") {
                    pal.literal
                } else if c == b'-' || c.is_ascii_digit() {
                    pal.number
                } else {
                    pal.plain
                };
                push(word, color);
                j
            }
        };
        i = end;
    }
    job
}

#[derive(Default)]
struct Highlighter;

impl egui::cache::ComputerMut<(&str, bool, u32, [u8; 4]), LayoutJob> for Highlighter {
    fn compute(&mut self, (text, dark, size_bits, plain): (&str, bool, u32, [u8; 4])) -> LayoutJob {
        let plain = Color32::from_rgba_premultiplied(plain[0], plain[1], plain[2], plain[3]);
        highlight(text, dark, f32::from_bits(size_bits), plain)
    }
}

type HighlightCache = egui::cache::FrameCache<LayoutJob, Highlighter>;

/// A `TextEdit` layouter that highlights JSON. The layout job is memoised per frame, so large
/// bodies are only tokenised when the text (or theme) changes.
pub fn layouter() -> impl FnMut(&Ui, &dyn TextBuffer, f32) -> Arc<egui::Galley> {
    |ui, buf, wrap_width| {
        let dark = ui.visuals().dark_mode;
        let size = egui::TextStyle::Monospace.resolve(ui.style()).size;
        let plain = ui.visuals().text_color().to_array();
        let mut job = ui.ctx().memory_mut(|m| {
            m.caches
                .cache::<HighlightCache>()
                .get((buf.as_str(), dark, size.to_bits(), plain))
                .clone()
        });
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(text: &str) -> Vec<(String, Color32)> {
        highlight(text, true, 14.0, Color32::WHITE)
            .sections
            .iter()
            .map(|s| {
                (
                    text[s.byte_range.start.0..s.byte_range.end.0].to_string(),
                    s.format.color,
                )
            })
            .collect()
    }

    #[test]
    fn covers_every_byte_exactly_once() {
        for text in [
            r#"{"a": 1, "b": [true, null, "x\"y"], "c": {{var}}, "d": -1.5e3}"#,
            "{\"é€😀\": \"ü\\\\\", \"k\": {{ x }}}",
            "{\"unterminated\": \"abc",
            "[1, 2,",
            "{{",
            "{ {{ no end",
        ] {
            let joined: String = sections(text).into_iter().map(|(s, _)| s).collect();
            assert_eq!(joined, text);
        }
    }

    #[test]
    fn colours_keys_values_and_variables() {
        let pal = crate::ui::theme::syntax_colors(true);
        let s = sections(r#"{"k": "v", "n": 12, "b": true, "x": {{id}}}"#);
        let color_of = |needle: &str| s.iter().find(|(t, _)| t == needle).map(|(_, c)| *c).unwrap();
        assert_eq!(color_of("\"k\""), pal.key);
        assert_eq!(color_of("\"v\""), pal.string);
        assert_eq!(color_of("12"), pal.number);
        assert_eq!(color_of("true"), pal.literal);
        assert_eq!(color_of("{{id}}"), pal.var);
    }

    #[test]
    fn variables_inside_strings_are_coloured_and_plain_follows_theme() {
        let pal = crate::ui::theme::syntax_colors(true);
        let s = sections(r#"{"name": "hi {{who}}!", "x": 1}"#);
        let color_of = |needle: &str| s.iter().find(|(t, _)| t == needle).map(|(_, c)| *c).unwrap();
        assert_eq!(color_of("{{who}}"), pal.var);
        assert_eq!(color_of("\"hi "), pal.string);
        assert_eq!(color_of("!\""), pal.string);
        let custom = Color32::from_rgb(1, 2, 3);
        let job = highlight("hello", true, 14.0, custom);
        assert_eq!(job.sections[0].format.color, custom);
    }

    #[test]
    fn non_json_is_left_plain() {
        let s = sections("<html>{\"a\":1}</html>");
        assert_eq!(s.len(), 1);
    }
}
