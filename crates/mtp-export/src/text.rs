//! The bundled font, measured and laid out in one place so the PNG and the
//! PDF place every glyph identically.

use std::ops::Range;
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, GlyphId};

use crate::ExportError;

/// Cantarell, SIL Open Font License 1.1 (see `assets/fonts/OFL.txt`).
pub(crate) static FONT_BYTES: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/Cantarell-Regular.ttf"));

/// A glyph placed on a line: advance in ems, and the text it stands for.
#[derive(Clone, Debug)]
pub(crate) struct LaidGlyph {
    pub id: u16,
    /// Offset of the glyph's origin from the start of the line, in ems.
    pub x: f32,
    pub advance: f32,
    pub range: Range<usize>,
}

pub(crate) struct Typeface {
    pub font: FontRef<'static>,
    upem: f32,
}

/// The font, parsed once.
pub(crate) fn typeface() -> Result<&'static Typeface, ExportError> {
    static FACE: OnceLock<Option<Typeface>> = OnceLock::new();
    FACE.get_or_init(|| {
        let font = FontRef::try_from_slice(FONT_BYTES).ok()?;
        let upem = font.units_per_em()?;
        Some(Typeface { font, upem })
    })
    .as_ref()
    .ok_or(ExportError::Font)
}

impl Typeface {
    pub fn units_per_em(&self) -> f32 {
        self.upem
    }

    /// The text with anything the font can't draw replaced: control
    /// characters by spaces, characters without a glyph by `?`.
    pub fn clean(&self, s: &str) -> String {
        s.chars()
            .map(|c| {
                if c.is_control() {
                    ' '
                } else if !c.is_whitespace() && self.font.glyph_id(c).0 == 0 {
                    '?'
                } else {
                    c
                }
            })
            .collect()
    }

    /// Glyphs for an already [cleaned](Self::clean) line, with kerning.
    pub fn layout(&self, s: &str) -> Vec<LaidGlyph> {
        let mut out: Vec<LaidGlyph> = Vec::with_capacity(s.len());
        let mut x = 0.0;
        let mut prev: Option<GlyphId> = None;
        for (i, c) in s.char_indices() {
            let id = self.font.glyph_id(c);
            if let Some(p) = prev {
                x += self.font.kern_unscaled(p, id) / self.upem;
            }
            let advance = self.font.h_advance_unscaled(id) / self.upem;
            out.push(LaidGlyph { id: id.0, x, advance, range: i..i + c.len_utf8() });
            x += advance;
            prev = Some(id);
        }
        out
    }

    /// Width of a cleaned line at `size` (the em size, in output units).
    pub fn width(&self, s: &str, size: f32) -> f32 {
        self.layout(s).last().map_or(0.0, |g| (g.x + g.advance) * size)
    }

    fn ellipsis(&self) -> &'static str {
        if self.font.glyph_id('\u{2026}').0 != 0 { "\u{2026}" } else { "..." }
    }

    /// Cleans `s` and shortens it with an ellipsis to fit `max_width`.
    /// Returns an empty string when not even the ellipsis fits.
    pub fn fit(&self, s: &str, size: f32, max_width: f32) -> String {
        let s = self.clean(s);
        if self.width(&s, size) <= max_width {
            return s;
        }
        let dots = self.ellipsis();
        let mut chars: Vec<char> = s.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let candidate: String = chars.iter().collect::<String>().trim_end().to_string() + dots;
            if self.width(&candidate, size) <= max_width {
                return candidate;
            }
        }
        String::new()
    }

    /// Cleans `s` and breaks it into lines no wider than `max_width`, at
    /// spaces where possible and inside long words otherwise. Never drops text.
    pub fn wrap(&self, s: &str, size: f32, max_width: f32) -> Vec<String> {
        let s = self.clean(s);
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in s.split(' ').filter(|w| !w.is_empty()) {
            let joined = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if self.width(&joined, size) <= max_width {
                line = joined;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // Break a word too long for a line of its own
            for c in word.chars() {
                let mut next = line.clone();
                next.push(c);
                if !line.is_empty() && self.width(&next, size) > max_width {
                    lines.push(std::mem::take(&mut line));
                    line.push(c);
                } else {
                    line = next;
                }
            }
        }
        if !line.is_empty() || lines.is_empty() {
            lines.push(line);
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_glyphs_become_question_marks() {
        let face = typeface().expect("font");
        assert_eq!(face.clean("Floor\u{4e00}\n"), "Floor? ");
    }

    #[test]
    fn wrapping_keeps_every_character() {
        let face = typeface().expect("font");
        let text = "Ruined tower with a staircase_and_an_extremely_long_name_without_spaces";
        let lines = face.wrap(text, 10.0, 60.0);
        assert!(lines.len() > 2);
        assert!(lines.iter().all(|l| face.width(l, 10.0) <= 60.0 || l.chars().count() == 1));
        assert_eq!(lines.concat().replace(' ', ""), text.replace(' ', ""));
    }

    #[test]
    fn fitting_shortens_with_an_ellipsis() {
        let face = typeface().expect("font");
        let fitted = face.fit("A rather long tile name", 10.0, 50.0);
        assert!(face.width(&fitted, 10.0) <= 50.0);
        assert!(fitted.ends_with(face.ellipsis()));
        assert_eq!(face.fit("Short", 10.0, 500.0), "Short");
    }
}
