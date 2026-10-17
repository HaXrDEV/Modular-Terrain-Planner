//! Ctrl+K: find any command or tile by typing part of its name.

use eframe::egui::{self, Key, RichText};

use crate::commands::{COMMANDS, Command};
use crate::library::Library;
use mtp_core::tile::TileKey;
use mtp_core::units::TileSize;

/// What a palette entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    Run(Command),
    Place(TileKey, TileSize),
}

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    query: String,
    chosen: usize,
}

/// Whether every word of the query appears in the text.
fn matches(text: &str, query: &str) -> bool {
    let text = text.to_lowercase();
    query.split_whitespace().all(|w| text.contains(&w.to_lowercase()))
}

const MAX_TILES: usize = 30;

impl Palette {
    pub fn show(&mut self) {
        self.open = true;
        self.query.clear();
        self.chosen = 0;
    }

    /// Draws the palette; returns what was picked.
    pub fn ui(&mut self, ctx: &egui::Context, library: &Library) -> Option<Pick> {
        if !self.open {
            return None;
        }
        let mut entries: Vec<(String, String, Pick)> = COMMANDS
            .iter()
            .filter(|s| matches(s.label, &self.query))
            .map(|s| {
                let key = s.shortcut.map(|k| ctx.format_shortcut(&k)).unwrap_or_default();
                (s.label.trim_end_matches('…').to_owned(), key, Pick::Run(s.command))
            })
            .collect();
        if !self.query.trim().is_empty() {
            let mut tiles: Vec<_> = library.tiles().filter(|t| matches(&t.name, &self.query)).collect();
            tiles.sort_by_key(|t| t.name.to_lowercase());
            entries.extend(tiles.into_iter().take(MAX_TILES).map(|t| {
                (format!("Place {}", t.name), format!("{}×{}", t.size.w, t.size.h), Pick::Place(t.key.clone(), t.size))
            }));
        }
        self.chosen = self.chosen.min(entries.len().saturating_sub(1));

        let (down, up, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if down {
            self.chosen = (self.chosen + 1).min(entries.len().saturating_sub(1));
        }
        if up {
            self.chosen = self.chosen.saturating_sub(1);
        }
        let mut picked = None;
        if enter {
            picked = entries.get(self.chosen).map(|e| e.2.clone());
        }

        let modal = egui::Modal::new(egui::Id::new("command palette")).show(ctx, |ui| {
            ui.set_width(460.0);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Type a command or a tile name")
                    .desired_width(f32::INFINITY),
            );
            edit.request_focus();
            if edit.changed() {
                self.chosen = 0;
            }
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                if entries.is_empty() {
                    ui.label(RichText::new("Nothing matches.").weak());
                }
                for (i, (label, hint, pick)) in entries.iter().enumerate() {
                    let selected = i == self.chosen;
                    let resp = ui
                        .horizontal(|ui| {
                            let r = ui.selectable_label(selected, label);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(hint).weak());
                            });
                            r
                        })
                        .inner;
                    if selected {
                        resp.scroll_to_me(None);
                    }
                    if resp.clicked() {
                        picked = Some(pick.clone());
                    }
                }
            });
        });
        if picked.is_some() || escape || modal.should_close() {
            self.open = false;
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_must_match() {
        assert!(matches("Export build plan (PDF)", "build pdf"));
        assert!(matches("Export build plan (PDF)", "EXPORT"));
        assert!(!matches("Export build plan (PDF)", "build png"));
        assert!(matches("anything", "  "));
    }
}
