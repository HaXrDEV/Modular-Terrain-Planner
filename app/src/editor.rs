//! The open project with its editing state: selection, active tool and
//! clipboard. Every edit goes through the document (and so the placement
//! rule); this adds what the UI needs around it.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use mtp_core::document::{DocState, Document};
use mtp_core::footprint::{self, Rect, Rotation, Turn};
use mtp_core::grid::GridSize;
use mtp_core::rules::{self, Height, Mode, Proposal, Refusal};
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
    /// Drag to measure a distance.
    Measure,
}

impl Tool {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Place { .. } => "Place",
            Self::Erase => "Erase",
            Self::Paste => "Paste",
            Self::Measure => "Measure",
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
    pub fn from_tiles(tiles: &[&Placed]) -> Option<Self> {
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
    /// Where the last tile was placed, for Shift+click lines.
    pub last_placed: Option<(f32, f32)>,
    /// Tiles placed recently, newest first, for keys 1 to 9.
    pub recent: Vec<(TileKey, TileSize)>,
    /// The level being edited; new tiles go on it.
    pub level: i32,
    pub level_view: LevelView,
}

/// How the levels other than the one being edited are shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LevelView {
    /// Only the active level.
    Alone,
    /// Levels below the active one, faded.
    #[default]
    Faded,
    /// Every level, stacked on top of each other.
    Stacked,
}

/// How many recent tiles keys 1 to 9 reach.
pub const HOTBAR: usize = 9;

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
            last_placed: None,
            recent: Vec::new(),
            level: 0,
            level_view: LevelView::default(),
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

    fn remember(&mut self, p: &Proposal) {
        self.last_placed = Some((p.x, p.y));
        self.recent.retain(|(k, _)| *k != p.key);
        self.recent.insert(0, (p.key.clone(), p.size));
        self.recent.truncate(HOTBAR);
    }

    pub fn place(&mut self, proposals: Vec<Proposal>, mode: Mode) -> Option<Vec<PlacedId>> {
        let label = if proposals.len() == 1 { "Place tile" } else { "Place tiles" };
        let last = proposals.last().cloned();
        match self.doc.place(label, proposals, mode) {
            Ok(ids) => {
                self.changed();
                if let Some(p) = last {
                    self.remember(&p);
                }
                Some(ids)
            }
            Err(r) => {
                self.warn(r.to_string());
                None
            }
        }
    }

    /// Which of `proposals` fit, each checked against the document and the
    /// ones before it, with the height each would get. For fills and
    /// lines, which skip what doesn't fit instead of refusing everything.
    pub fn fit(&self, proposals: &[Proposal], mode: Mode) -> Vec<(Proposal, Option<f32>)> {
        let grid = self.doc.grid();
        let mut accepted: Vec<Placed> = self.doc.tiles().to_vec();
        let none = std::collections::HashSet::new();
        proposals
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let z = rules::arrange(grid, &accepted, &none, std::slice::from_ref(p), mode, Height::Settle)
                    .ok()
                    .map(|z| z[0]);
                if let Some(z) = z {
                    accepted.push(p.clone().into_placed(PlacedId(u64::MAX - i as u64), z));
                }
                (p.clone(), z)
            })
            .collect()
    }

    /// Places every proposal that fits, as one undo step. Returns how many
    /// were placed and how many skipped.
    pub fn fill(&mut self, proposals: Vec<Proposal>, mode: Mode) -> (usize, usize) {
        let fitted = self.fit(&proposals, mode);
        let total = fitted.len();
        let ok: Vec<Proposal> = fitted.into_iter().filter_map(|(p, z)| z.map(|_| p)).collect();
        let placed = ok.len();
        if placed > 0 {
            let last = ok.last().cloned();
            if self.doc.place(if placed == 1 { "Place tile" } else { "Fill" }, ok, mode).is_ok() {
                self.changed();
                if let Some(p) = last {
                    self.remember(&p);
                }
            }
        }
        let skipped = total - placed;
        if skipped > 0 {
            self.tell(format!("Placed {placed} tiles; {skipped} didn't fit and were skipped."));
        } else if placed > 1 {
            self.tell(format!("Placed {placed} tiles."));
        }
        (placed, skipped)
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
        self.selection = self.doc.tiles().iter().filter(|t| t.level == self.level).map(|t| t.id).collect();
    }

    /// Selects every copy of the tiles in the selection (or of `key`).
    pub fn select_same(&mut self, key: &TileKey) {
        self.selection =
            self.doc.tiles().iter().filter(|t| t.key == *key && t.level == self.level).map(|t| t.id).collect();
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
        let level = self.level;
        let proposals: Vec<Proposal> = clip.at(x, y).into_iter().map(|p| Proposal { level, ..p }).collect();
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
            Tool::Select | Tool::Erase | Tool::Measure => {
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

    /// Puts walls (and corner pieces) round the selected tiles' cells, on
    /// top of them, facing in. Returns how many edge stretches were too
    /// short for a wall.
    pub fn add_walls(&mut self, wall: (TileKey, TileSize), corner: Option<(TileKey, TileSize)>) -> usize {
        use mtp_core::outline::{WallShape, walls_around};
        let cells: std::collections::HashSet<(i32, i32)> =
            self.selected_tiles().iter().flat_map(|t| t.cells().collect::<Vec<_>>()).collect();
        let (key, size) = wall;
        let shape = if size.w >= size.h { WallShape::AlongX } else { WallShape::AlongY };
        let plan = walls_around(&cells, size.w.max(size.h), shape, corner.is_some());
        let level = self.level;
        let proposal = |key: &TileKey, size: TileSize, p: &mtp_core::outline::Piece| Proposal {
            key: key.clone(),
            size,
            size_known: true,
            x: p.x,
            y: p.y,
            z: 0.0,
            rotation: p.rotation,
            level,
        };
        let mut proposals: Vec<Proposal> = plan.walls.iter().map(|p| proposal(&key, size, p)).collect();
        if let Some((ck, cs)) = &corner {
            proposals.extend(plan.corners.iter().map(|p| proposal(ck, *cs, p)));
        }
        self.fill(proposals, Mode::Snap);
        plan.gaps
    }

    /// Switches the level being edited; the selection stays on its level.
    pub fn set_level(&mut self, level: i32) {
        if level != self.level {
            self.level = level;
            let doc = &self.doc;
            self.selection.retain(|id| doc.tile(*id).is_some_and(|t| t.level == level));
            self.tell(format!("Editing level {level}."));
        }
    }

    pub fn move_selection_to_level(&mut self, level: i32, mode: Mode) {
        let ids: Vec<PlacedId> = self.selection.iter().copied().collect();
        let result = self.doc.move_to_level(&ids, level, mode);
        if self.report(result) {
            self.level = level;
            self.tell(format!("Moved {} tiles to level {level}.", ids.len()));
        }
    }

    /// Swaps every selected tile for a tile of the same footprint from
    /// `choices(tile)`, picked at random, so a floor of one tile doesn't look
    /// repetitive. One undo step.
    pub fn randomise_selection(&mut self, choices: impl Fn(&Placed) -> Vec<(TileKey, TileSize)>, mode: Mode) {
        let mut seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9, |d| d.as_nanos() as u64)
            | 1;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let changes: Vec<(PlacedId, TileKey, TileSize)> = self
            .selected_tiles()
            .into_iter()
            .filter_map(|t| {
                let options = choices(t);
                (options.len() > 1).then(|| {
                    let (key, size) = options[(next() % options.len() as u64) as usize].clone();
                    (t.id, key, size)
                })
            })
            .collect();
        if changes.is_empty() {
            self.tell("No other tiles of the same size to mix in.");
            return;
        }
        let n = changes.len();
        let result = self.doc.replace_each("Randomise variants", &changes, mode);
        if self.report(result) {
            self.tell(format!("Mixed variants into {n} tiles."));
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
    fn fills_skip_what_does_not_fit() {
        let mut e = editor_with(1); // a 2x1 floor at (4, 4)
        let row: Vec<Proposal> = (0..4)
            .map(|i| Proposal {
                key: TileKey::new("C:/t/floor.stl"),
                size: TileSize { w: 2, h: 1, gz: 0.5 },
                size_known: true,
                x: 2.0 + 2.0 * i as f32,
                y: 4.0,
                z: 0.0,
                rotation: Rotation::ZERO,
                level: 0,
            })
            .collect();
        // (4, 4) is taken at ground level but a stacked copy fits there
        let (placed, skipped) = e.fill(row, Mode::Snap);
        assert_eq!((placed, skipped), (4, 0));
        assert_eq!(e.doc.tiles().len(), 5);
        e.undo();
        assert_eq!(e.doc.tiles().len(), 1, "a fill is one undo step");
        assert_eq!(e.recent.len(), 1);
    }

    #[test]
    fn walls_go_round_a_room_on_top_of_the_floor() {
        let mut e = Editor::empty(GridSize { cols: 30, rows: 30 });
        let floor = |x: f32, y: f32| Proposal {
            key: TileKey::new("C:/t/floor.stl"),
            size: TileSize { w: 2, h: 2, gz: 0.5 },
            size_known: true,
            x,
            y,
            z: 0.0,
            rotation: Rotation::ZERO,
            level: 0,
        };
        let floors: Vec<Proposal> =
            (0..3).flat_map(|i| (0..2).map(move |j| floor(2.0 * i as f32, 2.0 * j as f32))).collect();
        e.place(floors, Mode::Snap).expect("floor");
        e.select_all();
        let wall = (TileKey::new("C:/t/wall.stl"), TileSize { w: 2, h: 1, gz: 4.0 });
        let corner = (TileKey::new("C:/t/pillar.stl"), TileSize { w: 1, h: 1, gz: 4.5 });
        let gaps = e.add_walls(wall, Some(corner));
        assert_eq!(gaps, 0);
        let walls: Vec<_> = e.doc.tiles().iter().filter(|t| t.key.stem() == "wall").collect();
        let pillars: Vec<_> = e.doc.tiles().iter().filter(|t| t.key.stem() == "pillar").collect();
        assert_eq!((walls.len(), pillars.len()), (6, 4));
        assert!(walls.iter().chain(&pillars).all(|t| t.z == 0.5), "everything stands on the floor");
        e.undo();
        assert_eq!(e.doc.tiles().len(), 6, "one undo step");
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
