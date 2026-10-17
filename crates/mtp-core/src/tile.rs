//! Placed tiles and the keys that tie them to STL files.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use crate::footprint::{self, Rotation};
use crate::units::TileSize;

/// Identifies an STL file across the library, the document and the caches:
/// its path with forward slashes, compared without case on Windows.
#[derive(Clone, Eq)]
pub struct TileKey {
    /// The path as given, with forward slashes, for display and saving.
    path: Arc<str>,
    /// What equality and hashing use.
    norm: Arc<str>,
}

impl TileKey {
    pub fn new(path: &str) -> Self {
        let mut p = path.replace('\\', "/");
        while p.len() > 1 && p.ends_with('/') {
            p.pop();
        }
        let norm = if cfg!(windows) || looks_like_windows(&p) { p.to_lowercase() } else { p.clone() };
        Self { path: p.into(), norm: norm.into() }
    }

    pub fn from_path(path: &Path) -> Self {
        Self::new(&path.to_string_lossy())
    }

    pub fn as_str(&self) -> &str {
        &self.path
    }

    /// File name with extension.
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// File name without the extension: what the app shows as the tile's name.
    pub fn stem(&self) -> &str {
        let name = self.file_name();
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    }

    /// The folder holding the file, as a path string with forward slashes.
    pub fn folder(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(dir, _)| dir)
    }

    /// The name of the folder holding the file.
    pub fn folder_name(&self) -> &str {
        let dir = self.folder();
        dir.rsplit('/').next().unwrap_or(dir)
    }

    /// The path inside `dir`, compared by whole components (without case
    /// where keys are), or `None` when the file isn't inside it.
    pub fn relative_to(&self, dir: &str) -> Option<String> {
        let dir = TileKey::new(dir);
        let fold = cfg!(windows) || looks_like_windows(&self.path) || looks_like_windows(&dir.path);
        let mine: Vec<&str> = self.path.split('/').collect();
        let theirs: Vec<&str> = dir.path.split('/').collect();
        if theirs.len() >= mine.len() {
            return None;
        }
        let same = |a: &&str, b: &&str| if fold { a.to_lowercase() == b.to_lowercase() } else { a == b };
        mine.iter().zip(&theirs).all(|(a, b)| same(a, b)).then(|| mine[theirs.len()..].join("/"))
    }

    /// Whether `self` lies inside `dir` (compared by whole path components).
    pub fn is_inside(&self, dir: &str) -> bool {
        let dir = TileKey::new(dir);
        self.norm.len() > dir.norm.len()
            && self.norm.starts_with(&*dir.norm)
            && self.norm.as_bytes()[dir.norm.len()] == b'/'
    }
}

/// A drive-letter path like `C:/Tiles` is compared without case even on
/// Linux and macOS, so projects saved on Windows match up.
fn looks_like_windows(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

impl PartialEq for TileKey {
    fn eq(&self, other: &Self) -> bool {
        self.norm == other.norm
    }
}

impl std::hash::Hash for TileKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.norm.hash(state);
    }
}

impl PartialOrd for TileKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TileKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.norm.cmp(&other.norm)
    }
}

impl fmt::Debug for TileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TileKey({:?})", self.path)
    }
}

impl fmt::Display for TileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path)
    }
}

/// Stable identity of a placed tile within a document, kept through undo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlacedId(pub u64);

/// An axis-aligned box in cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box3 {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// A tile on the grid. It carries its own size, so a tile whose STL can't be
/// found still has a footprint, is drawn as a placeholder and is saved again.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub id: PlacedId,
    pub key: TileKey,
    pub size: TileSize,
    /// False when the size is a placeholder (old project, STL not found yet).
    pub size_known: bool,
    pub x: f32,
    pub y: f32,
    /// Height of the tile's base, in cells.
    pub z: f32,
    pub rotation: Rotation,
    pub level: i32,
}

impl Placed {
    pub fn effective(&self) -> (u32, u32) {
        footprint::effective_size(self.size, self.rotation)
    }

    pub fn cells(&self) -> impl Iterator<Item = (i32, i32)> + use<> {
        let (w, h) = self.effective();
        footprint::cells(self.x, self.y, w, h)
    }

    pub fn top(&self) -> f32 {
        self.z + self.size.gz
    }

    pub fn bounds(&self) -> Box3 {
        let (w, h) = self.effective();
        Box3 { min: [self.x, self.y, self.z], max: [self.x + w as f32, self.y + h as f32, self.top()] }
    }

    pub fn rect(&self) -> footprint::Rect {
        let (w, h) = self.effective();
        footprint::Rect { x: self.x, y: self.y, w: w as f32, h: h as f32 }
    }

    pub fn model_matrix(&self) -> [f32; 16] {
        footprint::model_matrix(self.x, self.y, self.z, self.size, self.rotation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_compare_paths_not_spelling() {
        assert_eq!(TileKey::new(r"C:\Tiles\Floor.stl"), TileKey::new("c:/tiles/floor.stl"));
        assert_eq!(TileKey::new("C:/Tiles/floor.stl").stem(), "floor");
        assert_eq!(TileKey::new("C:/Tiles/floor.stl").folder_name(), "Tiles");
    }

    #[test]
    fn paths_inside_folders_survive_case_that_changes_length() {
        // Lowercasing "ẞ" gives "ß", which is shorter in UTF-8
        let key = TileKey::new("C:/ßß/éa.stl");
        assert_eq!(key.relative_to("C:/ẞẞ").as_deref(), Some("éa.stl"));
        assert_eq!(TileKey::new("C:/Tiles/walls/a.stl").relative_to("c:/tiles").as_deref(), Some("walls/a.stl"));
        assert_eq!(TileKey::new("C:/Tiles-extra/a.stl").relative_to("C:/Tiles"), None);
    }

    #[test]
    fn inside_matches_whole_components() {
        let key = TileKey::new("C:/Tiles-extra/floor.stl");
        assert!(!key.is_inside("C:/Tiles"));
        assert!(key.is_inside("C:/Tiles-extra"));
        assert!(key.is_inside("c:\\tiles-extra\\"));
    }
}
