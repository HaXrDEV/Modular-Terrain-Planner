//! A cell index over placed tiles, for collisions and stacking.
//!
//! Tiles collide when their footprints share a cell and their height ranges
//! overlap. The Python version only compared exact base heights, so a floor
//! inside a wall, or two tiles at slightly different heights, went through.

use std::collections::HashMap;

use crate::tile::Placed;

/// Heights closer than this count as equal (a hundredth of a millimetre is
/// far below anything printable).
pub const HEIGHT_EPSILON: f32 = 1e-3;

/// Size of the grid in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u32,
    pub rows: u32,
}

impl GridSize {
    pub const DEFAULT: Self = Self { cols: 80, rows: 80 };
    /// Large enough for a whole convention table, small enough to stay fast.
    pub const MAX: u32 = 2000;

    /// Whether every cell of a footprint lies on the grid.
    pub fn contains(self, x: f32, y: f32, across: u32, deep: u32) -> bool {
        let (x0, y0) = (x.floor(), y.floor());
        x0 >= 0.0 && y0 >= 0.0 && x0 + across as f32 <= self.cols as f32 && y0 + deep as f32 <= self.rows as f32
    }
}

/// The highest surface under a footprint: equal `low` and `high` mean level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Support {
    pub low: f32,
    pub high: f32,
}

impl Support {
    pub fn is_level(self) -> bool {
        self.high - self.low <= HEIGHT_EPSILON
    }
}

/// Which tiles cover each cell. Holds indices into the slice it was built from.
#[derive(Clone, Debug, Default)]
pub struct CellIndex {
    cells: HashMap<(i32, i32), Vec<usize>>,
}

impl CellIndex {
    pub fn build(tiles: &[Placed]) -> Self {
        let mut index = Self::default();
        for (i, t) in tiles.iter().enumerate() {
            index.insert(i, t);
        }
        index
    }

    pub fn insert(&mut self, i: usize, tile: &Placed) {
        for cell in tile.cells() {
            self.cells.entry(cell).or_default().push(i);
        }
    }

    /// Indices of tiles covering a cell.
    pub fn at(&self, cell: (i32, i32)) -> &[usize] {
        self.cells.get(&cell).map_or(&[], Vec::as_slice)
    }

    /// Tops of the surfaces under each cell of a footprint (0 where empty),
    /// leaving out tiles `skip` says to.
    pub fn support(
        &self,
        tiles: &[Placed],
        cells: impl Iterator<Item = (i32, i32)>,
        skip: impl Fn(usize) -> bool,
    ) -> Support {
        let mut support = Support { low: f32::INFINITY, high: f32::NEG_INFINITY };
        for cell in cells {
            let top = self.at(cell).iter().filter(|&&i| !skip(i)).map(|&i| tiles[i].top()).fold(0.0, f32::max);
            support.low = support.low.min(top);
            support.high = support.high.max(top);
        }
        if support.low.is_infinite() { Support { low: 0.0, high: 0.0 } } else { support }
    }

    /// The first tile whose box overlaps `[z, top)` over any of the cells.
    pub fn overlapping(
        &self,
        tiles: &[Placed],
        cells: impl Iterator<Item = (i32, i32)>,
        z: f32,
        top: f32,
        skip: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        for cell in cells {
            for &i in self.at(cell) {
                if skip(i) {
                    continue;
                }
                let t = &tiles[i];
                if z < t.top() - HEIGHT_EPSILON && t.z < top - HEIGHT_EPSILON {
                    return Some(i);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::Rotation;
    use crate::tile::{PlacedId, TileKey};
    use crate::units::TileSize;

    fn tile(id: u64, x: f32, y: f32, z: f32, w: u32, h: u32, gz: f32) -> Placed {
        Placed {
            id: PlacedId(id),
            key: TileKey::new("t.stl"),
            size: TileSize { w, h, gz },
            size_known: true,
            x,
            y,
            z,
            rotation: Rotation::ZERO,
            level: 0,
        }
    }

    #[test]
    fn height_ranges_collide() {
        let tiles = [tile(1, 0.0, 0.0, 0.0, 2, 1, 4.0)];
        let index = CellIndex::build(&tiles);
        let cells = || [(0, 0)].into_iter();
        // A thin floor at 0.2 inside a 4-high wall: the Python version let it through
        assert_eq!(index.overlapping(&tiles, cells(), 0.2, 0.7, |_| false), Some(0));
        // On top of the wall: fine
        assert_eq!(index.overlapping(&tiles, cells(), 4.0, 4.5, |_| false), None);
        // Skipped tiles don't count
        assert_eq!(index.overlapping(&tiles, cells(), 0.2, 0.7, |i| i == 0), None);
    }

    #[test]
    fn support_spans_the_whole_footprint() {
        let tiles = [tile(1, 0.0, 0.0, 0.0, 1, 1, 0.5)];
        let index = CellIndex::build(&tiles);
        let s = index.support(&tiles, [(0, 0), (1, 0)].into_iter(), |_| false);
        assert_eq!(s, Support { low: 0.0, high: 0.5 });
        assert!(!s.is_level());
        assert!(index.support(&tiles, [(5, 5)].into_iter(), |_| false).is_level());
    }

    #[test]
    fn bounds_use_floored_origins() {
        let grid = GridSize { cols: 10, rows: 10 };
        assert!(grid.contains(8.0, 0.0, 2, 1));
        assert!(!grid.contains(9.0, 0.0, 2, 1));
        assert!(!grid.contains(-0.5, 0.0, 1, 1));
        assert!(grid.contains(8.9, 0.0, 1, 1));
    }
}
