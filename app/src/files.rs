//! Opening and saving projects, autosave and recovery, battle map images.
//! Projects are saved as containers that carry the STL files their tiles
//! use (format 6); every older format still opens.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mtp_core::container::{self, Fingerprint};
use mtp_core::document::{DocState, GroundImage};
use mtp_core::persist::{write_atomic, write_atomic_with};
use mtp_core::project::{self, LibraryFolder, ProjectFile};
use mtp_core::tile::TileKey;

use crate::embedded::{self, Archives, EmbeddedStore, Stored};
use crate::library::{EmbeddedTile, Library};
use crate::settings::VERSION;

/// A project being opened, waiting for missing folders to be found.
pub struct PendingOpen {
    pub path: PathBuf,
    pub file: ProjectFile,
    /// Per folder: where it is now, if found.
    pub found: Vec<Option<String>>,
    /// Folders to ask about, by index.
    pub missing: Vec<usize>,
}

impl PendingOpen {
    pub fn project_dir(&self) -> String {
        dir_of(&self.path)
    }
}

pub fn dir_of(path: &Path) -> String {
    TileKey::from_path(path.parent().unwrap_or(Path::new("."))).as_str().to_owned()
}

/// Reads a project and looks for its folders. A failed read changes
/// nothing, so the open project stays as it was.
pub fn read_project(path: &Path) -> Result<PendingOpen, String> {
    let file = container::read_file(path).map_err(|e| format!("{} can't be opened: {e}", path.display()))?;
    let dir = dir_of(path);
    let found: Vec<Option<String>> =
        file.folders.iter().map(|f| project::locate(&dir, f, |p| Path::new(p).is_dir())).collect();
    let missing = found.iter().enumerate().filter(|(_, f)| f.is_none()).map(|(i, _)| i).collect();
    Ok(PendingOpen { path: path.to_path_buf(), file, found, missing })
}

/// The document for a project whose folders have been looked for.
pub fn finish_open(pending: &PendingOpen) -> DocState {
    project::to_state(&pending.file, &pending.project_dir(), &pending.found, image_path(pending))
}

/// The battle map's path in the opened document: the file on disk when it's
/// there (next to the project first), else where it was when saved.
fn image_path(pending: &PendingOpen) -> Option<String> {
    let dir = pending.project_dir();
    let img = pending.file.ground_image.as_ref()?;
    let saved = img.absolute.clone().unwrap_or_else(|| project::join(&dir, &img.path));
    let candidates = [project::join(&dir, &img.path), saved.clone()];
    Some(candidates.into_iter().find(|p| Path::new(p).is_file()).unwrap_or(saved))
}

/// Folders a document needs, in the order to load them. Folders that
/// weren't found are listed too, at their saved place, so they show as not
/// found in the library and are saved back as they were, unless the project
/// carries every tile in them: then they're only clutter.
pub fn folders_to_load(pending: &PendingOpen) -> Vec<(String, bool)> {
    let dir = pending.project_dir();
    pending
        .file
        .folders
        .iter()
        .zip(&pending.found)
        .enumerate()
        .filter(|(i, (_, found))| found.is_some() || !carried(pending, *i))
        .map(|(_, (f, found))| (found.clone().unwrap_or_else(|| project::saved_location(&dir, f)), f.recursive))
        .collect()
}

/// Whether the project carries the STL file of every tile in folder `i`.
fn carried(pending: &PendingOpen, i: usize) -> bool {
    let mut tiles = pending.file.tiles.iter().filter(|t| t.folder == i).peekable();
    tiles.peek().is_some() && tiles.all(|t| pending.file.embedded.iter().any(|e| e.folder == i && e.file == t.file))
}

/// The STL files and battle map inside an opened project, by the tile key
/// or image path each stands in for, read from the project file when needed.
pub fn embedded_store(pending: &PendingOpen) -> EmbeddedStore {
    let entries = project::embedded_keys(&pending.file, &pending.project_dir(), &pending.found);
    let image = pending.file.ground_image.as_ref().and_then(|img| img.entry.clone());
    EmbeddedStore::in_archive(&pending.path, entries.into_iter().map(|(key, e)| (key, e.entry.clone())))
        .with_image(&pending.path, image_path(pending).zip(image))
}

/// Folders that weren't found and hold tiles whose STL file isn't inside
/// the project either, with how many such tiles each has.
pub fn uncovered(pending: &PendingOpen) -> Vec<(usize, usize)> {
    let inside: HashSet<(usize, &str)> = pending.file.embedded.iter().map(|e| (e.folder, e.file.as_str())).collect();
    pending
        .missing
        .iter()
        .filter(|&&i| pending.found.get(i).is_some_and(Option::is_none))
        .map(|&i| {
            let tiles = pending.file.tiles.iter().filter(|t| t.folder == i && !inside.contains(&(i, t.file.as_str())));
            (i, tiles.count())
        })
        .filter(|&(_, n)| n > 0)
        .collect()
}

/// What the library's "In this project" section shows for `store`.
pub fn embedded_tiles(pending: &PendingOpen, store: &EmbeddedStore) -> Vec<EmbeddedTile> {
    let dir = pending.project_dir();
    let mut tiles = Vec::new();
    for (key, record) in project::embedded_keys(&pending.file, &dir, &pending.found) {
        let Some(stored) = store.get(&key) else { continue };
        let folder = &pending.file.folders[record.folder];
        let saved = folder.absolute.as_deref().unwrap_or(&folder.path);
        let pack = saved.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_owned();
        tiles.push(EmbeddedTile { key, pack, relative: record.file.clone(), stored: stored.clone() });
    }
    tiles
}

/// What a save put inside the project.
#[derive(Debug, Default)]
pub struct Saved {
    /// The STL files inside it now, by the tile key each stands in for.
    pub embedded: Vec<(TileKey, String)>,
    /// STL files that are neither on this computer nor inside the project
    /// being replaced, so their tiles are saved without them.
    pub missing: Vec<TileKey>,
    /// The battle map's path in the document and its entry, if it's inside.
    pub image: Option<(String, String)>,
}

/// What a save writes, worked out on the UI thread so that the writing can
/// happen on another: the project file, and where each STL file inside it
/// comes from.
pub struct SavePlan {
    file: ProjectFile,
    /// One per record of `file.embedded`.
    sources: Vec<Source>,
    /// The battle map, when it goes inside.
    image: Option<Source>,
    saved: Saved,
}

impl SavePlan {
    /// How many STL files go inside.
    pub fn len(&self) -> usize {
        self.sources.len()
    }
}

/// Where one STL file going inside a project comes from.
struct Source {
    /// The tile key, which is also the file on disk.
    key: TileKey,
    /// Its copy inside the project being replaced, if there is one.
    stored: Option<Stored>,
    /// Keep the copy even though the file on disk is different.
    pinned: bool,
}

/// Plans saving `state` to `path`, with an entry for every STL file that
/// can be carried: it's on disk, or inside the project being replaced
/// (`store`).
pub fn plan_save(state: &DocState, library: &Library, store: &EmbeddedStore, path: &Path) -> SavePlan {
    // Folders that aren't there right now are written back too, so an
    // unplugged drive doesn't change the project
    let open: Vec<LibraryFolder> = library
        .folders()
        .iter()
        .filter(|f| !f.embedded)
        .map(|f| LibraryFolder { path: f.path.clone(), recursive: f.recursive })
        .collect();
    let mut file = project::from_state(state, &open, &dir_of(path));
    container::name_entries(&mut file);
    // from_state writes the tiles in the document's order
    let mut key_of: HashMap<(usize, &str), &TileKey> = HashMap::new();
    for (record, tile) in file.tiles.iter().zip(&state.tiles) {
        key_of.entry((record.folder, record.file.as_str())).or_insert(&tile.key);
    }
    let mut sources = Vec::new();
    let mut saved = Saved::default();
    let mut kept = Vec::new();
    for record in &file.embedded {
        let Some(&key) = key_of.get(&(record.folder, record.file.as_str())) else { continue };
        if embedded::available(key, store) {
            sources.push(Source { key: key.clone(), stored: store.get(key).cloned(), pinned: store.is_pinned(key) });
            saved.embedded.push((key.clone(), record.entry.clone()));
            kept.push(record.clone());
        } else {
            saved.missing.push(key.clone());
        }
    }
    file.embedded = kept;
    // The battle map goes inside too, when it's on disk or inside already
    let image_path = state.ground_image.as_ref().map(|img| img.path.as_str());
    let image = match (image_path, &mut file.ground_image) {
        (Some(path), Some(record)) if Path::new(path).is_file() || store.image(path).is_some() => {
            saved.image = record.entry.clone().map(|entry| (path.to_owned(), entry));
            Some(Source { key: TileKey::new(path), stored: store.image(path).cloned(), pinned: false })
        }
        (_, Some(record)) => {
            record.entry = None;
            None
        }
        _ => None,
    };
    SavePlan { file, sources, image, saved }
}

/// Writes a planned save atomically, keeping the previous file as `.bak`
/// with `keep_backup`. New and changed STL files are compressed with
/// `compress`; unchanged ones are copied from the project being replaced as
/// they are, which is what makes saving again quick. Identical files from
/// two folders share one entry. `progress` hears how many STL files are done.
pub fn write_save(
    plan: SavePlan,
    path: &Path,
    keep_backup: bool,
    compress: bool,
    progress: &dyn Fn(usize),
) -> Result<Saved, String> {
    let SavePlan { mut file, sources, image, mut saved } = plan;
    write_atomic_with(path, keep_backup, |f| {
        let mut archives = Archives::default();
        let mut out = container::Writer::new(BufWriter::new(f), compress);
        // Entries written so far, by contents
        let mut written: HashMap<Fingerprint, Vec<Written>> = HashMap::new();
        for (i, source) in sources.iter().enumerate() {
            let fail = |e: io::Error| io::Error::other(format!("{}: {e}", source.key.as_str()));
            let contents = source.contents(&mut archives).map_err(fail)?;
            let fingerprint = contents.fingerprint();
            let mut twin = None;
            for (j, copied) in written.get(&fingerprint).into_iter().flatten() {
                let earlier = match copied {
                    Some((archive, inside)) => archives.open(archive).and_then(|a| a.read(inside)),
                    None => sources[*j].read(),
                };
                // The same size and CRC32 is almost surely the same file; the bytes make sure
                if contents.same_as(&earlier.map_err(fail)?, &mut archives).map_err(fail)? {
                    twin = Some(*j);
                    break;
                }
            }
            if let Some(j) = twin {
                file.embedded[i].entry = file.embedded[j].entry.clone();
            } else {
                let entry = &file.embedded[i].entry;
                let copied = match contents {
                    Contents::Copy { archive, inside, .. } => {
                        out.copy(archives.open(&archive)?, &inside, entry).map_err(fail)?;
                        Some((archive, inside))
                    }
                    Contents::Bytes(bytes) => {
                        out.add(entry, &bytes).map_err(fail)?;
                        None
                    }
                };
                written.entry(fingerprint).or_default().push((i, copied));
            }
            progress(i + 1);
        }
        if let (Some(source), Some(entry)) = (&image, file.ground_image.as_ref().and_then(|i| i.entry.as_ref())) {
            let fail = |e: io::Error| io::Error::other(format!("{}: {e}", source.key.as_str()));
            match source.contents(&mut archives).map_err(fail)? {
                Contents::Copy { archive, inside, .. } => out.copy(archives.open(&archive)?, &inside, entry),
                Contents::Bytes(bytes) => out.add_image(entry, &bytes),
            }
            .map_err(fail)?;
        }
        let out = out.finish(&file, VERSION)?;
        // Closed before the new file takes the place of the one they read
        drop(archives);
        out.into_inner().map_err(io::IntoInnerError::into_error)?.flush()
    })
    .map_err(|e| format!("{} couldn't be saved: {e}", path.display()))?;
    saved.embedded = sources.iter().zip(&file.embedded).map(|(s, r)| (s.key.clone(), r.entry.clone())).collect();
    Ok(saved)
}

/// A record whose entry was written, and the entry of the project being
/// replaced it was copied from, if it was.
type Written = (usize, Option<(PathBuf, String)>);

/// What goes into one entry.
enum Contents {
    /// An entry of the project being replaced, copied as it is.
    Copy { archive: PathBuf, inside: String, fingerprint: Fingerprint },
    /// The file's bytes.
    Bytes(Vec<u8>),
}

impl Contents {
    fn fingerprint(&self) -> Fingerprint {
        match self {
            Self::Copy { fingerprint, .. } => *fingerprint,
            Self::Bytes(bytes) => Fingerprint::of(bytes),
        }
    }

    fn same_as(&self, bytes: &[u8], archives: &mut Archives) -> io::Result<bool> {
        Ok(match self {
            Self::Copy { archive, inside, .. } => archives.open(archive)?.read(inside)? == bytes,
            Self::Bytes(mine) => mine == bytes,
        })
    }
}

impl Source {
    /// The copy in the project being replaced when the file on disk is the
    /// same or isn't there (or the copy is pinned), else the file's bytes.
    fn contents(&self, archives: &mut Archives) -> io::Result<Contents> {
        let disk = Path::new(self.key.as_str());
        if let Some(Stored::Archive { path, entry }) = &self.stored
            && let Ok(archive) = archives.open(path)
            && let Some(fingerprint) = archive.fingerprint(entry)
            // A file on disk that can't be read counts as not there
            && (self.pinned || fingerprint.matches_file(disk).unwrap_or(true))
        {
            return Ok(Contents::Copy { archive: path.clone(), inside: entry.clone(), fingerprint });
        }
        self.read().map(Contents::Bytes)
    }

    /// The file's bytes: from disk when it's there, else the copy (always
    /// the copy when it's pinned).
    fn read(&self) -> io::Result<Vec<u8>> {
        if self.pinned
            && let Some(stored) = &self.stored
        {
            return stored.read();
        }
        match std::fs::read(self.key.as_str()) {
            Ok(bytes) => Ok(bytes),
            Err(e) => match &self.stored {
                Some(stored) => stored.read(),
                None => Err(e),
            },
        }
    }
}

/// Adds the extension when the file dialog didn't.
pub fn with_extension(mut path: PathBuf) -> PathBuf {
    let has = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(project::EXTENSION));
    if !has {
        path.as_mut_os_string().push(format!(".{}", project::EXTENSION));
    }
    path
}

/// An autosave next to its note of which file it belongs to.
#[derive(Debug, Clone)]
pub struct Autosave {
    pub file: PathBuf,
    pub original: Option<PathBuf>,
    pub saved_at: SystemTime,
}

fn note_path(file: &Path) -> PathBuf {
    file.with_extension("origin")
}

/// The file this window autosaves `original` (or an untitled project) to.
/// Each session writes its own (`session` tells them apart), so one window
/// never overwrites or deletes another's.
pub fn autosave_file(dir: &Path, session: &str, original: Option<&Path>) -> PathBuf {
    let name =
        original.and_then(|p| p.file_stem()).map_or_else(|| "Untitled".into(), |s| s.to_string_lossy().into_owned());
    dir.join(format!("{name}-{session}.autosave.mtp"))
}

/// Writes a planned autosave and its note of which project it belongs to.
/// Plan it against `original`, so recovering keeps the paths. New STL
/// files are stored, not compressed, so it's quick.
pub fn write_autosave(
    plan: SavePlan,
    file: &Path,
    original: Option<&Path>,
    progress: &dyn Fn(usize),
) -> Result<Saved, String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let saved = write_save(plan, file, false, false, progress)?;
    let note = original.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    write_atomic(&note_path(file), note.as_bytes(), false).map_err(|e| e.to_string())?;
    Ok(saved)
}

pub fn autosaves(dir: &Path) -> Vec<Autosave> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Autosave> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".autosave.mtp"))
        .map(|file| {
            let original = std::fs::read_to_string(note_path(&file)).ok().filter(|s| !s.is_empty()).map(PathBuf::from);
            let saved_at = std::fs::metadata(&file).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            Autosave { file, original, saved_at }
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.saved_at));
    out
}

pub fn discard_autosave(a: &Autosave) {
    discard_autosave_file(&a.file);
}

pub fn discard_autosave_file(file: &Path) {
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(note_path(file));
}

pub fn discard_autosaves(dir: &Path) {
    for a in autosaves(dir) {
        discard_autosave(&a);
    }
}

/// Writes one copy of every STL file the layout uses into a new folder
/// inside `dir`, named after the project, so nothing is ever overwritten.
/// Files are named as in the print list. Returns the folder, how many files
/// were written, and the tiles whose STL file is nowhere to be found.
pub fn export_stls(
    state: &DocState,
    store: &EmbeddedStore,
    names: &HashMap<TileKey, String>,
    dir: &Path,
    title: &str,
) -> Result<(PathBuf, usize, Vec<TileKey>), String> {
    let base = clean_name(&format!("{title} STL files"));
    let mut folder = dir.join(&base);
    let mut n = 2;
    while folder.exists() {
        folder = dir.join(format!("{base} ({n})"));
        n += 1;
    }
    std::fs::create_dir_all(&folder).map_err(|e| format!("{} couldn't be made: {e}", folder.display()))?;
    let mut seen = HashSet::new();
    let mut taken = HashSet::new();
    let (mut written, mut missing) = (0, Vec::new());
    for t in &state.tiles {
        if !seen.insert(t.key.clone()) {
            continue;
        }
        let Ok(bytes) = embedded::bytes(&t.key, store) else {
            missing.push(t.key.clone());
            continue;
        };
        let stem = clean_name(names.get(&t.key).map_or(t.key.stem(), String::as_str));
        let mut name = format!("{stem}.stl");
        let mut n = 2;
        while !taken.insert(name.to_lowercase()) {
            name = format!("{stem} ({n}).stl");
            n += 1;
        }
        let path = folder.join(&name);
        std::fs::write(&path, bytes).map_err(|e| format!("{} couldn't be written: {e}", path.display()))?;
        written += 1;
    }
    Ok((folder, written, missing))
}

/// A file name any system accepts.
fn clean_name(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_owned();
    if cleaned.is_empty() { "tile".to_owned() } else { cleaned }
}

/// A battle map decoded and scaled to what the GPU takes.
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn decode_image(path: &Path, max_side: u32) -> Result<DecodedImage, String> {
    let img = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| format!("{} can't be read: {e}", path.display()))?
        .decode()
        .map_err(|e| format!("{} isn't an image this app can read: {e}", path.display()))?;
    Ok(scaled(img, max_side, path))
}

/// The battle map at `path`: the file on disk when it's there, else its
/// copy inside the project.
pub fn load_image(path: &str, store: &EmbeddedStore, max_side: u32) -> Result<DecodedImage, String> {
    let on_disk = Path::new(path);
    let Some(stored) = store.image(path).filter(|_| !on_disk.is_file()) else { return decode_image(on_disk, max_side) };
    let bytes = stored.read().map_err(|e| format!("The copy of {path} inside the project can't be read: {e}"))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| format!("The copy of {path} inside the project isn't an image this app can read: {e}"))?;
    Ok(scaled(img, max_side, on_disk))
}

/// An image scaled down to fit `max_side`, as RGBA.
fn scaled(img: image::DynamicImage, max_side: u32, path: &Path) -> DecodedImage {
    let img = if img.width() > max_side || img.height() > max_side {
        tracing::info!("scaling {} down to {max_side} px", path.display());
        img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    DecodedImage { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
}

/// Where a new battle map goes: filling the grid, keeping its proportions.
pub fn fit_to_grid(cols: u32, rows: u32, width: u32, height: u32) -> [f32; 4] {
    let (gw, gh) = (cols as f32, rows as f32);
    let aspect = width as f32 / height.max(1) as f32;
    let (w, h) = if gw / gh > aspect { (gh * aspect, gh) } else { (gw, gw / aspect) };
    [(gw - w) / 2.0, (gh - h) / 2.0, w, h]
}

pub fn ground_image(path: &Path, rect: [f32; 4]) -> GroundImage {
    GroundImage { path: TileKey::from_path(path).as_str().to_owned(), rect }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtp_core::container::Embedded;
    use mtp_core::document::Document;
    use mtp_core::footprint::Rotation;
    use mtp_core::grid::GridSize;
    use mtp_core::tile::{Placed, PlacedId};
    use std::sync::Arc;

    const FLOOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/stl/good/floor_2x2_mm.stl");
    const WALL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/stl/good/wall_inch.stl");

    /// Saves as the app does, but on this thread.
    fn save_project(doc: &Document, library: &Library, store: &EmbeddedStore, path: &Path) -> Result<Saved, String> {
        write_save(plan_save(doc.state(), library, store, path), path, true, true, &|_| {})
    }

    fn autosave(
        dir: &Path,
        session: &str,
        doc: &Document,
        library: &Library,
        store: &EmbeddedStore,
        original: Option<&Path>,
    ) -> Result<PathBuf, String> {
        let file = autosave_file(dir, session, original);
        let plan = plan_save(doc.state(), library, store, original.unwrap_or(&file));
        write_autosave(plan, &file, original, &|_| {})?;
        Ok(file)
    }

    /// Waits for the library to load what it was given.
    fn settle(library: &mut Library) {
        let start = std::time::Instant::now();
        loop {
            library.poll();
            if library.progress().is_none() {
                return;
            }
            assert!(start.elapsed().as_secs() < 30, "loading took too long");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn placed(id: u64, key: &TileKey, x: f32) -> Placed {
        Placed {
            id: PlacedId(id),
            key: key.clone(),
            size: mtp_core::units::TileSize { w: 2, h: 2, gz: 0.5 },
            size_known: true,
            x,
            y: 2.0,
            z: 0.0,
            rotation: Rotation::ZERO,
            level: 0,
        }
    }

    #[test]
    fn a_project_carries_its_stl_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let tiles = dir.path().join("Tiles");
        std::fs::create_dir(&tiles).expect("folder");
        let stl = tiles.join("floor_2x2_mm.stl");
        std::fs::copy(FLOOR, &stl).expect("copy");
        let original = std::fs::read(&stl).expect("read");

        let mut library = Library::new(None, 0, Arc::new(|| {}));
        library.add_folder(&tiles, false);
        settle(&mut library);
        let key = TileKey::from_path(&stl);
        let size = library.tile(&key).expect("loaded").size;
        let mut state = DocState::empty(GridSize { cols: 20, rows: 20 });
        state.tiles = vec![placed(1, &key, 2.0), placed(2, &key, 6.0)];
        let path = dir.path().join("castle.mtp");
        let saved = save_project(&Document::new(state), &library, &EmbeddedStore::default(), &path).expect("save");
        assert_eq!((saved.embedded.len(), saved.missing.len()), (1, 0), "one copy however many tiles use it");

        // The tiles' folder is gone, as on another computer
        std::fs::remove_dir_all(&tiles).expect("remove");
        let pending = read_project(&path).expect("reopen");
        assert_eq!(pending.missing.len(), 1);
        assert!(uncovered(&pending).is_empty(), "nothing to ask: the project carries it");
        assert!(folders_to_load(&pending).is_empty(), "nor to list as not found");
        let store = embedded_store(&pending);
        let reopened = finish_open(&pending);
        assert_eq!(reopened.tiles.len(), 2);
        assert_eq!(reopened.tiles[0].key, key);
        assert_eq!(embedded::bytes(&key, &store).expect("inside"), original);

        // The library's "In this project" section loads it instead
        let mut elsewhere = Library::new(None, 0, Arc::new(|| {}));
        elsewhere.add_embedded(embedded_tiles(&pending, &store));
        settle(&mut elsewhere);
        let tile = elsewhere.tile(&key).expect("loaded from inside the project");
        assert_eq!((tile.size, tile.pack.as_deref()), (size, Some("Tiles")));

        // Saving over it again, the folder still gone, keeps the copy
        let again = save_project(&Document::new(reopened), &elsewhere, &store, &path).expect("save again");
        assert_eq!((again.embedded.len(), again.missing.len()), (1, 0));
        let store = embedded_store(&read_project(&path).expect("reopen"));
        assert_eq!(embedded::bytes(&key, &store).expect("still inside"), original);
    }

    #[test]
    fn unchanged_files_are_copied_from_the_project_as_they_are() {
        let dir = tempfile::tempdir().expect("temp dir");
        let tiles = dir.path().join("Tiles");
        std::fs::create_dir(&tiles).expect("folder");
        let stl = tiles.join("floor.stl");
        std::fs::copy(FLOOR, &stl).expect("copy");
        let key = TileKey::from_path(&stl);
        let mut state = DocState::empty(GridSize { cols: 20, rows: 20 });
        state.tiles = vec![placed(1, &key, 2.0)];
        let doc = Document::new(state);
        let library = Library::new(None, 0, Arc::new(|| {}));
        let path = dir.path().join("castle.mtp");
        save_project(&doc, &library, &EmbeddedStore::default(), &path).expect("save");
        let store = embedded_store(&read_project(&path).expect("reopen"));
        let inside = |file: &Path| {
            let mut archive = Embedded::open(file).expect("open");
            let entry = "stl/Tiles/floor.stl";
            (archive.is_compressed(entry), archive.read(entry).expect("entry"))
        };

        // An autosave stores new files, so a compressed one was copied as it was
        let progress = std::cell::Cell::new(0);
        let file = autosave_file(dir.path(), "a", Some(&path));
        let plan = plan_save(doc.state(), &library, &store, &path);
        assert_eq!(plan.len(), 1);
        write_autosave(plan, &file, Some(&path), &|n| progress.set(n)).expect("autosave");
        assert_eq!(progress.get(), 1);
        assert_eq!(inside(&file), (Some(true), std::fs::read(FLOOR).expect("fixture")));

        // A file changed on disk is read again
        std::fs::copy(WALL, &stl).expect("change it");
        autosave(dir.path(), "a", &doc, &library, &store, Some(&path)).expect("autosave");
        assert_eq!(inside(&file), (Some(false), std::fs::read(WALL).expect("fixture")));

        // With the file gone from disk, saving over the project copies from it
        std::fs::remove_file(&stl).expect("remove");
        save_project(&doc, &library, &store, &path).expect("save over the project it copies from");
        assert_eq!(inside(&path), (Some(true), std::fs::read(FLOOR).expect("fixture")));
    }

    #[test]
    fn saving_keeps_a_pinned_copy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let tiles = dir.path().join("Tiles");
        std::fs::create_dir(&tiles).expect("folder");
        let stl = tiles.join("floor.stl");
        std::fs::copy(FLOOR, &stl).expect("copy");
        let key = TileKey::from_path(&stl);
        let mut state = DocState::empty(GridSize { cols: 20, rows: 20 });
        state.tiles = vec![placed(1, &key, 2.0)];
        let doc = Document::new(state);
        let library = Library::new(None, 0, Arc::new(|| {}));
        let path = dir.path().join("castle.mtp");
        save_project(&doc, &library, &EmbeddedStore::default(), &path).expect("save");
        std::fs::copy(WALL, &stl).expect("the file changes on disk");

        let mut store = embedded_store(&read_project(&path).expect("reopen"));
        store.pin(std::slice::from_ref(&key));
        assert_eq!(embedded::bytes(&key, &store).expect("copy"), std::fs::read(FLOOR).expect("fixture"));
        save_project(&doc, &library, &store, &path).expect("save");
        let saved = embedded_store(&read_project(&path).expect("reopen")).with_pins_of(&store);
        assert_eq!(embedded::bytes(&key, &saved).expect("kept"), std::fs::read(FLOOR).expect("fixture"));

        // Not pinned, the file on disk goes in
        save_project(&doc, &library, &EmbeddedStore::default(), &path).expect("save");
        let store = embedded_store(&read_project(&path).expect("reopen"));
        assert_eq!(store.get(&key).expect("inside").read().expect("read"), std::fs::read(WALL).expect("fixture"));
    }

    #[test]
    fn the_battle_map_travels_with_the_project() {
        let dir = tempfile::tempdir().expect("temp dir");
        let maps = dir.path().join("Maps");
        std::fs::create_dir(&maps).expect("folder");
        let png = maps.join("cave.png");
        image::RgbaImage::from_pixel(4, 2, image::Rgba([200, 100, 50, 255])).save(&png).expect("png");
        let original = std::fs::read(&png).expect("read");
        let mut state = DocState::empty(GridSize { cols: 20, rows: 20 });
        state.ground_image = Some(ground_image(&png, [0.0, 0.0, 20.0, 10.0]));
        let library = Library::new(None, 0, Arc::new(|| {}));
        let path = dir.path().join("castle.mtp");
        let saved = save_project(&Document::new(state), &library, &EmbeddedStore::default(), &path).expect("save");
        assert_eq!(saved.image.as_ref().map(|(_, e)| e.as_str()), Some("images/cave.png"));
        let mut inside = Embedded::open(&path).expect("open");
        assert_eq!(inside.is_compressed("images/cave.png"), Some(false), "stored as it is");
        assert_eq!(inside.read("images/cave.png").expect("image"), original);

        // On another computer the copy is shown, and saving keeps it
        std::fs::remove_dir_all(&maps).expect("remove");
        let pending = read_project(&path).expect("reopen");
        let state = finish_open(&pending);
        let shown = state.ground_image.clone().expect("kept");
        let store = embedded_store(&pending);
        let decoded = load_image(&shown.path, &store, 4096).expect("from the project");
        assert_eq!((decoded.width, decoded.height), (4, 2));
        assert_eq!(&decoded.rgba[..4], &[200, 100, 50, 255]);
        assert!(load_image(&shown.path, &EmbeddedStore::default(), 4096).is_err(), "not on disk");
        let again = save_project(&Document::new(state), &library, &store, &path).expect("save again");
        assert_eq!(again.image, Some((shown.path.clone(), "images/cave.png".into())));
        let store = embedded_store(&read_project(&path).expect("reopen"));
        assert_eq!(store.image(&shown.path).expect("inside").read().expect("read"), original);
    }

    #[test]
    fn identical_files_share_one_entry() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut keys = Vec::new();
        for (pack, source) in [("A", FLOOR), ("B", FLOOR), ("C", WALL)] {
            let tiles = dir.path().join(pack).join("Tiles");
            std::fs::create_dir_all(&tiles).expect("folder");
            std::fs::copy(source, tiles.join("floor.stl")).expect("copy");
            keys.push(TileKey::from_path(&tiles.join("floor.stl")));
        }
        let mut state = DocState::empty(GridSize { cols: 20, rows: 20 });
        state.tiles = keys.iter().enumerate().map(|(i, k)| placed(i as u64 + 1, k, i as f32 * 4.0)).collect();
        let doc = Document::new(state);
        let library = Library::new(None, 0, Arc::new(|| {}));
        let path = dir.path().join("castle.mtp");
        let saved = save_project(&doc, &library, &EmbeddedStore::default(), &path).expect("save");
        let entries: Vec<&str> = saved.embedded.iter().map(|(_, e)| e.as_str()).collect();
        assert_eq!(entries, ["stl/Tiles/floor.stl", "stl/Tiles/floor.stl", "stl/Tiles (3)/floor.stl"]);
        assert!(Embedded::open(&path).expect("open").fingerprint("stl/Tiles (2)/floor.stl").is_none());

        // Opened on another computer, both tiles find the one copy, and
        // saving from the copies keeps sharing it
        for pack in ["A", "B", "C"] {
            std::fs::remove_dir_all(dir.path().join(pack)).expect("remove");
        }
        let pending = read_project(&path).expect("reopen");
        assert_eq!(pending.file.embedded.len(), 3);
        let store = embedded_store(&pending);
        assert_eq!(embedded::bytes(&keys[1], &store).expect("inside"), std::fs::read(FLOOR).expect("fixture"));
        assert_eq!(embedded::bytes(&keys[2], &store).expect("inside"), std::fs::read(WALL).expect("fixture"));
        let again = save_project(&Document::new(finish_open(&pending)), &library, &store, &path).expect("save");
        assert_eq!(again.embedded, saved.embedded);
    }

    #[test]
    fn a_file_on_disk_wins_over_the_copy_inside() {
        // The library keeps folders as absolute paths without ".."
        let floor = std::path::absolute(FLOOR).expect("absolute");
        let key = TileKey::from_path(&floor);
        let mut library = Library::new(None, 0, Arc::new(|| {}));
        let folder = floor.parent().expect("folder");
        library.add_folder(folder, false);
        settle(&mut library);
        let from_disk = library.tile(&key).expect("loaded").folder;
        // A stale copy inside the project of a file that is on disk
        let stale = crate::embedded::Stored::Memory(Arc::new(b"solid stale".to_vec()));
        let item = EmbeddedTile { key: key.clone(), pack: "good".into(), relative: "x".into(), stored: stale };
        let section = library.add_embedded(vec![item]);
        settle(&mut library);
        assert_eq!(library.tile(&key).expect("still there").folder, from_disk);
        assert!(library.folder(section).expect("section").tiles.contains(&key), "listed in the section too");
    }

    #[test]
    fn exported_stl_files_go_into_a_new_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let key = TileKey::new(FLOOR);
        let gone = TileKey::new("C:/Gone/wall.stl");
        let mut state = DocState::empty(GridSize { cols: 20, rows: 20 });
        state.tiles = vec![placed(1, &key, 2.0), placed(2, &key, 6.0), placed(3, &gone, 10.0)];
        let names = HashMap::from([(key.clone(), "floor (Packs)".to_owned())]);
        let store = EmbeddedStore::default();
        let (folder, written, missing) =
            export_stls(&state, &store, &names, dir.path(), "Crypt: level 1").expect("export");
        assert_eq!(folder, dir.path().join("Crypt_ level 1 STL files"));
        assert_eq!((written, missing), (1, vec![gone]));
        let copy = std::fs::read(folder.join("floor (Packs).stl")).expect("written");
        assert_eq!(copy, std::fs::read(FLOOR).expect("fixture"));
        let (second, ..) = export_stls(&state, &store, &names, dir.path(), "Crypt: level 1").expect("again");
        assert_eq!(second, dir.path().join("Crypt_ level 1 STL files (2)"), "never overwrites");
    }

    #[test]
    fn save_then_open_keeps_unresolved_tiles() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/projects/v3.mtp");
        let pending = read_project(Path::new(fixture)).expect("v3");
        assert_eq!(pending.missing.len(), 2, "the fixture's folders don't exist here");
        let state = finish_open(&pending);
        let library = Library::new(None, 0, Arc::new(|| {}));
        let path = dir.path().join("castle.mtp");
        let saved =
            save_project(&Document::new(state.clone()), &library, &EmbeddedStore::default(), &path).expect("save");
        assert!(saved.embedded.is_empty(), "the fixture's STL files aren't here to carry");
        let again = finish_open(&read_project(&path).expect("reopen"));
        assert_eq!(again.tiles, state.tiles);
    }

    #[test]
    fn a_missing_folder_is_saved_back_as_it_was() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = Library::new(None, 0, Arc::new(|| {}));
        let gone = dir.path().join("Unplugged drive").join("Tiles");
        library.add_folder(&gone, true);
        let path = dir.path().join("castle.mtp");
        save_project(&Document::default(), &library, &EmbeddedStore::default(), &path).expect("save");
        let file = container::read_file(&path).expect("readable");
        assert_eq!(file.folders.len(), 1);
        assert!(file.folders[0].recursive, "the subfolders setting is kept");
        assert_eq!(file.folders[0].path, "Unplugged drive/Tiles");
    }

    #[test]
    fn autosaves_are_found_and_discarded() {
        let dir = tempfile::tempdir().expect("temp dir");
        let library = Library::new(None, 0, Arc::new(|| {}));
        let original = dir.path().join("castle.mtp");
        let store = EmbeddedStore::default();
        let doc = Document::default();
        let mine = autosave(dir.path(), "a", &doc, &library, &store, Some(&original)).expect("autosave");
        autosave(dir.path(), "b", &doc, &library, &store, Some(&original)).expect("another window");
        let found = autosaves(dir.path());
        assert_eq!(found.len(), 2, "two sessions don't overwrite each other");
        discard_autosave_file(&mine);
        let found = autosaves(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original.as_deref(), Some(original.as_path()));
        discard_autosaves(dir.path());
        assert!(autosaves(dir.path()).is_empty());
    }

    #[test]
    fn maps_fill_the_grid_keeping_proportions() {
        assert_eq!(fit_to_grid(40, 20, 100, 100), [10.0, 0.0, 20.0, 20.0]);
        assert_eq!(fit_to_grid(20, 40, 200, 100), [0.0, 15.0, 20.0, 10.0]);
    }

    #[test]
    fn adds_the_extension() {
        assert_eq!(with_extension(PathBuf::from("a/castle")), PathBuf::from("a/castle.mtp"));
        assert_eq!(with_extension(PathBuf::from("a/castle.MTP")), PathBuf::from("a/castle.MTP"));
    }
}
