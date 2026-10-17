//! The printed map as a flat model, drawn by both the PNG and the PDF export
//! so the two can't disagree.
//!
//! Coordinates are grid cells with y up, as in the editor. The map is printed
//! with north at the top, so a point's distance down the page is
//! `bounds.max_y - y`.

use std::collections::HashMap;

use mtp_core::tile::{PlacedId, TileKey};
use mtp_core::units::CELL_MM;

use crate::ExportInput;
use crate::placements::placements;
use crate::print_list::{PrintRow, print_list};

/// A range of whole cells: `min` inclusive, `max` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellBounds {
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
}

impl CellBounds {
    pub fn width(&self) -> i32 {
        self.max_x - self.min_x
    }

    pub fn height(&self) -> i32 {
        self.max_y - self.min_y
    }
}

/// One tile as the map shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct MapTile {
    pub id: PlacedId,
    pub key: TileKey,
    /// Its number in the placements table.
    pub label: u32,
    pub name: String,
    pub display_name: String,
    /// Footprint `[x, y, w, h]` in cells, y up.
    pub rect: [f32; 4],
    pub color: [u8; 3],
    /// The direction the tile's front faces, in grid space (y up). The same
    /// vector the 3D view uses, so the map can't show it mirrored.
    pub arrow: [f32; 2],
    /// Base height above the floor, such as `+6 mm`, when it isn't zero.
    pub height_label: Option<String>,
    /// The STL wasn't found, so the size is a guess.
    pub placeholder: bool,
    pub level: i32,
    pub z: f32,
}

impl MapTile {
    /// The arrow in page directions, x right and y down.
    pub fn page_arrow(&self) -> [f32; 2] {
        [self.arrow[0], -self.arrow[1]]
    }
}

/// One level of the map (or all of them), ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct MapLayout {
    /// `None` when every level is drawn together.
    pub level: Option<i32>,
    pub bounds: CellBounds,
    /// In drawing order: lower tiles first, so a stack shows its top.
    pub tiles: Vec<MapTile>,
    /// The files on this map, as the print list shows them.
    pub legend: Vec<PrintRow>,
}

impl MapLayout {
    /// A grid point as a page offset from the map's top-left corner, in cells.
    pub fn to_page(&self, x: f32, y: f32) -> [f32; 2] {
        [x - self.bounds.min_x as f32, self.bounds.max_y as f32 - y]
    }
}

/// Lays out `level` (or every level, for `None`). Labels and display names
/// come from the whole export, so a tile has the same number on every page.
/// Returns `None` when there's nothing to draw.
pub fn map_layout(input: &ExportInput, level: Option<i32>) -> Option<MapLayout> {
    let labels: HashMap<PlacedId, u32> = placements(input).into_iter().collect();
    let rows = print_list(input);
    let rows_by_key: HashMap<&TileKey, &PrintRow> = rows.iter().map(|r| (&r.key, r)).collect();

    let mut tiles: Vec<MapTile> = input
        .tiles()
        .filter(|t| level.is_none_or(|l| t.level == l))
        .map(|t| {
            let (w, h) = t.effective();
            let row = rows_by_key.get(&t.key);
            let meta = input.info.meta(&t.key);
            MapTile {
                id: t.id,
                key: t.key.clone(),
                label: labels.get(&t.id).copied().unwrap_or(0),
                display_name: row.map_or_else(|| meta.name.clone(), |r| r.display_name.clone()),
                name: meta.name,
                rect: [t.x, t.y, w as f32, h as f32],
                color: meta.color,
                arrow: t.rotation.forward(),
                height_label: height_label(t.z),
                placeholder: !t.size_known,
                level: t.level,
                z: t.z,
            }
        })
        .collect();
    if tiles.is_empty() {
        return None;
    }
    tiles.sort_by(|a, b| a.level.cmp(&b.level).then(a.z.total_cmp(&b.z)).then(a.label.cmp(&b.label)));

    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for t in &tiles {
        let [x, y, w, h] = t.rect;
        lo = [lo[0].min(x), lo[1].min(y)];
        hi = [hi[0].max(x + w), hi[1].max(y + h)];
    }
    let grid = input.state.grid;
    // One cell of margin where the grid allows it; tiles left off the grid
    // by an old file still fit
    let (min_x, min_y) = (lo[0].floor() as i32, lo[1].floor() as i32);
    let (max_x, max_y) = (hi[0].ceil() as i32, hi[1].ceil() as i32);
    let bounds = CellBounds {
        min_x: (min_x - 1).max(0).min(min_x),
        min_y: (min_y - 1).max(0).min(min_y),
        max_x: (max_x + 1).min(grid.cols as i32).max(max_x),
        max_y: (max_y + 1).min(grid.rows as i32).max(max_y),
    };

    let mut legend: Vec<PrintRow> = Vec::new();
    for r in &rows {
        if tiles.iter().any(|t| t.key == r.key) {
            let count = tiles.iter().filter(|t| t.key == r.key).count() as u32;
            legend.push(PrintRow { count, ..r.clone() });
        }
    }
    Some(MapLayout { level, bounds, tiles, legend })
}

/// A base height in millimetres, which is what a builder measures, not cells.
fn height_label(z: f32) -> Option<String> {
    let mm = f64::from(z) * CELL_MM;
    if mm < 0.05 {
        return None;
    }
    Some(format!("+{} mm", format_mm(mm)))
}

/// Millimetres with one decimal, dropped when it's zero.
pub(crate) fn format_mm(mm: f64) -> String {
    let s = format!("{mm:.1}");
    s.strip_suffix(".0").map_or(s.clone(), str::to_string)
}

#[cfg(test)]
mod tests {
    use mtp_core::footprint::Rotation;

    use super::*;
    use crate::DefaultInfo;
    use crate::testutil::{state, tile};

    fn layout_of(tiles: Vec<mtp_core::tile::Placed>) -> MapLayout {
        let s = state(40, 30, tiles);
        let input = ExportInput { title: "Test".into(), state: &s, info: &DefaultInfo, level: None };
        map_layout(&input, None).expect("tiles to draw")
    }

    #[test]
    fn arrows_match_the_3d_view() {
        let turned = |q| {
            let mut t = tile(1, "a/b.stl", 2, 1, 4.0, 4.0);
            t.rotation = Rotation::from_quarters(q);
            layout_of(vec![t]).tiles[0].page_arrow()
        };
        // Page y grows downwards, so up the page is -y
        assert_eq!(turned(0), [0.0, -1.0], "front faces north, up the page");
        assert_eq!(turned(1), [-1.0, 0.0], "a quarter turn counter-clockwise faces left");
        assert_eq!(turned(3), [1.0, 0.0], "three quarter turns face right");
    }

    #[test]
    fn bounds_are_padded_and_clamped() {
        let l = layout_of(vec![tile(1, "a/b.stl", 2, 2, 0.0, 5.5), tile(2, "a/b.stl", 2, 2, 38.0, 10.0)]);
        assert_eq!(l.bounds, CellBounds { min_x: 0, min_y: 4, max_x: 40, max_y: 13 });
        assert_eq!(l.to_page(0.0, 13.0), [0.0, 0.0], "north-west corner is the page's top-left");
    }

    #[test]
    fn heights_are_in_millimetres() {
        let mut raised = tile(1, "a/b.stl", 1, 1, 0.0, 0.0);
        raised.z = 0.48;
        let mut placeholder = tile(2, "a/c.stl", 1, 1, 3.0, 0.0);
        placeholder.size_known = false;
        let l = layout_of(vec![raised, placeholder]);
        let by_id = |id| l.tiles.iter().find(|t| t.id == PlacedId(id)).expect("tile");
        assert_eq!(by_id(1).height_label.as_deref(), Some("+6 mm"));
        assert_eq!(by_id(2).height_label, None);
        assert!(by_id(2).placeholder && !by_id(1).placeholder);
        assert_eq!(l.legend.len(), 2);
    }
}
