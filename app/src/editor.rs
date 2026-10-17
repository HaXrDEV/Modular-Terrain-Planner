//! The open project with its editing state: selection, active tool and
//! clipboard. Every edit goes through the document (and so the placement
//! rule); this adds what the UI needs around it.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use mtp_core::document::{DocState, Document};
use mtp_core::footprint::{self, Rect, Rotation, Turn};
use mtp_core::grid::GridSize;
use mtp_core::rules::{Mode, Proposal, Refusal};
use mtp_core::tile::{Placed, PlacedId, TileKey};
use mtp_core::units::TileSize;

#[derive(Clone, Debug, PartialEq)]
pub enum Tool {
    /// Click, Shift-click and box select; drag to move.
    Select,
    /// Places the chosen tile with each click (and while dragging).
    Place { key: TileKey, size: TileSize, rotation: Rotation },
    /// Removes tiles under the pointer.
    Erase,
    /// Places the clipboard where clicked.
    Paste,
}

impl Tool {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Place { .. } => "Place",
            Self::Erase => "Erase",
            Self::Paste => "Paste",
        }
    }
}

/// Copied tiles, positioned relative to their bounding box's corner.
#[derive(Clone, Debug)]
pub struct Clipboard {
    pub items: Vec<Proposal>,
    pub w: f32,
    pub h: f32,
}

impl Clipboard {
    fn from_tiles(tiles: &[&Placed]) -> Option<Self> {
        let min_x = tiles.iter().map(|t| t.x).fold(f32::INFINITY, f32::min);
        let min_y = tiles.iter().map(|t| t.y).fold(f32::INFINITY, f32::min);
        let min_z = tiles.iter().map(|t| t.z).fold(f32::INFINITY, f32::min);
        if !min_x.is_finite() {
            return None;
        }
        let (ox, oy) = (min_x.floor(), min_y.floor());
        let items: Vec<Proposal> = tiles
            .iter()
            .map(|t| Proposal { x: t.x - ox, y: t.y - oy, z: t.z - min_z, ..Proposal::from_placed(t) })
            .collect();
        let w = tiles.iter().map(|t| t.x + t.effective().0 as f32 - ox).fold(0.0, f32::max);
        let h = tiles.iter().map(|t| t.y + t.effective().1 as f32 - oy).fold(0.0, f32::max);
        Some(Self { items, w, h })
    }

    /// Turns the clipboard a quarter about its own centre, keeping its corner at 0,0.
    fn turn(&mut self, turn: Turn) {
        let pivot = [0.0, 0.0];
        let mut rects: Vec<Rect> = self
            .items
            .iter()
            .map(|p| {
                let (w, h) = footprint::effective_size(p.size, p.rotation);
                footprint::turn_rect(Rect { x: p.x, y: p.y, w: w as f32, h: h as f32 }, pivot, turn)
            })
            .collect();
        let min_x = rects.iter().map(|r| r.x).fold(f32::INFINITY, f32::min);
        let min_y = rects.iter().map(|r| r.y).fold(f32::INFINITY, f32::min);
        for (p, r) in self.items.iter_mut().zip(&mut rects) {
            p.x = r.x - min_x;
            p.y = r.y - min_y;
            p.rotation = p.rotation.turned(turn);
        }
        std::mem::swap(&mut self.w, &mut self.h);
    }

    /// The clipboard's tiles with their corner at `x`, `y`.
    pub fn at(&self, x: f32, y: f32) -> Vec<Proposal> {
        self.items.iter().map(|p| Proposal { x: p.x + x, y: p.y + y, ..p.clone() }).collect()
    }
}

/// A message for the status bar, which fades after a few seconds.
#[derive(Clone, Debug)]
pub struct Notice {
    pub text: String,
    pub warning: bool,
    pub at: Instant,
}

pub struct Editor {
    pub doc: Document,
    /// Where the project is saved, if it has been.
    pub path: Option<PathBuf>,
    pub selection: BTreeSet<PlacedId>,
    pub tool: Tool,
    pub clipboard: Option<Clipboard>,
    pub notice: Option<Notice>,
    /// Pivot kept while the same selection is turned again, so four turns
    /// bring it back.
    turn_pivot: Option<(BTreeSet<PlacedId>, [f32; 2], u64)>,
    /// Increases with every change to the document, for caches.
    pub revision: u64,
}

impl Editor {
    pub fn new(doc: Document, path: Option<PathBuf>) -> Self {
        Self {
            doc,
            path,
            selection: BTreeSet::new(),
            tool: Tool::Select,
            clipboard: None,
            notice: None,
            turn_pivot: None,
            revision: 0,
        }
    }

    pub fn empty(grid: GridSize) -> Self {
        Self::new(Document::new(DocState::empty(grid)), None)
    }

    pub fn title(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "Untitled".to_owned(), |s| s.to_string_lossy().into_owned())
    }

    pub fn tell(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), warning: false, at: Instant::now() });
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), warning: true, at: Instant::now() });
    }

    /// After any change: drop selected tiles that no longer exist.
    pub fn changed(&mut self) {
        let doc = &self.doc;
        self.selection.retain(|id| doc.contains(*id));
        self.revision += 1;
    }

    fn report(&mut self, result: Result<(), Refusal>) -> bool {
        match result {
            Ok(()) => {
                self.changed();
                true
            }
            Err(r) => {
                self.warn(r.to_string());
                false
            }
        }
    }

    pub fn selected_tiles(&self) -> Vec<&Placed> {
        self.doc.tiles().iter().filter(|t| self.selection.contains(&t.id)).collect()
    }

    pub fn place(&mut self, proposals: Vec<Proposal>, mode: Mode) -> Option<Vec<PlacedId>> {
        let label = if proposals.len() == 1 { "Place tile" } else { "Place tiles" };
        match self.doc.place(label, proposals, mode) {
            Ok(ids) => {
                self.changed();
                Some(ids)
            }
            Err(r) => {
                self.warn(r.to_string());
                None
            }
        }
    }

    pub fn delete(&mut self, ids: &[PlacedId]) {
        let n = self.doc.remove(if ids.len() == 1 { "Delete tile" } else { "Delete tiles" }, ids);
        if n > 0 {
            self.changed();
        }
    }

    pub fn delete_selection(&mut self) {
        let ids: Vec<PlacedId> = self.selection.iter().copied().collect();
        self.delete(&ids);
    }

    pub fn select_all(&mut self) {
        self.selection = self.doc.tiles().iter().map(|t| t.id).collect();
    }

    /// Selects every copy of the tiles in the selection (or of `key`).
    pub fn select_same(&mut self, key: &TileKey) {
        self.selection = self.doc.tiles().iter().filter(|t| t.key == *key).map(|t| t.id).collect();
        let n = self.selection.len();
        self.tell(format!("Selected {n} {}.", if n == 1 { "tile" } else { "copies" }));
    }

    pub fn copy(&mut self) -> bool {
        let tiles = self.selected_tiles();
        match Clipboard::from_tiles(&tiles) {
            Some(c) => {
                let n = c.items.len();
                self.clipboard = Some(c);
                self.tell(format!("Copied {n} {}.", if n == 1 { "tile" } else { "tiles" }));
                true
            }
            None => false,
        }
    }

    pub fn cut(&mut self) {
        if self.copy() {
            let ids: Vec<PlacedId> = self.selection.iter().copied().collect();
            self.doc.remove("Cut", &ids);
            self.changed();
        }
    }

    pub fn paste_at(&mut self, x: f32, y: f32, mode: Mode) -> bool {
        let Some(clip) = &self.clipboard else { return false };
        let proposals = clip.at(x, y);
        match self.doc.place("Paste", proposals, mode) {
            Ok(ids) => {
                self.selection = ids.into_iter().collect();
                self.changed();
                true
            }
            Err(r) => {
                self.warn(r.to_string());
                false
            }
        }
    }

    pub fn move_selection(&mut self, dx: f32, dy: f32, mode: Mode) -> bool {
        let ids: Vec<PlacedId> = self.selection.iter().copied().collect();
        let result = self.doc.move_by(&ids, dx, dy, mode);
        self.report(result)
    }

    /// Turns what's being worked on: the tile being placed, the clipboard
    /// while pasting, or the selection.
    pub fn turn(&mut self, turn: Turn, mode: Mode) {
        match &mut self.tool {
            Tool::Place { rotation, .. } => *rotation = rotation.turned(turn),
            Tool::Paste => {
                if let Some(c) = &mut self.clipboard {
                    c.turn(turn);
                }
            }
            Tool::Select | Tool::Erase => {
                if self.selection.is_empty() {
                    return;
                }
                // Only a turn straight after a turn of the same tiles keeps the
                // pivot; after any other change (a move, a nudge) it's worked out again
                let pivot = match &self.turn_pivot {
                    Some((sel, p, rev)) if *sel == self.selection && *rev == self.revision => *p,
                    _ => footprint::group_pivot(&self.selected_tiles().iter().map(|t| t.rect()).collect::<Vec<_>>()),
                };
                let ids: Vec<PlacedId> = self.selection.iter().copied().collect();
                let result = self.doc.turn(&ids, pivot, turn, mode);
                if self.report(result) {
                    self.turn_pivot = Some((self.selection.clone(), pivot, self.revision));
                }
            }
        }
    }

    pub fn replace_selection(&mut self, key: &TileKey, size: TileSize, mode: Mode) {
        let ids: Vec<PlacedId> = self.selection.iter().copied().collect();
        let result = self.doc.replace_with(&ids, key, size, mode);
        self.report(result);
    }

    pub fn undo(&mut self) {
        let label = self.doc.undo_label().map(str::to_owned);
        if self.doc.undo() {
            self.changed();
            self.tell(format!("Undid {}.", label.unwrap_or_default().to_lowercase()));
        }
    }

    pub fn redo(&mut self) {
        if self.doc.redo() {
            self.changed();
            let label = self.doc.undo_label().map(str::to_owned).unwrap_or_default();
            self.tell(format!("Redid {}.", label.to_lowercase()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with(n: usize) -> Editor {
        let mut e = Editor::empty(GridSize { cols: 30, rows: 30 });
        let proposals = (0..n)
            .map(|i| Proposal {
                key: TileKey::new("C:/t/floor.stl"),
                size: TileSize { w: 2, h: 1, gz: 0.5 },
                size_known: true,
                x: 4.0 + 2.0 * i as f32,
                y: 4.0,
                z: 0.0,
                rotation: Rotation::ZERO,
                level: 0,
            })
            .collect();
        e.place(proposals, Mode::Snap).expect("fits");
        e
    }

    #[test]
    fn copy_paste_keeps_the_shape() {
        let mut e = editor_with(3);
        e.select_all();
        e.copy();
        assert!(e.paste_at(10.0, 10.0, Mode::Snap));
        let pasted = e.selected_tiles();
        assert_eq!(pasted.len(), 3);
        let xs: Vec<f32> = pasted.iter().map(|t| t.x).collect();
        assert_eq!(xs, [10.0, 12.0, 14.0]);
    }

    #[test]
    fn turning_the_clipboard_four_times_is_the_identity() {
        let mut e = editor_with(3);
        e.select_all();
        e.copy();
        e.tool = Tool::Paste;
        let before = e.clipboard.as_ref().map(|c| c.at(0.0, 0.0));
        for _ in 0..4 {
            e.turn(Turn::Clockwise, Mode::Snap);
        }
        assert_eq!(e.clipboard.as_ref().map(|c| c.at(0.0, 0.0)), before);
    }

    #[test]
    fn undo_prunes_the_selection() {
        let mut e = editor_with(1);
        e.select_all();
        e.undo();
        assert!(e.selection.is_empty(), "the undone tile can't stay selected");
    }

    #[test]
    fn a_moved_selection_turns_about_its_new_place() {
        let mut e = editor_with(1); // a 2x1 floor at (4, 4)
        e.select_all();
        e.turn(Turn::Clockwise, Mode::Snap);
        e.move_selection(0.0, 10.0, Mode::Snap);
        let before = e.selected_tiles()[0].rect();
        e.turn(Turn::Clockwise, Mode::Snap);
        let after = e.selected_tiles()[0].rect();
        assert!((after.y - before.y).abs() <= 2.0, "turned in place, not about the old pivot: {before:?} -> {after:?}");
    }

    #[test]
    fn repeated_turns_keep_their_pivot() {
        let mut e = editor_with(3);
        e.select_all();
        let before: Vec<(f32, f32)> = e.doc.tiles().iter().map(|t| (t.x, t.y)).collect();
        for _ in 0..4 {
            e.turn(Turn::CounterClockwise, Mode::Snap);
        }
        let after: Vec<(f32, f32)> = e.doc.tiles().iter().map(|t| (t.x, t.y)).collect();
        assert_eq!(before, after);
    }
}
