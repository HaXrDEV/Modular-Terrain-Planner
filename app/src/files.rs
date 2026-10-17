//! Opening and saving projects, autosave and recovery, battle map images.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mtp_core::document::{DocState, Document, GroundImage};
use mtp_core::persist::write_atomic;
use mtp_core::project::{self, LibraryFolder, ProjectFile};
use mtp_core::tile::TileKey;

use crate::library::Library;
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
    let bytes = std::fs::read(path).map_err(|e| format!("{} can't be read: {e}", path.display()))?;
    let file = project::read(&bytes).map_err(|e| format!("{} can't be opened: {e}", path.display()))?;
    let dir = dir_of(path);
    let found: Vec<Option<String>> =
        file.folders.iter().map(|f| project::locate(&dir, f, |p| Path::new(p).is_dir())).collect();
    let missing = found.iter().enumerate().filter(|(_, f)| f.is_none()).map(|(i, _)| i).collect();
    Ok(PendingOpen { path: path.to_path_buf(), file, found, missing })
}

/// The document for a project whose folders have been looked for.
pub fn finish_open(pending: &PendingOpen) -> DocState {
    let dir = pending.project_dir();
    let image = pending.file.ground_image.as_ref().and_then(|img| {
        let candidates = [Some(project::join(&dir, &img.path)), img.absolute.clone()];
        candidates.into_iter().flatten().find(|p| Path::new(p).is_file())
    });
    project::to_state(&pending.file, &dir, &pending.found, image)
}

/// Folders a document needs, in the order to load them. Folders that
/// weren't found are listed too, at their saved place, so they show as not
/// found in the library and are saved back as they were.
pub fn folders_to_load(pending: &PendingOpen) -> Vec<(String, bool)> {
    let dir = pending.project_dir();
    pending
        .file
        .folders
        .iter()
        .zip(&pending.found)
        .map(|(f, found)| (found.clone().unwrap_or_else(|| project::saved_location(&dir, f)), f.recursive))
        .collect()
}

pub fn project_bytes(state: &DocState, library: &Library, path: &Path) -> Vec<u8> {
    // Folders that aren't there right now are written back too, so an
    // unplugged drive doesn't change the project
    let open: Vec<LibraryFolder> =
        library.folders().iter().map(|f| LibraryFolder { path: f.path.clone(), recursive: f.recursive }).collect();
    let file = project::from_state(state, &open, &dir_of(path));
    project::write(&file, VERSION).into_bytes()
}

/// Saves atomically, keeping the previous file as `.bak`.
pub fn save_project(doc: &Document, library: &Library, path: &Path) -> Result<(), String> {
    let bytes = project_bytes(doc.state(), library, path);
    write_atomic(path, &bytes, true).map_err(|e| format!("{} couldn't be saved: {e}", path.display()))
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

/// Autosaves `doc`. Each session writes its own file (`session` tells them
/// apart), so one window never overwrites or deletes another's.
pub fn autosave(
    dir: &Path,
    session: &str,
    doc: &Document,
    library: &Library,
    original: Option<&Path>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name =
        original.and_then(|p| p.file_stem()).map_or_else(|| "Untitled".into(), |s| s.to_string_lossy().into_owned());
    let file = dir.join(format!("{name}-{session}.autosave.mtp"));
    let bytes = project_bytes(doc.state(), library, original.unwrap_or(&file));
    write_atomic(&file, &bytes, false).map_err(|e| e.to_string())?;
    let note = original.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    write_atomic(&note_path(&file), note.as_bytes(), false).map_err(|e| e.to_string())?;
    Ok(file)
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
    let img = if img.width() > max_side || img.height() > max_side {
        tracing::info!("scaling {} down to {max_side} px", path.display());
        img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(DecodedImage { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
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
    use std::sync::Arc;

    #[test]
    fn save_then_open_keeps_unresolved_tiles() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/projects/v3.mtp");
        let pending = read_project(Path::new(fixture)).expect("v3");
        assert_eq!(pending.missing.len(), 2, "the fixture's folders don't exist here");
        let state = finish_open(&pending);
        let library = Library::new(None, 0, Arc::new(|| {}));
        let path = dir.path().join("castle.mtp");
        save_project(&Document::new(state.clone()), &library, &path).expect("save");
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
        let bytes = project_bytes(&Document::default().state().clone(), &library, &path);
        let file = mtp_core::project::read(&bytes).expect("readable");
        assert_eq!(file.folders.len(), 1);
        assert!(file.folders[0].recursive, "the subfolders setting is kept");
        assert_eq!(file.folders[0].path, "Unplugged drive/Tiles");
    }

    #[test]
    fn autosaves_are_found_and_discarded() {
        let dir = tempfile::tempdir().expect("temp dir");
        let library = Library::new(None, 0, Arc::new(|| {}));
        let original = dir.path().join("castle.mtp");
        let mine = autosave(dir.path(), "a", &Document::default(), &library, Some(&original)).expect("autosave");
        autosave(dir.path(), "b", &Document::default(), &library, Some(&original)).expect("another window");
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
