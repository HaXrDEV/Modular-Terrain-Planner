//! Rotation and footprints: the one place that knows how a turned tile sits on
//! the grid. Placement, moving, pasting, the renderer and the build plan all
//! use these functions, so they can't disagree.

use crate::units::TileSize;

/// Quarter turns counter-clockwise, seen from above. Stored in files as
/// degrees (0, 90, 180, 270).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rotation(u8);

/// Direction of a turn, seen from above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    CounterClockwise,
    Clockwise,
}

impl Rotation {
    pub const ZERO: Self = Self(0);

    pub fn from_quarters(quarters: i64) -> Self {
        Self(quarters.rem_euclid(4) as u8)
    }

    /// Nearest quarter turn to any angle, so `-90` is 270 and `450` is 90.
    /// Returns `None` for NaN and infinities.
    pub fn from_degrees(degrees: f64) -> Option<Self> {
        degrees.is_finite().then(|| Self::from_quarters((degrees / 90.0).round() as i64))
    }

    pub fn quarters(self) -> u8 {
        self.0
    }

    pub fn degrees(self) -> u16 {
        u16::from(self.0) * 90
    }

    pub fn radians(self) -> f32 {
        f32::from(self.degrees()).to_radians()
    }

    pub fn turned(self, turn: Turn) -> Self {
        match turn {
            Turn::CounterClockwise => Self::from_quarters(i64::from(self.0) + 1),
            Turn::Clockwise => Self::from_quarters(i64::from(self.0) + 3),
        }
    }

    /// Whether width and depth swap (90° and 270°).
    pub fn swaps_sides(self) -> bool {
        self.0 % 2 == 1
    }

    /// Exact cosine and sine for the quarter turn.
    fn cos_sin(self) -> (f32, f32) {
        match self.0 {
            0 => (1.0, 0.0),
            1 => (0.0, 1.0),
            2 => (-1.0, 0.0),
            _ => (0.0, -1.0),
        }
    }

    /// The direction a tile's front (+Y at rotation 0) faces after turning.
    pub fn forward(self) -> [f32; 2] {
        let (c, s) = self.cos_sin();
        [-s, c]
    }
}

impl serde::Serialize for Rotation {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u16(self.degrees())
    }
}

/// Footprint in cells after rotation: `(across, deep)`.
pub fn effective_size(size: TileSize, rotation: Rotation) -> (u32, u32) {
    if rotation.swaps_sides() { (size.h, size.w) } else { (size.w, size.h) }
}

/// The cells a tile covers, from its floored origin, row by row.
pub fn cells(x: f32, y: f32, across: u32, deep: u32) -> impl Iterator<Item = (i32, i32)> {
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    (0..deep as i32).flat_map(move |dy| (0..across as i32).map(move |dx| (x0 + dx, y0 + dy)))
}

/// Model matrix, column-major, mapping the unit cube of a normalised mesh to
/// the world: turn inside the unit square, scale to the turned footprint,
/// move to the grid position. Identical to the Python version's.
pub fn model_matrix(x: f32, y: f32, z: f32, size: TileSize, rotation: Rotation) -> [f32; 16] {
    let (ew, eh) = effective_size(size, rotation);
    let (ew, eh) = (ew as f32, eh as f32);
    let (c, s) = rotation.cos_sin();
    // Translation that keeps the unit square in place while turning about its centre
    let tx = 0.5 - (c * 0.5 - s * 0.5);
    let ty = 0.5 - (s * 0.5 + c * 0.5);
    [ew * c, eh * s, 0.0, 0.0, -ew * s, eh * c, 0.0, 0.0, 0.0, 0.0, size.gz, 0.0, x + ew * tx, y + eh * ty, z, 1.0]
}

/// A tile's footprint rectangle in cells, for turning a group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The fixed pivot for turning a group: the floored centre of its bounds.
///
/// Turning about an integer point keeps whole-cell positions whole, and
/// keeping the same pivot across presses makes four turns the identity.
pub fn group_pivot(rects: &[Rect]) -> [f32; 2] {
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for r in rects {
        lo = [lo[0].min(r.x), lo[1].min(r.y)];
        hi = [hi[0].max(r.x + r.w), hi[1].max(r.y + r.h)];
    }
    if rects.is_empty() {
        return [0.0, 0.0];
    }
    [((lo[0] + hi[0]) / 2.0).floor(), ((lo[1] + hi[1]) / 2.0).floor()]
}

/// Where a footprint lands after a quarter turn about `pivot`. The returned
/// rectangle has width and depth swapped.
pub fn turn_rect(r: Rect, pivot: [f32; 2], turn: Turn) -> Rect {
    let [px, py] = pivot;
    match turn {
        Turn::CounterClockwise => Rect { x: px - (r.y + r.h - py), y: py + (r.x - px), w: r.h, h: r.w },
        Turn::Clockwise => Rect { x: px + (r.y - py), y: py - (r.x + r.w - px), w: r.h, h: r.w },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_snaps_any_angle() {
        let deg = |d: f64| Rotation::from_degrees(d).map(Rotation::degrees);
        assert_eq!(deg(-90.0), Some(270));
        assert_eq!(deg(450.0), Some(90));
        assert_eq!(deg(80.0), Some(90));
        assert_eq!(deg(f64::NAN), None);
    }

    #[test]
    fn four_turns_are_the_identity() {
        let rects = [Rect { x: 3.0, y: 1.0, w: 2.0, h: 1.0 }, Rect { x: 5.0, y: 1.0, w: 1.0, h: 4.0 }];
        let pivot = group_pivot(&rects);
        for turn in [Turn::CounterClockwise, Turn::Clockwise] {
            let mut turned = rects;
            for _ in 0..4 {
                turned = turned.map(|r| turn_rect(r, pivot, turn));
                for r in &turned {
                    assert_eq!(r.x.fract(), 0.0, "whole cells stay whole");
                    assert_eq!(r.y.fract(), 0.0);
                }
            }
            assert_eq!(turned, rects);
        }
    }

    #[test]
    fn opposite_turns_cancel() {
        let r = Rect { x: 7.0, y: -2.0, w: 3.0, h: 2.0 };
        let p = [4.0, 4.0];
        assert_eq!(turn_rect(turn_rect(r, p, Turn::CounterClockwise), p, Turn::Clockwise), r);
    }

    #[test]
    fn forward_turns_counter_clockwise() {
        assert_eq!(Rotation::ZERO.forward(), [0.0, 1.0]);
        assert_eq!(Rotation::from_quarters(1).forward(), [-1.0, 0.0]);
        assert_eq!(Rotation::from_quarters(3).forward(), [1.0, 0.0]);
    }
}
