//! Build plan exports of Modular Terrain Planner: what to print and where
//! each printed tile goes.
//!
//! Everything starts from an [`ExportInput`]. [`print_list`] counts the STL
//! files to print (one row per file, so same-named files from different packs
//! stay apart), [`placements`] numbers the tiles in reading order, and
//! [`map_layout`] turns a level into a flat drawing model. The PNG and PDF
//! exports draw that one model with the same code, so the two can't disagree
//! about where a tile is or which way it faces.

pub mod csv;
mod draw;
pub mod info;
pub mod map_layout;
pub mod pdf;
pub mod placements;
pub mod png;
pub mod print_list;
mod text;

use std::io;
use std::path::{Path, PathBuf};

use mtp_core::document::DocState;
use mtp_core::tile::Placed;

pub use crate::csv::{PathMode, export_csv, write_csv};
pub use crate::info::{DefaultInfo, TileInfo, TileMeta};
pub use crate::map_layout::{CellBounds, MapLayout, MapTile, map_layout};
pub use crate::pdf::{Paper, PdfOptions, export_pdf, render_pdf};
pub use crate::placements::placements;
pub use crate::png::{export_png, render_png};
pub use crate::print_list::{PrintRow, PrintTotals, print_list};

/// What every export reads: the document, how to describe its STL files and
/// which level to export.
#[derive(Clone)]
pub struct ExportInput<'a> {
    /// Shown at the top of every page and image.
    pub title: String,
    pub state: &'a DocState,
    pub info: &'a dyn TileInfo,
    /// `None` exports every level; `Some(n)` only the tiles on level `n`.
    pub level: Option<i32>,
}

impl<'a> ExportInput<'a> {
    /// The tiles this export covers, in document order.
    pub fn tiles(&self) -> impl Iterator<Item = &'a Placed> + use<'a> {
        let level = self.level;
        self.state.tiles.iter().filter(move |t| level.is_none_or(|l| t.level == l))
    }

    /// The levels present, lowest first.
    pub fn levels(&self) -> Vec<i32> {
        let mut levels: Vec<i32> = self.tiles().map(|t| t.level).collect();
        levels.sort_unstable();
        levels.dedup();
        levels
    }

    fn check_not_empty(&self) -> Result<(), ExportError> {
        if self.tiles().next().is_none() { Err(ExportError::Empty) } else { Ok(()) }
    }
}

/// Why an export failed, worded for the person who asked for it.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("Nothing to export: place some tiles first.")]
    Empty,
    #[error("Couldn't write {}: {source}. Is it open in another program?", path.display())]
    Write { path: PathBuf, source: io::Error },
    #[error("Couldn't create the {format}: {detail}")]
    Encode { format: &'static str, detail: String },
    #[error("The bundled font couldn't be loaded, so text can't be drawn.")]
    Font,
}

/// Writes a finished export next to its destination and renames it into
/// place, so a failed write never leaves a half-written file behind.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    mtp_core::persist::write_atomic(path, bytes, false)
        .map_err(|source| ExportError::Write { path: path.to_path_buf(), source })
}

#[cfg(test)]
pub(crate) mod testutil {
    use mtp_core::document::DocState;
    use mtp_core::footprint::Rotation;
    use mtp_core::grid::GridSize;
    use mtp_core::tile::{Placed, PlacedId, TileKey};
    use mtp_core::units::TileSize;

    /// A placed tile with a known size.
    pub fn tile(id: u64, path: &str, w: u32, h: u32, x: f32, y: f32) -> Placed {
        Placed {
            id: PlacedId(id),
            key: TileKey::new(path),
            size: TileSize { w, h, gz: 0.48 },
            size_known: true,
            x,
            y,
            z: 0.0,
            rotation: Rotation::ZERO,
            level: 0,
        }
    }

    pub fn state(cols: u32, rows: u32, tiles: Vec<Placed>) -> DocState {
        DocState { grid: GridSize { cols, rows }, tiles, ground_image: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{state, tile};

    #[test]
    fn empty_layouts_have_nothing_to_export() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut s = state(10, 10, Vec::new());
        let input = ExportInput { title: "Empty".into(), state: &s, info: &DefaultInfo, level: None };
        let errors = [
            export_csv(&dir.path().join("a.csv"), &input, PathMode::Relative),
            export_png(&dir.path().join("a.png"), &input),
            export_pdf(&dir.path().join("a.pdf"), &input, &PdfOptions::default()),
        ];
        for e in errors {
            assert_eq!(e.map_err(|e| e.to_string()), Err("Nothing to export: place some tiles first.".to_string()));
        }
        assert_eq!(std::fs::read_dir(dir.path()).expect("list").count(), 0, "no files written");

        // A level with no tiles on it is just as empty
        s.tiles.push(tile(1, "a/b.stl", 1, 1, 0.0, 0.0));
        let input = ExportInput { title: "Empty".into(), state: &s, info: &DefaultInfo, level: Some(3) };
        assert!(matches!(render_png(&input), Err(ExportError::Empty)));
    }

    #[test]
    fn write_errors_name_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let s = state(10, 10, vec![tile(1, "a/b.stl", 1, 1, 0.0, 0.0)]);
        let input = ExportInput { title: "Test".into(), state: &s, info: &DefaultInfo, level: None };
        let path = dir.path().join("missing folder").join("plan.csv");
        let message = export_csv(&path, &input, PathMode::Relative).expect_err("no folder").to_string();
        assert!(message.starts_with("Couldn't write ") && message.contains("plan.csv"), "{message}");
        assert!(message.ends_with("Is it open in another program?"), "{message}");
    }
}
