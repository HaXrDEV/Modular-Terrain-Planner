//! Format 6: a project in a ZIP container with the STL files its tiles use,
//! so it opens complete on any computer, like a PSD with its layers.
//!
//! - `project.json`: the project, with an `embedded` table pointing each STL
//!   file a tile uses at its entry;
//! - `stl/<folder>/<file>`: the STL files, each stored once.
//!
//! Renaming a project to `.zip` shows its STL files under their own names.
//! Formats 1 to 5 are plain JSON and are still read.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Cursor, Read, Seek, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::project::{self, EmbeddedRecord, ProjectError, ProjectFile};

/// The entry holding the project itself.
pub const PROJECT_ENTRY: &str = "project.json";

/// Whether a file starts like a container rather than JSON.
pub fn is_container(start: &[u8]) -> bool {
    start.starts_with(b"PK\x03\x04")
}

/// Reads a project file of any format. Embedded STL files aren't read here;
/// [`Embedded`] reads them when they're needed.
pub fn read_file(path: &Path) -> Result<ProjectFile, ProjectError> {
    let unreadable = |e: io::Error| ProjectError::Unreadable(e.to_string());
    let mut f = File::open(path).map_err(unreadable)?;
    let mut start = [0u8; 4];
    let n = f.read(&mut start).map_err(unreadable)?;
    if !is_container(&start[..n]) {
        let bytes = std::fs::read(path).map_err(unreadable)?;
        return project::read(&bytes);
    }
    f.rewind().map_err(unreadable)?;
    read_container(f)
}

/// Reads the project in a container, dropping table records whose entry isn't there.
pub fn read_container(reader: impl Read + Seek) -> Result<ProjectFile, ProjectError> {
    let damaged = |e: &dyn std::fmt::Display| ProjectError::Damaged(e.to_string());
    let mut zip = ZipArchive::new(reader).map_err(|e| damaged(&e))?;
    let mut json = Vec::new();
    zip.by_name(PROJECT_ENTRY)
        .map_err(|_| damaged(&"it has no project.json"))?
        .read_to_end(&mut json)
        .map_err(|e| damaged(&e))?;
    let mut file = project::read(&json)?;
    let before = file.embedded.len();
    file.embedded.retain(|e| zip.index_for_name(&e.entry).is_some());
    let lost = before - file.embedded.len();
    if lost > 0 {
        file.warnings.push(format!(
            "{lost} STL files the project lists weren't inside it. Their tiles use the files on disk if they're there."
        ));
    }
    Ok(file)
}

/// The STL files inside one container, kept open while several are read.
pub struct Embedded<R: Read + Seek> {
    zip: ZipArchive<R>,
}

impl Embedded<File> {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self { zip: ZipArchive::new(File::open(path)?).map_err(io::Error::other)? })
    }
}

impl<R: Read + Seek> Embedded<R> {
    pub fn new(reader: R) -> io::Result<Self> {
        Ok(Self { zip: ZipArchive::new(reader).map_err(io::Error::other)? })
    }

    /// One entry's bytes.
    pub fn read(&mut self, entry: &str) -> io::Result<Vec<u8>> {
        let mut file = self.zip.by_name(entry).map_err(io::Error::other)?;
        let mut out = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0));
        file.read_to_end(&mut out)?;
        Ok(out)
    }
}

/// Writes a container: the project, then each embedded STL file, asking
/// `bytes_of` for one at a time so they're never all in memory. `compress`
/// trades time for size (autosaves store them as they are).
pub fn write_to<W: Write + Seek>(
    out: W,
    file: &ProjectFile,
    app_version: &str,
    compress: bool,
    mut bytes_of: impl FnMut(&EmbeddedRecord) -> io::Result<Vec<u8>>,
) -> io::Result<W> {
    let mut zip = ZipWriter::new(out);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let stl_options =
        if compress { deflated } else { SimpleFileOptions::default().compression_method(CompressionMethod::Stored) };
    zip.start_file(PROJECT_ENTRY, deflated).map_err(io::Error::other)?;
    zip.write_all(project::write(file, app_version).as_bytes())?;
    for record in &file.embedded {
        let bytes = bytes_of(record)?;
        // STL files over 4 GiB need ZIP64
        let options = stl_options.large_file(bytes.len() as u64 >= u64::from(u32::MAX));
        zip.start_file(&record.entry, options).map_err(io::Error::other)?;
        zip.write_all(&bytes)?;
    }
    zip.finish().map_err(io::Error::other)
}

/// [`write_to`] in memory, with `stls` in the order of `file.embedded`.
pub fn write(file: &ProjectFile, app_version: &str, stls: &[Vec<u8>], compress: bool) -> io::Result<Vec<u8>> {
    if stls.len() != file.embedded.len() {
        return Err(io::Error::other("an embedded STL file has no bytes"));
    }
    let mut next = stls.iter();
    let out = write_to(Cursor::new(Vec::new()), file, app_version, compress, |_| {
        next.next().cloned().ok_or_else(|| io::Error::other("an embedded STL file has no bytes"))
    })?;
    Ok(out.into_inner())
}

/// Lists every STL file the tiles use in `file.embedded`, once each, under
/// a readable entry: `stl/<folder name>/<path inside the folder>`.
pub fn name_entries(file: &mut ProjectFile) {
    // Folder names as shown in the library, made unique ("Tiles (2)")
    let mut taken: HashSet<String> = HashSet::new();
    let mut folder_names: HashMap<usize, String> = HashMap::new();
    let mut entries: HashSet<String> = HashSet::new();
    let mut seen: HashSet<(usize, &str)> = HashSet::new();
    let mut embedded = Vec::new();
    for t in &file.tiles {
        if !seen.insert((t.folder, t.file.as_str())) {
            continue;
        }
        let folder = folder_names.entry(t.folder).or_insert_with(|| {
            let f = &file.folders[t.folder];
            let path = f.absolute.as_deref().unwrap_or(&f.path);
            let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
            unique(&mut taken, &clean(last, "Tiles"))
        });
        let inside: Vec<String> =
            t.file.split('/').filter(|p| !p.is_empty() && *p != "." && *p != "..").map(|p| clean(p, "tile")).collect();
        let entry = unique(&mut entries, &format!("stl/{folder}/{}", inside.join("/")));
        embedded.push(EmbeddedRecord { folder: t.folder, file: t.file.clone(), entry });
    }
    file.embedded = embedded;
}

/// A file or folder name that any system can unpack.
fn clean(name: &str, fallback: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c.is_control() || r#"\:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_owned();
    if cleaned.is_empty() { fallback.to_owned() } else { cleaned }
}

/// `name`, or `name (2)` and so on when it's taken; the number goes before
/// an extension.
fn unique(taken: &mut HashSet<String>, name: &str) -> String {
    let lower = |s: &str| s.to_lowercase();
    let mut candidate = name.to_owned();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.contains('/') => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    let mut n = 2;
    while taken.contains(&lower(&candidate)) {
        candidate = format!("{stem} ({n}){ext}");
        n += 1;
    }
    taken.insert(lower(&candidate));
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::Rotation;
    use crate::grid::GridSize;
    use crate::project::{FolderRecord, TileRecord};

    fn tile(folder: usize, file: &str) -> TileRecord {
        TileRecord { folder, file: file.into(), x: 0.0, y: 0.0, z: 0.0, rotation: Rotation::ZERO, level: 0, size: None }
    }

    fn project() -> ProjectFile {
        let folder = |path: &str| FolderRecord { path: path.into(), absolute: None, recursive: false };
        ProjectFile {
            version: project::FORMAT_VERSION,
            app_version: None,
            grid: GridSize { cols: 20, rows: 20 },
            folders: vec![folder("A/Tiles"), folder("B/Tiles"), folder("C:/Packs/Doors")],
            tiles: vec![
                tile(0, "floor.stl"),
                tile(0, "floor.stl"),
                tile(1, "floor.stl"),
                tile(2, "walls/door?.stl"),
                tile(2, "../escape.stl"),
            ],
            ground_image: None,
            embedded: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn entries_are_readable_unique_and_safe() {
        let mut file = project();
        name_entries(&mut file);
        let entries: Vec<&str> = file.embedded.iter().map(|e| e.entry.as_str()).collect();
        assert_eq!(
            entries,
            ["stl/Tiles/floor.stl", "stl/Tiles (2)/floor.stl", "stl/Doors/walls/door_.stl", "stl/Doors/escape.stl"],
            "one per file, folders told apart, nothing that leaves the folder"
        );
        assert_eq!(file.embedded[1].folder, 1);
        assert_eq!(file.embedded[3].file, "../escape.stl", "the record still names the tile's file");
    }

    #[test]
    fn a_container_round_trips_with_its_stl_files() {
        let mut file = project();
        name_entries(&mut file);
        let stls: Vec<Vec<u8>> = (0..file.embedded.len()).map(|i| vec![i as u8; 1000 + i]).collect();
        for compress in [true, false] {
            let bytes = write(&file, "0.5.0", &stls, compress).expect("write");
            assert!(is_container(&bytes));
            let read = read_container(Cursor::new(&bytes)).expect("read");
            assert_eq!(read.embedded, file.embedded);
            assert_eq!(read.tiles, file.tiles);
            assert!(read.warnings.is_empty(), "{:?}", read.warnings);
            let mut inside = Embedded::new(Cursor::new(&bytes)).expect("open");
            for (record, want) in file.embedded.iter().zip(&stls) {
                assert_eq!(&inside.read(&record.entry).expect("entry"), want);
            }
        }
        let compressed = write(&file, "0.5.0", &stls, true).expect("write").len();
        let stored = write(&file, "0.5.0", &stls, false).expect("write").len();
        assert!(compressed < stored);
    }

    #[test]
    fn damaged_containers_are_refused_and_missing_entries_dropped() {
        let mut file = project();
        name_entries(&mut file);
        let stls: Vec<Vec<u8>> = file.embedded.iter().map(|_| b"solid x\nendsolid x\n".to_vec()).collect();
        let bytes = write(&file, "0.5.0", &stls, true).expect("write");
        let cut = &bytes[..bytes.len() / 2];
        assert!(matches!(read_container(Cursor::new(cut)), Err(ProjectError::Damaged(_))));

        // A table record whose entry isn't there
        file.embedded.push(EmbeddedRecord { folder: 0, file: "gone.stl".into(), entry: "stl/Tiles/gone.stl".into() });
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(PROJECT_ENTRY, SimpleFileOptions::default()).expect("start");
        zip.write_all(project::write(&file, "0.5.0").as_bytes()).expect("json");
        let only_json = zip.finish().expect("finish").into_inner();
        let read = read_container(Cursor::new(only_json)).expect("read");
        assert!(read.embedded.is_empty());
        assert_eq!(read.warnings.len(), 1);
    }

    #[test]
    fn json_projects_still_read_from_a_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("old.mtp");
        std::fs::write(&path, br#"{"version": 3, "folders": [], "tiles": []}"#).expect("write");
        let file = read_file(&path).expect("read");
        assert_eq!(file.version, 3);
        assert!(file.embedded.is_empty());
    }
}
