//! The print list: how many of each STL file to print.
//!
//! Rows are keyed by file, never by name. The Python version merged
//! same-named files from different packs into one row, so a plan asking for
//! six `floor_2x2` didn't say which pack's floor.

use std::collections::{HashMap, HashSet};

use mtp_core::tile::TileKey;
use mtp_core::units::TileSize;

use crate::ExportInput;

/// One STL file and how many times the layout uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintRow {
    pub key: TileKey,
    pub name: String,
    pub folder: String,
    /// The name, or `name (folder)` when another row has the same name.
    pub display_name: String,
    pub count: u32,
    pub size: TileSize,
    /// Solid volume of one print, in cubic millimetres.
    pub volume_mm3: Option<f64>,
}

/// Sums over a print list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintTotals {
    /// Tiles to print.
    pub tiles: u32,
    /// Distinct STL files.
    pub files: usize,
    /// Solid volume of every print whose file has a known volume, in mm³.
    pub volume_mm3: f64,
    /// Files whose volume isn't known, so `volume_mm3` leaves them out.
    pub files_without_volume: usize,
}

impl PrintTotals {
    pub fn of(rows: &[PrintRow]) -> Self {
        let known = rows.iter().filter_map(|r| r.volume_mm3.map(|v| v * f64::from(r.count)));
        Self {
            tiles: rows.iter().map(|r| r.count).sum(),
            files: rows.len(),
            volume_mm3: known.sum(),
            files_without_volume: rows.iter().filter(|r| r.volume_mm3.is_none()).count(),
        }
    }

    /// Whether any volume is known at all.
    pub fn has_volume(&self) -> bool {
        self.files_without_volume < self.files
    }
}

/// One row per STL file the export covers, sorted by name without case, then
/// by folder.
pub fn print_list(input: &ExportInput) -> Vec<PrintRow> {
    let mut rows: Vec<PrintRow> = Vec::new();
    let mut index: HashMap<TileKey, usize> = HashMap::new();
    for t in input.tiles() {
        if let Some(&i) = index.get(&t.key) {
            let row = &mut rows[i];
            row.count += 1;
            // A placeholder size gives way to the real one
            if t.size_known && row.size == mtp_core::units::TileSize::PLACEHOLDER {
                row.size = t.size;
            }
            continue;
        }
        let meta = input.info.meta(&t.key);
        index.insert(t.key.clone(), rows.len());
        rows.push(PrintRow {
            key: t.key.clone(),
            display_name: meta.name.clone(),
            name: meta.name,
            folder: meta.folder,
            count: 1,
            size: t.size,
            volume_mm3: meta.volume_mm3,
        });
    }

    let mut seen = HashSet::new();
    let shared: HashSet<String> =
        rows.iter().map(|r| r.name.to_lowercase()).filter(|n| !seen.insert(n.clone())).collect();
    for r in &mut rows {
        if shared.contains(&r.name.to_lowercase()) {
            r.display_name = format!("{} ({})", r.name, r.folder);
        }
    }
    // Packs whose folders share a name too (two copies of "OpenForge") get
    // more of their folder path until the labels differ
    for depth in 2..=4 {
        let mut count: HashMap<String, usize> = HashMap::new();
        for r in &rows {
            *count.entry(r.display_name.to_lowercase()).or_insert(0) += 1;
        }
        let mut changed = false;
        for r in rows.iter_mut().filter(|r| count[&r.display_name.to_lowercase()] > 1) {
            let parts: Vec<&str> = r.key.folder().split('/').collect();
            let tail = parts[parts.len().saturating_sub(depth)..].join("/");
            let label = format!("{} ({tail})", r.name);
            changed |= label != r.display_name;
            r.display_name = label;
        }
        if !changed {
            break;
        }
    }

    rows.sort_by(|a, b| {
        (a.name.to_lowercase(), a.folder.to_lowercase(), &a.key).cmp(&(
            b.name.to_lowercase(),
            b.folder.to_lowercase(),
            &b.key,
        ))
    });
    rows
}

#[cfg(test)]
mod label_tests {
    use super::*;
    use crate::DefaultInfo;
    use crate::testutil::{state, tile};

    #[test]
    fn same_folder_names_in_two_places_get_told_apart() {
        let s = state(
            20,
            20,
            vec![
                tile(1, "C:/A/OpenForge/floor.stl", 2, 2, 0.0, 0.0),
                tile(2, "D:/B/OpenForge/floor.stl", 2, 2, 2.0, 0.0),
            ],
        );
        let rows = print_list(&ExportInput { title: "T".into(), state: &s, info: &DefaultInfo, level: None });
        let labels: Vec<&str> = rows.iter().map(|r| r.display_name.as_str()).collect();
        assert_eq!(labels, ["floor (A/OpenForge)", "floor (B/OpenForge)"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DefaultInfo;
    use crate::testutil::{state, tile};

    #[test]
    fn rows_are_files_not_names() {
        let s = state(
            20,
            20,
            vec![
                tile(1, "C:/Packs/OpenForge/floor.stl", 2, 2, 0.0, 0.0),
                tile(2, "C:/Packs/Dragonlock/floor.stl", 2, 2, 2.0, 0.0),
                tile(3, "C:/Packs/OpenForge/floor.stl", 2, 2, 4.0, 0.0),
                tile(4, "C:/Packs/OpenForge/Arch.stl", 1, 2, 6.0, 0.0),
            ],
        );
        let input = ExportInput { title: "Test".into(), state: &s, info: &DefaultInfo, level: None };
        let rows = print_list(&input);
        let names: Vec<(&str, u32)> = rows.iter().map(|r| (r.display_name.as_str(), r.count)).collect();
        assert_eq!(names, [("Arch", 1), ("floor (Dragonlock)", 1), ("floor (OpenForge)", 2)]);

        let totals = PrintTotals::of(&rows);
        assert_eq!((totals.tiles, totals.files), (4, 3));
        assert!(!totals.has_volume());
    }

    #[test]
    fn totals_sum_known_volumes() {
        let s = state(20, 20, vec![tile(1, "a/x.stl", 1, 1, 0.0, 0.0), tile(2, "a/x.stl", 1, 1, 1.0, 0.0)]);
        let mut info = HashMap::new();
        let key = TileKey::new("a/x.stl");
        info.insert(key.clone(), crate::TileMeta { volume_mm3: Some(1500.0), ..crate::TileMeta::from_key(&key) });
        let input = ExportInput { title: "Test".into(), state: &s, info: &info, level: None };
        let totals = PrintTotals::of(&print_list(&input));
        assert_eq!(totals.volume_mm3, 3000.0);
        assert!(totals.has_volume());
    }
}
