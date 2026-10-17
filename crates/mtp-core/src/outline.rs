//! Walls round an area: where straight wall pieces and corner pieces go
//! along every outside edge of a set of cells, facing into the area.

use std::collections::{BTreeMap, HashSet};

use crate::footprint::Rotation;

/// Which way a wall tile is drawn: long along x (`w` = length, `h` = 1) or
/// along y.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallShape {
    AlongX,
    AlongY,
}

/// A wall or corner piece: its origin in cells and its turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub x: f32,
    pub y: f32,
    pub rotation: Rotation,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WallPlan {
    pub walls: Vec<Piece>,
    pub corners: Vec<Piece>,
    /// Edge stretches too short for a wall piece, left open.
    pub gaps: usize,
}

/// Plans walls of `length` cells round `cells`, on the outermost cells.
/// Walls face into the area (a tile's front faces +y at rotation 0). With
/// `corners`, the cells where two edges meet are left for corner pieces;
/// without, the walls along x run through them.
pub fn walls_around(cells: &HashSet<(i32, i32)>, length: u32, shape: WallShape, corners: bool) -> WallPlan {
    let has = |x: i32, y: i32| cells.contains(&(x, y));
    let mut plan = WallPlan::default();
    let length = length.max(1) as i32;
    // Rotations that make a wall facing into the area run along the edge
    let turn = |facing: u8| {
        let base = match shape {
            WallShape::AlongX => 0,
            WallShape::AlongY => 1,
        };
        Rotation::from_quarters(i64::from(facing) + base)
    };
    let vertical_edge = |x: i32, y: i32| !has(x - 1, y) || !has(x + 1, y);
    let horizontal_edge = |x: i32, y: i32| !has(x, y - 1) || !has(x, y + 1);
    let is_corner = |x: i32, y: i32| vertical_edge(x, y) && horizontal_edge(x, y);

    // (edge, fixed coordinate) -> the free coordinates along it
    let mut runs: BTreeMap<(u8, i32), Vec<i32>> = BTreeMap::new();
    for &(x, y) in cells {
        // Edges along x: north (facing south, quarter 2) and south (facing
        // north, 0). A cell one row thick holds one wall, on the north side.
        let north = !has(x, y + 1);
        for (edge, outside, facing) in [(0u8, north, 2u8), (1, !has(x, y - 1) && !north, 0)] {
            if outside && !(corners && is_corner(x, y)) {
                runs.entry((edge * 4 + facing, y)).or_default().push(x);
            }
        }
        // Edges along y: east (facing west, quarter 1) and west (facing east,
        // 3). Cells on an x edge are already walled there; a cell one column
        // thick holds one wall, on the east side.
        let east = !has(x + 1, y);
        for (edge, outside, facing) in [(2u8, east, 1u8), (3, !has(x - 1, y) && !east, 3)] {
            if outside && !horizontal_edge(x, y) {
                runs.entry((edge * 4 + facing, x)).or_default().push(y);
            }
        }
        if corners && is_corner(x, y) {
            // A corner piece as drawn closes a south-west corner; it turns a
            // quarter for each corner round the room
            let quarters = match (!has(x - 1, y), !has(x, y - 1)) {
                (true, true) => 0,
                (false, true) => 1,
                (false, false) => 2,
                (true, false) => 3,
            };
            plan.corners.push(Piece { x: x as f32, y: y as f32, rotation: Rotation::from_quarters(quarters) });
        }
    }

    for ((code, fixed), mut along) in runs {
        let (edge, facing) = (code / 4, code % 4);
        along.sort_unstable();
        along.dedup();
        // Split into unbroken stretches, then fill each with whole pieces
        let mut start = 0;
        for i in 1..=along.len() {
            if i == along.len() || along[i] != along[i - 1] + 1 {
                let (from, len) = (along[start], (i - start) as i32);
                for k in 0..len / length {
                    let at = (from + k * length) as f32;
                    let (x, y) = if edge < 2 { (at, fixed as f32) } else { (fixed as f32, at) };
                    plan.walls.push(Piece { x, y, rotation: turn(facing) });
                }
                if len % length != 0 {
                    plan.gaps += 1;
                }
                start = i;
            }
        }
    }
    plan.corners.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap_or(std::cmp::Ordering::Equal));
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(w: i32, h: i32) -> HashSet<(i32, i32)> {
        (0..w).flat_map(|x| (0..h).map(move |y| (x, y))).collect()
    }

    #[test]
    fn a_room_gets_four_walls_facing_in_and_four_corners() {
        // 6 x 4 room, 2-long walls, corners: x edges have 4 free cells
        // (2 walls each), y edges 2 free cells (1 wall each)
        let plan = walls_around(&room(6, 4), 2, WallShape::AlongX, true);
        assert_eq!(plan.corners.len(), 4);
        assert_eq!(plan.walls.len(), 2 + 2 + 1 + 1);
        assert_eq!(plan.gaps, 0);
        let south: Vec<_> = plan.walls.iter().filter(|p| p.y == 0.0).collect();
        assert!(south.iter().all(|p| p.rotation.degrees() == 0), "south wall faces north, into the room");
        let north: Vec<_> = plan.walls.iter().filter(|p| p.y == 3.0).collect();
        assert!(north.iter().all(|p| p.rotation.degrees() == 180));
        let east: Vec<_> = plan.walls.iter().filter(|p| p.x == 5.0).collect();
        assert_eq!(east.len(), 1);
        assert_eq!(east[0].rotation.degrees(), 90, "east wall faces west");
        assert_eq!(east[0].rotation.forward(), [-1.0, 0.0]);
    }

    #[test]
    fn without_corners_the_x_walls_run_through() {
        let plan = walls_around(&room(4, 4), 2, WallShape::AlongX, false);
        assert!(plan.corners.is_empty());
        // x edges: 4 cells = 2 walls each; y edges: the 2 inner cells = 1 wall each
        assert_eq!(plan.walls.len(), 6);
    }

    #[test]
    fn short_stretches_are_gaps_and_walls_drawn_along_y_turn_a_quarter_more() {
        let plan = walls_around(&room(5, 3), 2, WallShape::AlongY, true);
        // x edges have 3 free cells (a wall and a gap each), y edges one (a gap each)
        assert_eq!(plan.gaps, 4);
        let south: Vec<_> = plan.walls.iter().filter(|p| p.y == 0.0).collect();
        assert!(south.iter().all(|p| p.rotation.degrees() == 90));
    }

    #[test]
    fn a_corridor_one_cell_wide_gets_one_wall_per_cell() {
        let plan = walls_around(&room(5, 1), 1, WallShape::AlongX, false);
        let mut seen = HashSet::new();
        for p in &plan.walls {
            assert!(seen.insert((p.x as i32, p.y as i32)), "two walls on one cell");
        }
        assert_eq!(plan.walls.len(), 5);
    }

    #[test]
    fn corners_turn_to_face_the_room() {
        let plan = walls_around(&room(4, 4), 2, WallShape::AlongX, true);
        let at = |x: f32, y: f32| plan.corners.iter().find(|p| (p.x, p.y) == (x, y)).map(|p| p.rotation.degrees());
        assert_eq!((at(0.0, 0.0), at(3.0, 0.0), at(3.0, 3.0), at(0.0, 3.0)), (Some(0), Some(90), Some(180), Some(270)));
    }

    #[test]
    fn an_l_shaped_area_follows_its_edges() {
        let mut cells = room(4, 2);
        cells.extend(room(2, 4));
        let plan = walls_around(&cells, 1, WallShape::AlongX, true);
        // Every outer cell gets a wall or a corner, none twice
        let mut seen = HashSet::new();
        for p in plan.walls.iter().chain(&plan.corners) {
            assert!(seen.insert((p.x as i32, p.y as i32)), "two pieces on one cell");
        }
        assert_eq!(plan.gaps, 0);
    }
}
