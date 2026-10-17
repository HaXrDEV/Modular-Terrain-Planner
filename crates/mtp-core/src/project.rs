//! `.mtp` project files.
//!
//! Versions 1 to 4 came from the Python app: JSON with absolute paths. They
//! are read exactly as it read them (checked against its golden output), but
//! damaged records become warnings instead of a crash.
//!
//! Version 5 adds what the old format couldn't express, and the Python app
//! lost data over:
//! - `app_version` and `min_reader_version`, so a newer file says whether
//!   this version can still read it;
//! - a folder table, with each folder stored relative to the project when
//!   possible and absolute as a fallback, so a project and its tiles can move
//!   together, also between Windows, macOS and Linux;
//! - each tile's size, so tiles whose STL is missing keep their footprint and
//!   are written back unchanged;
//! - a `level` per tile (0 unless set).

use std::fmt::Write as _;

use serde_json::{Map, Value, json};

use crate::document::{DocState, GroundImage};
use crate::footprint::Rotation;
use crate::grid::GridSize;
use crate::tile::{Placed, PlacedId, TileKey};
use crate::units::TileSize;

/// The version this app writes.
pub const FORMAT_VERSION: u64 = 5;
/// Files can say they need at least this reader; ours reads up to [`FORMAT_VERSION`].
pub const MIN_READER_VERSION: u64 = 5;
/// The default extension, without the dot.
pub const EXTENSION: &str = "mtp";

#[derive(Clone, Debug, PartialEq)]
pub struct FolderRecord {
    /// As saved: relative to the project file when it was, else absolute.
    pub path: String,
    /// Where it was when saved, for when the relative path doesn't resolve.
    pub absolute: Option<String>,
    /// Whether subfolders were loaded too.
    pub recursive: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TileRecord {
    /// Index into [`ProjectFile::folders`].
    pub folder: usize,
    /// Path inside that folder, with forward slashes.
    pub file: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rotation: Rotation,
    pub level: i32,
    /// Unknown in files from the Python app.
    pub size: Option<TileSize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageRecord {
    pub path: String,
    pub absolute: Option<String>,
    pub rect: [f32; 4],
}

/// The contents of a project file, before its folders are looked for on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectFile {
    /// The version the file was written in.
    pub version: u64,
    pub app_version: Option<String>,
    pub grid: GridSize,
    pub folders: Vec<FolderRecord>,
    pub tiles: Vec<TileRecord>,
    pub ground_image: Option<ImageRecord>,
    /// Problems found while reading, for the Problems panel.
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ProjectError {
    #[error("this isn't a project file (it isn't valid JSON: {0})")]
    NotJson(String),
    #[error("this isn't a project file (it doesn't hold a project)")]
    NotAProject,
    #[error("this project was saved by a newer version of the app (format {version}); update the app to open it")]
    TooNew { version: u64 },
}

/// Reads a project file of any version.
pub fn read(bytes: &[u8]) -> Result<ProjectFile, ProjectError> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let value: Value = serde_json::from_slice(bytes).map_err(|e| ProjectError::NotJson(e.to_string()))?;
    let Value::Object(root) = value else {
        return Err(ProjectError::NotAProject);
    };
    let mut warnings = Vec::new();
    let version = match root.get("version") {
        None => 1,
        Some(v) => match as_f64(v) {
            Some(n) if n >= 1.0 && n.fract() == 0.0 => n as u64,
            _ => {
                warnings.push(format!("The file's version ({v}) isn't a number; read as the newest known version."));
                4
            }
        },
    };
    if version > FORMAT_VERSION {
        let min_reader = root.get("min_reader_version").and_then(as_f64).map_or(version, |n| n as u64);
        if min_reader > FORMAT_VERSION {
            return Err(ProjectError::TooNew { version });
        }
        warnings.push(format!(
            "This project was saved by a newer version of the app (format {version}). \
             Anything this version doesn't know about is lost if you save it."
        ));
    }
    let mut file = if version <= 4 { read_v1_to_v4(&root, &mut warnings) } else { read_v5(&root, &mut warnings) };
    file.version = version;
    file.warnings = warnings;
    Ok(file)
}

/// Numbers, also when written as strings ("4", "NaN").
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn finite(v: Option<&Value>) -> Option<f32> {
    v.and_then(as_f64).filter(|n| n.is_finite()).map(|n| n as f32)
}

fn read_grid(cols: Option<&Value>, rows: Option<&Value>, default: u32, warnings: &mut Vec<String>) -> GridSize {
    let mut side = |v: Option<&Value>, what: &str| match v.and_then(as_f64) {
        None => default,
        Some(n) if n.is_finite() && n >= 1.0 && n <= f64::from(GridSize::MAX) => n.round() as u32,
        Some(n) => {
            let clamped = if n.is_finite() { n.clamp(1.0, f64::from(GridSize::MAX)) as u32 } else { default };
            warnings.push(format!("The grid's {what} ({n}) was out of range; using {clamped}."));
            clamped
        }
    };
    GridSize { cols: side(cols, "width"), rows: side(rows, "depth") }
}

fn read_rotation(v: Option<&Value>, what: &str, warnings: &mut Vec<String>) -> Rotation {
    let raw = v.and_then(as_f64).unwrap_or(0.0);
    match Rotation::from_degrees(raw) {
        Some(r) => {
            if f64::from(r.degrees()) != raw.rem_euclid(360.0) {
                warnings.push(format!("{what} was turned {raw}°; it is now turned {}°.", r.degrees()));
            }
            r
        }
        None => {
            warnings.push(format!("{what} had no valid turn; it is now turned 0°."));
            Rotation::ZERO
        }
    }
}

fn read_image(v: Option<&Value>, warnings: &mut Vec<String>) -> Option<ImageRecord> {
    let obj = v?.as_object()?;
    let path = obj.get("path")?.as_str()?.to_owned();
    let absolute = obj.get("absolute").and_then(Value::as_str).map(str::to_owned);
    let rect: Vec<f32> = obj
        .get("rect")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| finite(Some(x))).collect())
        .unwrap_or_default();
    if rect.len() != 4 || rect[2] <= 0.0 || rect[3] <= 0.0 {
        // A zero size tells the app to fit the image to the grid again
        warnings.push(format!("The battle map {path} had no usable size; it is placed to fill the grid."));
        return Some(ImageRecord { path, absolute, rect: [0.0; 4] });
    }
    Some(ImageRecord { path, absolute, rect: [rect[0], rect[1], rect[2], rect[3]] })
}

/// The Python app's files, with its migrations: v1 had no grid size (40 × 40),
/// v2 no heights (0), v3 no battle map.
fn read_v1_to_v4(root: &Map<String, Value>, warnings: &mut Vec<String>) -> ProjectFile {
    let grid = read_grid(root.get("grid_cols"), root.get("grid_rows"), 40, warnings);
    let mut folders: Vec<FolderRecord> = match root.get("folders") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|f| match f.as_str() {
                Some(s) => Some(FolderRecord { path: s.replace('\\', "/"), absolute: None, recursive: false }),
                None => {
                    warnings.push(format!("A tile folder entry ({f}) wasn't a path and was left out."));
                    None
                }
            })
            .collect(),
        Some(other) => {
            warnings.push(format!("The tile folder list ({other}) wasn't a list and was left out."));
            Vec::new()
        }
    };

    let mut tiles = Vec::new();
    let list = root.get("tiles").and_then(Value::as_array).cloned().unwrap_or_default();
    for (i, t) in list.iter().enumerate() {
        let what = format!("Tile {}", i + 1);
        let Some(obj) = t.as_object() else {
            warnings.push(format!("{what} wasn't a tile record and was left out."));
            continue;
        };
        let Some(path) = obj.get("stl_path").and_then(Value::as_str) else {
            warnings.push(format!("{what} had no STL file and was left out."));
            continue;
        };
        let (Some(x), Some(y)) = (finite(obj.get("grid_x")), finite(obj.get("grid_y"))) else {
            warnings.push(format!("{what} ({path}) had no valid position and was left out."));
            continue;
        };
        let key = TileKey::new(path);
        let folder = folders
            .iter()
            .enumerate()
            .filter(|(_, f)| key.is_inside(&f.path))
            .max_by_key(|(_, f)| f.path.len())
            .map(|(i, _)| i)
            .unwrap_or_else(|| {
                folders.push(FolderRecord { path: key.folder().to_owned(), absolute: None, recursive: false });
                folders.len() - 1
            });
        let file = key.relative_to(&folders[folder].path).unwrap_or_else(|| key.file_name().to_owned());
        tiles.push(TileRecord {
            folder,
            file,
            x,
            y,
            z: finite(obj.get("z_offset")).unwrap_or(0.0),
            rotation: read_rotation(obj.get("rotation"), &format!("{what} ({path})"), warnings),
            level: 0,
            size: None,
        });
    }

    ProjectFile {
        version: 0,
        app_version: None,
        grid,
        folders,
        tiles,
        ground_image: read_image(root.get("ground_image"), warnings),
        warnings: Vec::new(),
    }
}

fn read_v5(root: &Map<String, Value>, warnings: &mut Vec<String>) -> ProjectFile {
    let grid = root.get("grid").and_then(Value::as_object);
    let grid = read_grid(grid.and_then(|g| g.get("cols")), grid.and_then(|g| g.get("rows")), 80, warnings);
    let folders: Vec<FolderRecord> = root
        .get("folders")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|f| FolderRecord {
                    path: f.get("path").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    absolute: f.get("absolute").and_then(Value::as_str).map(str::to_owned),
                    recursive: f.get("recursive").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();

    let mut tiles = Vec::new();
    for (i, t) in root.get("tiles").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let what = format!("Tile {}", i + 1);
        let folder = t.get("folder").and_then(Value::as_u64).map(|f| f as usize).filter(|&f| f < folders.len());
        let file = t.get("file").and_then(Value::as_str);
        let (Some(folder), Some(file)) = (folder, file) else {
            warnings.push(format!("{what} had no STL file and was left out."));
            continue;
        };
        let (Some(x), Some(y)) = (finite(t.get("x")), finite(t.get("y"))) else {
            warnings.push(format!("{what} ({file}) had no valid position and was left out."));
            continue;
        };
        let size = t.get("size").and_then(|s| {
            let w = s.get("w").and_then(Value::as_u64)?;
            let h = s.get("h").and_then(Value::as_u64)?;
            let gz = finite(s.get("gz"))?;
            let fits = |n: u64| (1..=u64::from(crate::units::MAX_CELLS)).contains(&n);
            (fits(w) && fits(h) && gz > 0.0).then_some(TileSize { w: w as u32, h: h as u32, gz })
        });
        tiles.push(TileRecord {
            folder,
            file: file.to_owned(),
            x,
            y,
            z: finite(t.get("z")).unwrap_or(0.0),
            rotation: read_rotation(t.get("rotation"), &format!("{what} ({file})"), warnings),
            level: t.get("level").and_then(Value::as_i64).map_or(0, |l| l as i32),
            size,
        });
    }

    ProjectFile {
        version: 0,
        app_version: root.get("app_version").and_then(Value::as_str).map(str::to_owned),
        grid,
        folders,
        tiles,
        ground_image: read_image(root.get("ground_image"), warnings),
        warnings: Vec::new(),
    }
}

/// Writes the newest format as pretty JSON.
pub fn write(file: &ProjectFile, app_version: &str) -> String {
    let folders: Vec<Value> = file
        .folders
        .iter()
        .map(|f| {
            let mut o = json!({ "path": f.path });
            if let Some(abs) = &f.absolute {
                o["absolute"] = json!(abs);
            }
            if f.recursive {
                o["recursive"] = json!(true);
            }
            o
        })
        .collect();
    let tiles: Vec<Value> = file
        .tiles
        .iter()
        .map(|t| {
            let mut o = json!({
                "folder": t.folder,
                "file": t.file,
                "x": t.x,
                "y": t.y,
                "z": t.z,
                "rotation": t.rotation.degrees(),
            });
            if t.level != 0 {
                o["level"] = json!(t.level);
            }
            if let Some(s) = t.size {
                o["size"] = json!({ "w": s.w, "h": s.h, "gz": s.gz });
            }
            o
        })
        .collect();
    let mut root = json!({
        "format": "Modular Terrain Planner project",
        "version": FORMAT_VERSION,
        "min_reader_version": MIN_READER_VERSION,
        "app_version": app_version,
        "grid": { "cols": file.grid.cols, "rows": file.grid.rows },
        "folders": folders,
        "tiles": tiles,
    });
    if let Some(img) = &file.ground_image {
        let mut o = json!({ "path": img.path, "rect": img.rect });
        if let Some(abs) = &img.absolute {
            o["absolute"] = json!(abs);
        }
        root["ground_image"] = o;
    }
    let mut out = serde_json::to_string_pretty(&root).unwrap_or_default();
    let _ = writeln!(out);
    out
}

/// Path of `target` relative to the directory `base`, both with forward
/// slashes and absolute. `None` when they share no root (another drive).
pub fn relative_path(base: &str, target: &str) -> Option<String> {
    let windows = |s: &str| s.as_bytes().get(1) == Some(&b':');
    let fold = windows(base) || windows(target) || cfg!(windows);
    let parts = |s: &str| -> Vec<String> {
        s.replace('\\', "/").split('/').filter(|p| !p.is_empty() && *p != ".").map(str::to_owned).collect()
    };
    let (b, t) = (parts(base), parts(target));
    let same = |x: &String, y: &String| if fold { x.eq_ignore_ascii_case(y) } else { x == y };
    if b.is_empty() || t.is_empty() || !same(&b[0], &t[0]) {
        return None;
    }
    let common = b.iter().zip(&t).take_while(|(x, y)| same(x, y)).count();
    let mut rel: Vec<&str> = vec![".."; b.len() - common];
    rel.extend(t[common..].iter().map(String::as_str));
    Some(if rel.is_empty() { ".".to_owned() } else { rel.join("/") })
}

/// Joins a saved path onto the project's directory unless it is absolute.
pub fn join(base: &str, path: &str) -> String {
    let p = path.replace('\\', "/");
    let absolute = p.starts_with('/') || p.as_bytes().get(1) == Some(&b':');
    if absolute || base.is_empty() {
        return p;
    }
    let mut parts: Vec<&str> = base.trim_end_matches('/').split('/').collect();
    // The root ("" before a leading /, or a drive) can't be left
    let at_root = |parts: &[&str]| parts.len() == 1 && (parts[0].is_empty() || parts[0].ends_with(':'));
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => match parts.last() {
                None | Some(&"..") => parts.push(".."),
                Some(_) if at_root(&parts) => {}
                Some(_) => {
                    parts.pop();
                }
            },
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// A folder open in the library when saving.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryFolder {
    /// Absolute path with forward slashes.
    pub path: String,
    pub recursive: bool,
}

/// Where a project's folder was when saved, if it can't be found now: the
/// absolute path if one was stored, else the saved path joined onto the
/// project's directory.
pub fn saved_location(project_dir: &str, folder: &FolderRecord) -> String {
    folder.absolute.clone().unwrap_or_else(|| join(project_dir, &folder.path))
}

/// Finds a project's folder on disk: first relative to the project (so the
/// two can move together), then where it was when saved.
pub fn locate(project_dir: &str, folder: &FolderRecord, exists: impl Fn(&str) -> bool) -> Option<String> {
    let mut candidates = vec![join(project_dir, &folder.path)];
    candidates.extend(folder.absolute.clone());
    candidates.into_iter().find(|c| exists(c))
}

/// The document a project file describes. `found` holds, for each folder,
/// where it is now (`None` keeps the saved location, so its tiles are saved
/// again unchanged). Tiles keep every saved position, overlapping or not.
pub fn to_state(file: &ProjectFile, project_dir: &str, found: &[Option<String>], image: Option<String>) -> DocState {
    let folder_paths: Vec<String> = file
        .folders
        .iter()
        .enumerate()
        .map(|(i, f)| found.get(i).cloned().flatten().unwrap_or_else(|| saved_location(project_dir, f)))
        .collect();
    let tiles = file
        .tiles
        .iter()
        .enumerate()
        .map(|(i, t)| Placed {
            id: PlacedId(i as u64 + 1),
            key: TileKey::new(&format!("{}/{}", folder_paths[t.folder].trim_end_matches('/'), t.file)),
            size: t.size.unwrap_or(TileSize::PLACEHOLDER),
            size_known: t.size.is_some(),
            x: t.x,
            y: t.y,
            z: t.z,
            rotation: t.rotation,
            level: t.level,
        })
        .collect();
    let ground_image = file.ground_image.as_ref().map(|img| GroundImage {
        path: image.clone().unwrap_or_else(|| img.absolute.clone().unwrap_or_else(|| join(project_dir, &img.path))),
        rect: img.rect,
    });
    DocState { grid: file.grid, tiles, ground_image }
}

/// The project file for a document, with paths relative to `project_dir`
/// where possible. The folder table holds the open library folders plus any
/// folder a tile comes from, so tiles from a closed folder aren't lost.
pub fn from_state(state: &DocState, open: &[LibraryFolder], project_dir: &str) -> ProjectFile {
    let mut folders: Vec<LibraryFolder> = Vec::new();
    for f in open {
        if !folders.iter().any(|g| TileKey::new(&g.path) == TileKey::new(&f.path)) {
            folders.push(f.clone());
        }
    }
    let mut tiles = Vec::with_capacity(state.tiles.len());
    for t in &state.tiles {
        let folder = folders
            .iter()
            .enumerate()
            .filter(|(_, f)| t.key.is_inside(&f.path))
            .max_by_key(|(_, f)| f.path.len())
            .map(|(i, _)| i)
            .unwrap_or_else(|| {
                folders.push(LibraryFolder { path: t.key.folder().to_owned(), recursive: false });
                folders.len() - 1
            });
        tiles.push(TileRecord {
            folder,
            file: t.key.relative_to(&folders[folder].path).unwrap_or_else(|| t.key.file_name().to_owned()),
            x: t.x,
            y: t.y,
            z: t.z,
            rotation: t.rotation,
            level: t.level,
            size: t.size_known.then_some(t.size),
        });
    }
    let saved_path = |abs: &str| relative_path(project_dir, abs).unwrap_or_else(|| abs.to_owned());
    ProjectFile {
        version: FORMAT_VERSION,
        app_version: None,
        grid: state.grid,
        folders: folders
            .iter()
            .map(|f| FolderRecord {
                path: saved_path(&f.path),
                absolute: Some(TileKey::new(&f.path).as_str().to_owned()),
                recursive: f.recursive,
            })
            .collect(),
        tiles,
        ground_image: state.ground_image.as_ref().map(|img| ImageRecord {
            path: saved_path(&img.path),
            absolute: Some(TileKey::new(&img.path).as_str().to_owned()),
            rect: img.rect,
        }),
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(relative_path("C:/Maps/Castle", "C:/Tiles/OpenForge").as_deref(), Some("../../Tiles/OpenForge"));
        assert_eq!(relative_path("C:/Maps", "c:/maps/tiles").as_deref(), Some("tiles"));
        assert_eq!(relative_path("C:/Maps", "D:/Tiles"), None);
        assert_eq!(relative_path("/home/a/maps", "/home/a/tiles/x").as_deref(), Some("../tiles/x"));
        assert_eq!(join("/home/a/maps", "../tiles/x"), "/home/a/tiles/x");
        assert_eq!(join("/home/a/maps", "C:/Tiles"), "C:/Tiles");
        assert_eq!(join("maps", "../Tiles"), "Tiles");
        assert_eq!(join("maps", "../../Tiles"), "../Tiles");
        assert_eq!(join("/home", "../../x"), "/x");
        assert_eq!(join("C:/Maps", "../../Tiles"), "C:/Tiles");
    }

    #[test]
    fn round_trips_the_newest_format() {
        let file = ProjectFile {
            version: FORMAT_VERSION,
            app_version: Some("0.5.0".into()),
            grid: GridSize { cols: 60, rows: 40 },
            folders: vec![FolderRecord { path: "../Tiles".into(), absolute: Some("C:/Tiles".into()), recursive: true }],
            tiles: vec![TileRecord {
                folder: 0,
                file: "walls/wall.stl".into(),
                x: 3.0,
                y: 4.5,
                z: 0.48,
                rotation: Rotation::from_quarters(3),
                level: 2,
                size: Some(TileSize { w: 2, h: 1, gz: 4.064 }),
            }],
            ground_image: Some(ImageRecord { path: "cave.png".into(), absolute: None, rect: [0.0, 0.0, 60.0, 40.0] }),
            warnings: Vec::new(),
        };
        let text = write(&file, "0.5.0");
        assert_eq!(read(text.as_bytes()), Ok(file));
    }

    #[test]
    fn newer_files_open_when_they_allow_it() {
        let ok = read(br#"{"version": 6, "min_reader_version": 5, "tiles": []}"#).expect("readable");
        assert_eq!(ok.warnings.len(), 1);
        assert_eq!(read(br#"{"version": 6, "min_reader_version": 6}"#), Err(ProjectError::TooNew { version: 6 }));
    }

    #[test]
    fn a_battle_map_without_a_size_is_kept() {
        let file = read(br#"{"version": 5, "ground_image": {"path": "map.png", "absolute": "C:/Maps/map.png"}}"#)
            .expect("readable");
        let img = file.ground_image.expect("kept");
        assert_eq!((img.rect, img.absolute.as_deref()), ([0.0; 4], Some("C:/Maps/map.png")));
        assert_eq!(file.warnings.len(), 1);
    }

    #[test]
    fn folders_matched_without_case_of_another_length() {
        let text = r#"{"version": 4, "folders": ["C:/ẞẞ"], "tiles": [{"stl_path": "C:/ßß/éa.stl", "grid_x": 0, "grid_y": 0, "rotation": 0}]}"#;
        let file = read(text.as_bytes()).expect("readable");
        assert_eq!(file.tiles[0].file, "éa.stl");
    }

    #[test]
    fn huge_or_wrapping_sizes_are_not_trusted() {
        let text = r#"{"version": 5, "folders": [{"path": "C:/T"}], "tiles": [
            {"folder": 0, "file": "a.stl", "x": 0, "y": 0, "size": {"w": 4294967296, "h": 1, "gz": 1}},
            {"folder": 0, "file": "b.stl", "x": 0, "y": 0, "size": {"w": 100000, "h": 100000, "gz": 1}}]}"#;
        let file = read(text.as_bytes()).expect("readable");
        assert!(file.tiles.iter().all(|t| t.size.is_none()), "sizes are learnt again from the STL");
    }

    #[test]
    fn reads_a_byte_order_mark() {
        assert!(read(b"\xef\xbb\xbf{\"version\": 4}").is_ok());
    }
}
