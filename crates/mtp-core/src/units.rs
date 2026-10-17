//! Grid units and how an STL's size turns into a tile footprint.
//!
//! One grid cell is 12.5 mm, so a 1" battle-map square is 2 × 2 cells.

/// Size of one grid cell in millimetres.
pub const CELL_MM: f64 = 12.5;
/// Millimetres per inch.
pub const INCH_MM: f64 = 25.4;
/// Cells per 1" square, in each direction.
pub const CELLS_PER_INCH_SQUARE: u32 = 2;
/// A file whose larger horizontal side is under this many units is taken to
/// be in inches (OpenForge and Printable Scenery export in inches).
pub const INCH_HEURISTIC_LIMIT: f64 = 5.0;
/// Lowest tile height in cells, so flat plates stay visible and pickable.
pub const MIN_HEIGHT_CELLS: f32 = 0.1;
/// Widest footprint in cells, the largest grid side: a stray vertex far
/// away can't make a tile that floods the cell index.
pub const MAX_CELLS: u32 = 2000;

/// The unit an STL file was authored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceUnit {
    #[default]
    Millimetres,
    Inches,
}

impl SourceUnit {
    /// Guesses the unit from the bounding box, like the Python version did.
    pub fn detect(dx: f64, dy: f64) -> Self {
        let side = dx.max(dy);
        if side > 0.0 && side < INCH_HEURISTIC_LIMIT { Self::Inches } else { Self::Millimetres }
    }

    /// Millimetres per file unit.
    pub fn scale(self) -> f64 {
        match self {
            Self::Millimetres => 1.0,
            Self::Inches => INCH_MM,
        }
    }
}

/// A tile's size in cells at rotation 0: whole cells across, fractional up.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TileSize {
    pub w: u32,
    pub h: u32,
    pub gz: f32,
}

impl TileSize {
    pub const PLACEHOLDER: Self = Self { w: 1, h: 1, gz: 0.5 };

    /// Footprint from a bounding box given in file units.
    ///
    /// Rounds half to even, as Python's `round` does, so files sized exactly
    /// between two cell counts get the same footprint as before.
    pub fn from_extent(dx: f64, dy: f64, dz: f64, unit: SourceUnit) -> Self {
        let s = unit.scale();
        let cells = |mm: f64| ((mm / CELL_MM).round_ties_even().min(f64::from(MAX_CELLS)) as u32).max(1);
        Self { w: cells(dx * s), h: cells(dy * s), gz: ((dz * s / CELL_MM) as f32).max(MIN_HEIGHT_CELLS) }
    }

    /// Height in millimetres.
    pub fn height_mm(self) -> f64 {
        f64::from(self.gz) * CELL_MM
    }
}

/// Formats a length in cells as 1" squares and millimetres, e.g. `3" (75 mm)`.
pub fn describe_cells(cells: f32) -> String {
    let inches = f64::from(cells) / f64::from(CELLS_PER_INCH_SQUARE);
    let mm = f64::from(cells) * CELL_MM;
    if (inches - inches.round()).abs() < 1e-6 {
        format!("{}\" ({mm:.0} mm)", inches.round())
    } else {
        format!("{inches:.1}\" ({mm:.0} mm)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inch_heuristic() {
        assert_eq!(SourceUnit::detect(2.0, 0.5), SourceUnit::Inches);
        assert_eq!(SourceUnit::detect(25.0, 25.0), SourceUnit::Millimetres);
        assert_eq!(SourceUnit::detect(0.0, 0.0), SourceUnit::Millimetres);
        assert_eq!(SourceUnit::detect(5.0, 1.0), SourceUnit::Millimetres);
    }

    #[test]
    fn sizes_round_half_to_even() {
        // 31.25 mm is 2.5 cells: Python's round gives 2, not 3
        assert_eq!(TileSize::from_extent(31.25, 43.75, 6.0, SourceUnit::Millimetres).w, 2);
        assert_eq!(TileSize::from_extent(31.25, 43.75, 6.0, SourceUnit::Millimetres).h, 4);
        assert_eq!(TileSize::from_extent(1.0, 1.0, 0.0, SourceUnit::Millimetres), TileSize { w: 1, h: 1, gz: 0.1 });
    }

    #[test]
    fn a_stray_far_vertex_cannot_make_a_huge_tile() {
        let size = TileSize::from_extent(1.0e7, 25.0, 6.0, SourceUnit::Millimetres);
        assert_eq!((size.w, size.h), (MAX_CELLS, 2));
    }

    #[test]
    fn describes_lengths() {
        assert_eq!(describe_cells(6.0), "3\" (75 mm)");
        assert_eq!(describe_cells(3.0), "1.5\" (38 mm)");
    }
}
