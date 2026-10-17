//! The one placement rule. Placing, moving, pasting, turning and filling all
//! go through [`arrange`], so a tile can't end up somewhere one of them
//! allows and another doesn't. In the Python version moves, pastes and turns
//! skipped every check.

use std::collections::HashSet;
use std::fmt;

use crate::footprint::Rotation;
use crate::grid::{CellIndex, GridSize};
use crate::tile::{Placed, PlacedId, TileKey};
use crate::units::TileSize;

/// How strictly tiles are placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Whole cells, no overlaps, level support under the whole footprint.
    #[default]
    Snap,
    /// Anywhere on the grid (held Ctrl): only the grid edge is checked.
    Free,
}

/// Where a tile's base ends up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Height {
    /// On top of whatever is under it.
    Settle,
    /// Where it was (a free-mode move keeps hand-placed heights).
    Keep,
}

/// A tile as an edit wants it, before its height is worked out.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub key: TileKey,
    pub size: TileSize,
    pub size_known: bool,
    pub x: f32,
    pub y: f32,
    /// The base height it had, for [`Height::Keep`] and for settling stacked
    /// groups bottom first.
    pub z: f32,
    pub rotation: Rotation,
    pub level: i32,
}

impl Proposal {
    pub fn from_placed(t: &Placed) -> Self {
        Self {
            key: t.key.clone(),
            size: t.size,
            size_known: t.size_known,
            x: t.x,
            y: t.y,
            z: t.z,
            rotation: t.rotation,
            level: t.level,
        }
    }

    pub fn into_placed(self, id: PlacedId, z: f32) -> Placed {
        Placed {
            id,
            key: self.key,
            size: self.size,
            size_known: self.size_known,
            x: self.x,
            y: self.y,
            z,
            rotation: self.rotation,
            level: self.level,
        }
    }
}

/// Why an edit was refused, in words for the status bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Refusal {
    OutsideGrid,
    Overlaps,
    Uneven { low: f32, high: f32 },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideGrid => f.write_str("Part of it would be outside the grid."),
            Self::Overlaps => f.write_str("It would overlap another tile."),
            Self::Uneven { low, high } => write!(
                f,
                "The surface under it isn't level ({:.1} mm higher on one side). Hold Ctrl to place it anyway.",
                (high - low) * crate::units::CELL_MM as f32
            ),
        }
    }
}

/// Works out the base height of every proposed tile, against the document's
/// other tiles and each other, or refuses the whole edit.
///
/// `replacing` are tiles the edit removes or moves: they are left out of the
/// checks. Proposals settle bottom first, so a wall moved together with the
/// floor under it lands on that floor. Returns heights in proposal order.
pub fn arrange(
    grid: GridSize,
    tiles: &[Placed],
    replacing: &HashSet<PlacedId>,
    proposals: &[Proposal],
    mode: Mode,
    height: Height,
) -> Result<Vec<f32>, Refusal> {
    let mut working: Vec<Placed> = tiles.iter().filter(|t| !replacing.contains(&t.id)).cloned().collect();
    let mut index = CellIndex::build(&working);
    let mut order: Vec<usize> = (0..proposals.len()).collect();
    order.sort_by(|&a, &b| proposals[a].z.total_cmp(&proposals[b].z));

    let mut heights = vec![0.0; proposals.len()];
    for i in order {
        let p = &proposals[i];
        let probe = p.clone().into_placed(PlacedId(u64::MAX), p.z);
        let (across, deep) = probe.effective();
        if !grid.contains(p.x, p.y, across, deep) {
            return Err(Refusal::OutsideGrid);
        }
        let z = match height {
            Height::Keep => p.z,
            Height::Settle => {
                let support = index.support(&working, probe.cells(), |_| false);
                if mode == Mode::Snap && !support.is_level() {
                    return Err(Refusal::Uneven { low: support.low, high: support.high });
                }
                support.high
            }
        };
        if mode == Mode::Snap && index.overlapping(&working, probe.cells(), z, z + p.size.gz, |_| false).is_some() {
            return Err(Refusal::Overlaps);
        }
        let placed = p.clone().into_placed(PlacedId(u64::MAX - i as u64), z);
        index.insert(working.len(), &placed);
        working.push(placed);
        heights[i] = z;
    }
    Ok(heights)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: GridSize = GridSize { cols: 20, rows: 20 };

    fn proposal(x: f32, y: f32, w: u32, h: u32, gz: f32) -> Proposal {
        Proposal {
            key: TileKey::new("t.stl"),
            size: TileSize { w, h, gz },
            size_known: true,
            x,
            y,
            z: 0.0,
            rotation: Rotation::ZERO,
            level: 0,
        }
    }

    fn placed(id: u64, p: Proposal, z: f32) -> Placed {
        p.into_placed(PlacedId(id), z)
    }

    fn none() -> HashSet<PlacedId> {
        HashSet::new()
    }

    #[test]
    fn stacks_on_level_support() {
        let floor = placed(1, proposal(0.0, 0.0, 2, 2, 0.5), 0.0);
        let wall = proposal(0.0, 0.0, 2, 1, 4.0);
        assert_eq!(arrange(GRID, &[floor], &none(), &[wall], Mode::Snap, Height::Settle), Ok(vec![0.5]));
    }

    #[test]
    fn refuses_uneven_support_unless_free() {
        let floor = placed(1, proposal(0.0, 0.0, 2, 2, 0.5), 0.0);
        let half_over = proposal(1.0, 0.0, 2, 2, 0.5);
        let tiles = [floor];
        assert!(matches!(
            arrange(GRID, &tiles, &none(), std::slice::from_ref(&half_over), Mode::Snap, Height::Settle),
            Err(Refusal::Uneven { .. })
        ));
        assert_eq!(arrange(GRID, &tiles, &none(), &[half_over], Mode::Free, Height::Settle), Ok(vec![0.5]));
    }

    #[test]
    fn refuses_outside_the_grid_in_both_modes() {
        let p = proposal(19.0, 0.0, 2, 1, 1.0);
        for mode in [Mode::Snap, Mode::Free] {
            assert_eq!(
                arrange(GRID, &[], &none(), std::slice::from_ref(&p), mode, Height::Settle),
                Err(Refusal::OutsideGrid)
            );
        }
    }

    #[test]
    fn a_moved_group_keeps_its_stack() {
        let floor = placed(1, proposal(0.0, 0.0, 2, 2, 0.5), 0.0);
        let wall = placed(2, proposal(0.0, 0.0, 2, 1, 4.0), 0.5);
        let moving: HashSet<_> = [floor.id, wall.id].into();
        let shifted = [&wall, &floor].map(|t| Proposal { x: t.x + 5.0, ..Proposal::from_placed(t) });
        let z = arrange(GRID, &[floor.clone(), wall.clone()], &moving, &shifted, Mode::Snap, Height::Settle);
        assert_eq!(z, Ok(vec![0.5, 0.0]), "wall settles on the moved floor");
    }

    #[test]
    fn refuses_overlaps_with_tiles_hanging_above() {
        // A bridge at 3..3.5 over cell (0,0): a 4-high pillar can't go under it
        let bridge = placed(1, proposal(0.0, 0.0, 1, 1, 0.5), 3.0);
        let pillar = proposal(0.0, 0.0, 1, 1, 4.0);
        let tiles = [bridge];
        // Support is the bridge's top, so the pillar would sit on the bridge instead
        assert_eq!(
            arrange(GRID, &tiles, &none(), std::slice::from_ref(&pillar), Mode::Snap, Height::Settle),
            Ok(vec![3.5])
        );
        // Keeping its height on the ground runs into the bridge
        assert_eq!(arrange(GRID, &tiles, &none(), &[pillar], Mode::Snap, Height::Keep), Err(Refusal::Overlaps));
    }

    #[test]
    fn proposals_collide_with_each_other() {
        let a = proposal(0.0, 0.0, 2, 2, 0.5);
        let b = proposal(1.0, 1.0, 2, 2, 0.5);
        assert!(arrange(GRID, &[], &none(), &[a, b], Mode::Snap, Height::Keep).is_err());
    }
}
