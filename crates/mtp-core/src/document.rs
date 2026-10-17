//! The open project: grid, placed tiles and battle map, edited only through
//! methods that apply the placement rules and record one undo step each.

use std::collections::HashSet;
use std::sync::Arc;

use crate::footprint::{self, Turn};
use crate::grid::{CellIndex, GridSize};
use crate::history::{History, SaveToken};
use crate::rules::{self, Height, Mode, Proposal, Refusal};
use crate::tile::{Placed, PlacedId, TileKey};
use crate::units::TileSize;

/// An image laid on the ground under the grid, such as a battle map.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundImage {
    /// Path as saved (resolved by the app against the project's folder).
    pub path: String,
    /// `[x, y, width, height]` in cells.
    pub rect: [f32; 4],
}

/// Everything undo can take back.
#[derive(Clone, Debug, PartialEq)]
pub struct DocState {
    pub grid: GridSize,
    pub tiles: Vec<Placed>,
    pub ground_image: Option<GroundImage>,
}

impl DocState {
    pub fn empty(grid: GridSize) -> Self {
        Self { grid, tiles: Vec::new(), ground_image: None }
    }
}

#[derive(Debug)]
pub struct Document {
    state: Arc<DocState>,
    history: History<DocState>,
    index: CellIndex,
    next_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new(DocState::empty(GridSize::DEFAULT))
    }
}

impl Document {
    /// Opens a state exactly as given: overlapping or off-grid tiles from an
    /// old file are kept, not dropped as the Python version did.
    pub fn new(mut state: DocState) -> Self {
        let mut next_id = state.tiles.iter().map(|t| t.id.0 + 1).max().unwrap_or(1);
        let mut seen = HashSet::new();
        for t in &mut state.tiles {
            if !seen.insert(t.id) {
                t.id = PlacedId(next_id);
                next_id += 1;
            }
        }
        let state = Arc::new(state);
        Self { index: CellIndex::build(&state.tiles), history: History::new(state.clone()), state, next_id }
    }

    pub fn state(&self) -> &DocState {
        &self.state
    }

    pub fn tiles(&self) -> &[Placed] {
        &self.state.tiles
    }

    pub fn grid(&self) -> GridSize {
        self.state.grid
    }

    pub fn index(&self) -> &CellIndex {
        &self.index
    }

    pub fn tile(&self, id: PlacedId) -> Option<&Placed> {
        self.state.tiles.iter().find(|t| t.id == id)
    }

    pub fn contains(&self, id: PlacedId) -> bool {
        self.tile(id).is_some()
    }

    pub fn ground_image(&self) -> Option<&GroundImage> {
        self.state.ground_image.as_ref()
    }

    fn commit(&mut self, label: &str, state: DocState) {
        if state != *self.state {
            self.state = Arc::new(state);
            self.history.push(label, self.state.clone());
            self.index = CellIndex::build(&self.state.tiles);
        }
    }

    fn adopt(&mut self, state: Arc<DocState>) {
        self.state = state;
        self.index = CellIndex::build(&self.state.tiles);
    }

    fn new_id(&mut self) -> PlacedId {
        self.next_id += 1;
        PlacedId(self.next_id - 1)
    }

    fn selected(&self, ids: &[PlacedId]) -> (HashSet<PlacedId>, Vec<&Placed>) {
        let set: HashSet<PlacedId> = ids.iter().copied().collect();
        let tiles = self.state.tiles.iter().filter(|t| set.contains(&t.id)).collect();
        (set, tiles)
    }

    /// Places new tiles, settled on what's under them. All or nothing.
    pub fn place(&mut self, label: &str, proposals: Vec<Proposal>, mode: Mode) -> Result<Vec<PlacedId>, Refusal> {
        let heights =
            rules::arrange(self.grid(), &self.state.tiles, &HashSet::new(), &proposals, mode, Height::Settle)?;
        let mut state = (*self.state).clone();
        let mut ids = Vec::with_capacity(proposals.len());
        for (p, z) in proposals.into_iter().zip(heights) {
            let id = self.new_id();
            ids.push(id);
            state.tiles.push(p.into_placed(id, z));
        }
        self.commit(label, state);
        Ok(ids)
    }

    /// Removes tiles; returns how many there were.
    pub fn remove(&mut self, label: &str, ids: &[PlacedId]) -> usize {
        let set: HashSet<PlacedId> = ids.iter().copied().collect();
        let mut state = (*self.state).clone();
        let before = state.tiles.len();
        state.tiles.retain(|t| !set.contains(&t.id));
        let removed = before - state.tiles.len();
        self.commit(label, state);
        removed
    }

    /// Replaces some tiles with edited copies (same ids), checked by the rules.
    fn rearrange(
        &mut self,
        label: &str,
        ids: &[PlacedId],
        edit: impl Fn(&Placed) -> Proposal,
        mode: Mode,
        height: Height,
    ) -> Result<(), Refusal> {
        let (set, tiles) = self.selected(ids);
        if tiles.is_empty() {
            return Ok(());
        }
        let proposals: Vec<Proposal> = tiles.iter().map(|t| edit(t)).collect();
        let tile_ids: Vec<PlacedId> = tiles.iter().map(|t| t.id).collect();
        let heights = rules::arrange(self.grid(), &self.state.tiles, &set, &proposals, mode, height)?;
        let mut state = (*self.state).clone();
        for ((id, p), z) in tile_ids.into_iter().zip(proposals).zip(heights) {
            if let Some(t) = state.tiles.iter_mut().find(|t| t.id == id) {
                *t = p.into_placed(id, z);
            }
        }
        self.commit(label, state);
        Ok(())
    }

    /// Moves tiles by whole cells (snap) or any amount (free). Snapped moves
    /// settle again; free moves keep the tiles' heights.
    pub fn move_by(&mut self, ids: &[PlacedId], dx: f32, dy: f32, mode: Mode) -> Result<(), Refusal> {
        let height = if mode == Mode::Snap { Height::Settle } else { Height::Keep };
        self.rearrange("Move", ids, |t| Proposal { x: t.x + dx, y: t.y + dy, ..Proposal::from_placed(t) }, mode, height)
    }

    /// Turns tiles a quarter about a pivot (see [`footprint::group_pivot`]).
    pub fn turn(&mut self, ids: &[PlacedId], pivot: [f32; 2], turn: Turn, mode: Mode) -> Result<(), Refusal> {
        let height = if mode == Mode::Snap { Height::Settle } else { Height::Keep };
        self.rearrange(
            "Turn",
            ids,
            |t| {
                let r = footprint::turn_rect(t.rect(), pivot, turn);
                Proposal { x: r.x, y: r.y, rotation: t.rotation.turned(turn), ..Proposal::from_placed(t) }
            },
            mode,
            height,
        )
    }

    /// Swaps tiles for another tile, in place and with the same turn.
    pub fn replace_with(&mut self, ids: &[PlacedId], key: &TileKey, size: TileSize, mode: Mode) -> Result<(), Refusal> {
        self.rearrange(
            "Replace",
            ids,
            |t| Proposal { key: key.clone(), size, size_known: true, ..Proposal::from_placed(t) },
            mode,
            Height::Settle,
        )
    }

    /// Moves tiles to another level, settled on what is there.
    pub fn move_to_level(&mut self, ids: &[PlacedId], level: i32, mode: Mode) -> Result<(), Refusal> {
        self.rearrange("Move to level", ids, |t| Proposal { level, ..Proposal::from_placed(t) }, mode, Height::Settle)
    }

    /// The levels in use, lowest first.
    pub fn levels(&self) -> Vec<i32> {
        let mut levels: Vec<i32> = self.state.tiles.iter().map(|t| t.level).collect();
        levels.sort_unstable();
        levels.dedup();
        levels
    }

    /// Swaps each listed tile for its own replacement, in place and with the
    /// same turn, as one undo step.
    pub fn replace_each(
        &mut self,
        label: &str,
        changes: &[(PlacedId, TileKey, TileSize)],
        mode: Mode,
    ) -> Result<(), Refusal> {
        let map: std::collections::HashMap<PlacedId, (TileKey, TileSize)> =
            changes.iter().map(|(id, k, s)| (*id, (k.clone(), *s))).collect();
        let ids: Vec<PlacedId> = changes.iter().map(|(id, _, _)| *id).collect();
        self.rearrange(
            label,
            &ids,
            |t| match map.get(&t.id) {
                Some((key, size)) => {
                    Proposal { key: key.clone(), size: *size, size_known: true, ..Proposal::from_placed(t) }
                }
                None => Proposal::from_placed(t),
            },
            mode,
            Height::Settle,
        )
    }

    /// Tiles that wouldn't fit on a resized grid whose old origin moves by
    /// `shift` cells (to grow or shrink on the left or bottom).
    pub fn outside_after_resize(&self, grid: GridSize, shift: [i32; 2]) -> Vec<PlacedId> {
        let (sx, sy) = (shift[0] as f32, shift[1] as f32);
        self.state
            .tiles
            .iter()
            .filter(|t| {
                let (w, h) = t.effective();
                !grid.contains(t.x + sx, t.y + sy, w, h)
            })
            .map(|t| t.id)
            .collect()
    }

    /// Resizes the grid, shifting everything by `shift`; tiles that no longer
    /// fit are removed (one undo step). Returns how many were removed.
    pub fn resize(&mut self, grid: GridSize, shift: [i32; 2]) -> usize {
        let outside: HashSet<PlacedId> = self.outside_after_resize(grid, shift).into_iter().collect();
        let (sx, sy) = (shift[0] as f32, shift[1] as f32);
        let mut state = (*self.state).clone();
        state.grid = grid;
        state.tiles.retain(|t| !outside.contains(&t.id));
        for t in &mut state.tiles {
            t.x += sx;
            t.y += sy;
        }
        if let Some(img) = &mut state.ground_image {
            img.rect[0] += sx;
            img.rect[1] += sy;
        }
        self.commit("Resize grid", state);
        outside.len()
    }

    /// Sets or clears the battle map. Repeated calls with `merge` (a slider
    /// being dragged) make a single undo step.
    pub fn set_ground_image(&mut self, image: Option<GroundImage>, merge: bool) {
        let mut state = (*self.state).clone();
        state.ground_image = image;
        if state == *self.state {
            return;
        }
        self.state = Arc::new(state);
        if merge {
            self.history.push_or_merge("Adjust battle map", self.state.clone());
        } else {
            self.history.push("Battle map", self.state.clone());
        }
    }

    /// Records the real size of a tile's STL once it has loaded, in every
    /// undo step too (it is a fact, not an edit). Returns whether anything
    /// changed.
    pub fn learn_size(&mut self, key: &TileKey, size: TileSize) -> bool {
        let update = |s: &DocState| -> Option<DocState> {
            let stale = |t: &Placed| t.key == *key && (t.size != size || !t.size_known);
            s.tiles.iter().any(stale).then(|| {
                let mut s = s.clone();
                for t in s.tiles.iter_mut().filter(|t| stale(t)) {
                    t.size = size;
                    t.size_known = true;
                }
                s
            })
        };
        let changed = update(&self.state);
        self.history.map_states(update);
        if changed.is_some() {
            self.adopt(self.history.current().clone());
        }
        changed.is_some()
    }

    pub fn undo(&mut self) -> bool {
        match self.history.undo().cloned() {
            Some(s) => {
                self.adopt(s);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.history.redo().cloned() {
            Some(s) => {
                self.adopt(s);
                true
            }
            None => false,
        }
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo_label()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo_label()
    }

    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    pub fn mark_saved(&mut self) {
        self.history.mark_saved();
    }

    /// Names the state now, for a save that runs while editing goes on.
    pub fn save_token(&self) -> SaveToken {
        self.history.token()
    }

    /// Marks the state a finished save started from as saved.
    pub fn mark_saved_at(&mut self, token: SaveToken) {
        self.history.mark_saved_at(token);
    }

    pub fn mark_unsaved(&mut self) {
        self.history.mark_unsaved();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::Rotation;

    fn proposal(x: f32, y: f32, w: u32, h: u32, gz: f32) -> Proposal {
        Proposal {
            key: TileKey::new(&format!("C:/Tiles/t{w}x{h}.stl")),
            size: TileSize { w, h, gz },
            size_known: true,
            x,
            y,
            z: 0.0,
            rotation: Rotation::ZERO,
            level: 0,
        }
    }

    fn doc() -> Document {
        Document::new(DocState::empty(GridSize { cols: 20, rows: 20 }))
    }

    #[test]
    fn a_refused_place_leaves_no_undo_step() {
        let mut d = doc();
        d.place("Place", vec![proposal(0.0, 0.0, 2, 2, 0.5)], Mode::Snap).expect("fits");
        d.mark_saved();
        assert!(d.place("Place", vec![proposal(19.0, 0.0, 2, 2, 0.5)], Mode::Snap).is_err());
        assert!(!d.is_dirty());
        assert_eq!(d.undo_label(), Some("Place"));
    }

    #[test]
    fn undo_restores_ids_and_redo_follows() {
        let mut d = doc();
        let ids = d.place("Place", vec![proposal(0.0, 0.0, 1, 1, 0.5)], Mode::Snap).expect("fits");
        d.remove("Delete", &ids);
        assert!(!d.contains(ids[0]));
        d.undo();
        assert!(d.contains(ids[0]));
        d.redo();
        assert!(!d.contains(ids[0]));
    }

    #[test]
    fn opening_keeps_overlapping_and_off_grid_tiles() {
        let a = proposal(0.0, 0.0, 2, 2, 0.5).into_placed(PlacedId(1), 0.0);
        let b = proposal(1.0, 1.0, 2, 2, 0.5).into_placed(PlacedId(1), 0.0);
        let c = proposal(30.0, 0.0, 2, 2, 0.5).into_placed(PlacedId(5), 0.0);
        let d = Document::new(DocState { tiles: vec![a, b, c], ..DocState::empty(GridSize { cols: 20, rows: 20 }) });
        assert_eq!(d.tiles().len(), 3);
        let ids: HashSet<_> = d.tiles().iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), 3, "duplicate ids are renumbered");
        assert!(!d.is_dirty());
    }

    #[test]
    fn four_turns_bring_a_group_back() {
        let mut d = doc();
        let ids = d
            .place("Place", vec![proposal(4.0, 4.0, 2, 1, 4.0), proposal(6.0, 4.0, 1, 1, 4.0)], Mode::Snap)
            .expect("fits");
        let before = d.state().clone();
        let pivot = footprint::group_pivot(&d.tiles().iter().map(Placed::rect).collect::<Vec<_>>());
        for _ in 0..4 {
            d.turn(&ids, pivot, Turn::CounterClockwise, Mode::Snap).expect("turns");
        }
        assert_eq!(*d.state(), before);
    }

    #[test]
    fn moves_settle_on_the_new_surface() {
        let mut d = doc();
        d.place("Place", vec![proposal(5.0, 0.0, 2, 2, 0.5)], Mode::Snap).expect("floor");
        let pillar = d.place("Place", vec![proposal(0.0, 0.0, 1, 1, 4.0)], Mode::Snap).expect("pillar");
        d.move_by(&pillar, 5.0, 0.0, Mode::Snap).expect("onto the floor");
        assert_eq!(d.tile(pillar[0]).map(|t| t.z), Some(0.5));
    }

    #[test]
    fn tiles_move_between_levels() {
        let mut d = doc();
        let a = d.place("Place", vec![proposal(0.0, 0.0, 2, 2, 0.5)], Mode::Snap).expect("ground");
        let b = d.place("Place", vec![proposal(0.0, 0.0, 2, 2, 0.5)], Mode::Snap).expect("stacked");
        assert_eq!(d.tile(b[0]).map(|t| t.z), Some(0.5));
        d.move_to_level(&b, 1, Mode::Snap).expect("upstairs");
        assert_eq!(d.tile(b[0]).map(|t| (t.level, t.z)), Some((1, 0.0)), "on its own floor");
        assert_eq!(d.levels(), [0, 1]);
        assert!(d.contains(a[0]));
    }

    #[test]
    fn each_tile_gets_its_own_replacement() {
        let mut d = doc();
        let ids = d
            .place("Place", vec![proposal(0.0, 0.0, 2, 2, 0.5), proposal(2.0, 0.0, 2, 2, 0.5)], Mode::Snap)
            .expect("fits");
        let (a, b) = (TileKey::new("C:/T/a.stl"), TileKey::new("C:/T/b.stl"));
        let size = TileSize { w: 2, h: 2, gz: 0.5 };
        d.replace_each("Vary", &[(ids[0], a.clone(), size), (ids[1], b.clone(), size)], Mode::Snap).expect("same size");
        assert_eq!(d.tile(ids[0]).map(|t| t.key.clone()), Some(a));
        assert_eq!(d.tile(ids[1]).map(|t| t.key.clone()), Some(b));
        d.undo();
        assert_ne!(d.tile(ids[0]).map(|t| t.key.clone()), Some(TileKey::new("C:/T/a.stl")));
    }

    #[test]
    fn resizing_removes_what_falls_off_in_one_step() {
        let mut d = doc();
        d.place("Place", vec![proposal(0.0, 0.0, 1, 1, 0.5), proposal(15.0, 0.0, 1, 1, 0.5)], Mode::Snap)
            .expect("fits");
        let grid = GridSize { cols: 10, rows: 10 };
        assert_eq!(d.outside_after_resize(grid, [0, 0]).len(), 1);
        assert_eq!(d.resize(grid, [0, 0]), 1);
        d.undo();
        assert_eq!(d.tiles().len(), 2);
    }

    #[test]
    fn learnt_sizes_survive_undo() {
        let key = TileKey::new("C:/Tiles/t1x1.stl");
        let mut d = doc();
        let mut p = proposal(0.0, 0.0, 1, 1, 0.5);
        p.size_known = false;
        d.place("Place", vec![p], Mode::Free).expect("fits");
        assert!(d.learn_size(&key, TileSize { w: 2, h: 2, gz: 0.48 }));
        d.undo();
        d.redo();
        assert_eq!(d.tiles()[0].size, TileSize { w: 2, h: 2, gz: 0.48 });
        assert!(d.tiles()[0].size_known);
    }

    #[test]
    fn dragging_a_slider_is_one_step() {
        let mut d = doc();
        for w in 1..10 {
            d.set_ground_image(Some(GroundImage { path: "map.png".into(), rect: [0.0, 0.0, w as f32, 1.0] }), true);
        }
        d.undo();
        assert!(d.ground_image().is_none());
    }
}
