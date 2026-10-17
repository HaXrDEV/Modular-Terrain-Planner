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

/// A file's size and CRC32, as a ZIP stores them for each entry, so a file
/// on disk can be compared with its copy inside without unpacking the copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fingerprint {
    pub size: u64,
    pub crc32: u32,
}

impl Fingerprint {
    pub fn of(bytes: &[u8]) -> Self {
        Self { size: bytes.len() as u64, crc32: crc32fast::hash(bytes) }
    }

    /// Reads a file through once.
    pub fn of_file(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut hasher = crc32fast::Hasher::new();
        let mut buf = vec![0; 1 << 16];
        let mut size = 0;
        loop {
            let n = match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            hasher.update(&buf[..n]);
            size += n as u64;
        }
        Ok(Self { size, crc32: hasher.finalize() })
    }

    /// Whether the file at `path` has these contents. A different size
    /// answers without reading it.
    pub fn matches_file(&self, path: &Path) -> io::Result<bool> {
        if std::fs::metadata(path)?.len() != self.size {
            return Ok(false);
        }
        Ok(Self::of_file(path)? == *self)
    }
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

    /// One entry's bytes. `zip` checks them against the entry's CRC32, so a
    /// damaged entry is an error rather than a wrong file.
    pub fn read(&mut self, entry: &str) -> io::Result<Vec<u8>> {
        let mut file = self.zip.by_name(entry).map_err(io::Error::other)?;
        let mut out = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0));
        file.read_to_end(&mut out)?;
        Ok(out)
    }

    /// An entry's size and CRC32, from the container's directory.
    pub fn fingerprint(&self, entry: &str) -> Option<Fingerprint> {
        let data = self.zip.by_index_data(self.zip.index_for_name(entry)?).ok()?;
        Some(Fingerprint { size: data.size(), crc32: data.crc32() })
    }

    /// Whether an entry is compressed rather than stored as it is.
    pub fn is_compressed(&self, entry: &str) -> Option<bool> {
        let data = self.zip.by_index_data(self.zip.index_for_name(entry)?).ok()?;
        Some(data.compression() != CompressionMethod::Stored)
    }
}

/// Writes a container one STL file at a time, so they're never all in
/// memory. The project goes in last, by [`Writer::finish`].
pub struct Writer<W: Write + Seek> {
    zip: ZipWriter<W>,
    stl: SimpleFileOptions,
}

impl<W: Write + Seek> Writer<W> {
    /// `compress` trades time for size (autosaves store new STL files as
    /// they are).
    pub fn new(out: W, compress: bool) -> Self {
        let method = if compress { CompressionMethod::Deflated } else { CompressionMethod::Stored };
        Self { zip: ZipWriter::new(out), stl: SimpleFileOptions::default().compression_method(method) }
    }

    /// Adds an STL file.
    pub fn add(&mut self, entry: &str, bytes: &[u8]) -> io::Result<()> {
        // Files over 4 GiB need ZIP64
        let options = self.stl.large_file(bytes.len() as u64 >= u64::from(u32::MAX));
        self.zip.start_file(entry, options).map_err(io::Error::other)?;
        self.zip.write_all(bytes)
    }

    /// Copies an entry of another container as it is stored there, as
    /// `as_entry`: no unpacking and packing again, so an unchanged file
    /// costs no more than copying its bytes.
    pub fn copy<R: Read + Seek>(&mut self, from: &mut Embedded<R>, entry: &str, as_entry: &str) -> io::Result<()> {
        let index = from.zip.index_for_name(entry).ok_or_else(|| io::Error::other(format!("{entry} isn't there")))?;
        let file = from.zip.by_index_raw(index).map_err(io::Error::other)?;
        self.zip.raw_copy_file_rename(file, as_entry).map_err(io::Error::other)
    }

    /// Adds the project, listing what was added, and finishes the container.
    pub fn finish(mut self, file: &ProjectFile, app_version: &str) -> io::Result<W> {
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        self.zip.start_file(PROJECT_ENTRY, deflated).map_err(io::Error::other)?;
        self.zip.write_all(project::write(file, app_version).as_bytes())?;
        self.zip.finish().map_err(io::Error::other)
    }
}

/// A whole container in memory, with `stls` in the order of `file.embedded`.
pub fn write(file: &ProjectFile, app_version: &str, stls: &[Vec<u8>], compress: bool) -> io::Result<Vec<u8>> {
    if stls.len() != file.embedded.len() {
        return Err(io::Error::other("an embedded STL file has no bytes"));
    }
    let mut out = Writer::new(Cursor::new(Vec::new()), compress);
    for (record, bytes) in file.embedded.iter().zip(stls) {
        out.add(&record.entry, bytes)?;
    }
    Ok(out.finish(file, app_version)?.into_inner())
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
    fn copied_entries_keep_their_bytes_and_crc() {
        let mut file = project();
        name_entries(&mut file);
        let stls: Vec<Vec<u8>> = (0..file.embedded.len())
            .map(|i| {
                format!(
                    "solid {i}
endsolid
"
                )
                .repeat(200)
                .into_bytes()
            })
            .collect();
        let first = write(&file, "0.5.0", &stls, true).expect("write");
        let mut from = Embedded::new(Cursor::new(&first)).expect("open");

        // As an autosave does: an unchanged file copied as it is, a new one stored
        let mut out = Writer::new(Cursor::new(Vec::new()), false);
        let renamed = "stl/Tiles/renamed.stl";
        out.copy(&mut from, &file.embedded[0].entry, renamed).expect("copy");
        out.add(&file.embedded[1].entry, &stls[1]).expect("add");
        let mut listed = file.clone();
        listed.embedded.truncate(2);
        listed.embedded[0].entry = renamed.into();
        let second = out.finish(&listed, "0.5.0").expect("finish").into_inner();

        let mut inside = Embedded::new(Cursor::new(&second)).expect("open");
        assert_eq!(inside.read(renamed).expect("copied"), stls[0]);
        assert_eq!(inside.fingerprint(renamed), Some(Fingerprint::of(&stls[0])));
        assert_eq!(inside.fingerprint(renamed), from.fingerprint(&file.embedded[0].entry));
        assert_eq!(inside.is_compressed(renamed), Some(true), "still compressed, as it was");
        assert_eq!(inside.is_compressed(&file.embedded[1].entry), Some(false));
        let read = read_container(Cursor::new(&second)).expect("read");
        assert_eq!(read.embedded, listed.embedded);
        assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    }

    #[test]
    fn a_damaged_entry_is_an_error_not_a_wrong_file() {
        let mut file = project();
        name_entries(&mut file);
        file.embedded.truncate(1);
        let stl = b"solid floor\nendsolid floor\n".repeat(20);
        let mut bytes = write(&file, "0.5.0", std::slice::from_ref(&stl), false).expect("write");
        let at = bytes.windows(stl.len()).position(|w| w == stl.as_slice()).expect("stored as it is");
        bytes[at + 6] ^= 0x20;
        let mut inside = Embedded::new(Cursor::new(&bytes)).expect("open");
        assert_eq!(inside.fingerprint(&file.embedded[0].entry), Some(Fingerprint::of(&stl)));
        let error = inside.read(&file.embedded[0].entry).expect_err("the CRC32 doesn't match");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn files_on_disk_compare_by_size_then_crc() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("floor.stl");
        std::fs::write(&path, b"solid floor").expect("write");
        let fp = Fingerprint::of(b"solid floor");
        assert_eq!(Fingerprint::of_file(&path).expect("read"), fp);
        assert!(fp.matches_file(&path).expect("read"));
        std::fs::write(&path, b"solid FLOOR").expect("same size, other bytes");
        assert!(!fp.matches_file(&path).expect("read"));
        std::fs::write(&path, b"solid floor 2").expect("other size");
        assert!(!fp.matches_file(&path).expect("read"));
        assert!(fp.matches_file(&dir.path().join("gone.stl")).is_err());
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
