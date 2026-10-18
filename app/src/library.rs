//! The tile library: STL folders the user has opened, loaded in the
//! background. It belongs to the session, not the project, so New and Open
//! keep it (the Python version unloaded everything and tied tabs to files).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender};
use mtp_core::color::tile_color;
use mtp_core::tile::TileKey;
use mtp_core::units::{SourceUnit, TileSize};
use mtp_stl::{LoadedTile, MeshCache};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FolderId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub enum FolderStatus {
    Scanning,
    Loading {
        done: usize,
        total: usize,
    },
    Ready,
    /// The folder isn't there (an unplugged drive): kept so it comes back.
    Unavailable(String),
}

#[derive(Debug)]
pub struct Folder {
    pub id: FolderId,
    /// Absolute, with forward slashes.
    pub path: String,
    pub name: String,
    pub recursive: bool,
    pub status: FolderStatus,
    pub tiles: Vec<TileKey>,
    pub skipped: Vec<String>,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug)]
pub struct Tile {
    pub key: TileKey,
    pub name: String,
    pub folder: FolderId,
    /// Path inside the folder, for tiles in subfolders.
    pub relative: String,
    pub size: TileSize,
    pub unit: SourceUnit,
    pub volume_mm3: f64,
    pub color: [u8; 3],
    pub mesh: Arc<LoadedTile>,
    pub thumbnail: Option<eframe::egui::TextureId>,
}

impl Tile {
    /// Triangle counts of every tier, finest first.
    pub fn tiers(&self) -> Vec<u32> {
        self.mesh.lods.iter().map(|l| (l.indices.len() / 3) as u32).collect()
    }

    /// The tier for thumbnails and exact picking: detailed enough to read,
    /// cheap enough to draw a few hundred of.
    pub fn preview_tier(&self) -> usize {
        let tiers = self.tiers();
        tiers.iter().position(|&t| t <= 20_000).unwrap_or(tiers.len().saturating_sub(1))
    }
}

enum Msg {
    Found { folder: FolderId, total: usize },
    Loaded { folder: FolderId, tile: Box<LoadedTile> },
    Skipped { folder: FolderId, message: String },
    Finished { folder: FolderId },
    Unavailable { folder: FolderId, message: String },
}

/// What happened while loading, for the app to react to.
#[derive(Debug)]
pub enum Event {
    TileLoaded(TileKey),
    FolderFinished { name: String, loaded: usize, skipped: Vec<String> },
    FolderUnavailable { path: String, message: String },
}

pub struct Library {
    folders: Vec<Folder>,
    tiles: HashMap<TileKey, Tile>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    cache: Option<Arc<MeshCache>>,
    next_id: u32,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl Library {
    pub fn new(cache_dir: Option<PathBuf>, cache_bytes: u64, repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let cache = cache_dir.map(|dir| Arc::new(MeshCache::new(dir, cache_bytes)));
        Self { folders: Vec::new(), tiles: HashMap::new(), tx, rx, cache, next_id: 1, repaint }
    }

    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    pub fn folder(&self, id: FolderId) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    pub fn tile(&self, key: &TileKey) -> Option<&Tile> {
        self.tiles.get(key)
    }

    pub fn tile_mut(&mut self, key: &TileKey) -> Option<&mut Tile> {
        self.tiles.get_mut(key)
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile> {
        self.tiles.values()
    }

    /// Tiles with the same footprint as `size` (either way round) in the
    /// folder `folder`, or in every folder, in library order.
    pub fn same_size(&self, size: TileSize, folder: Option<FolderId>) -> Vec<&Tile> {
        let fits = |t: &Tile| (t.size.w, t.size.h) == (size.w, size.h) || (t.size.w, t.size.h) == (size.h, size.w);
        self.folders
            .iter()
            .filter(|f| folder.is_none_or(|id| id == f.id))
            .flat_map(|f| f.tiles.iter())
            .filter_map(|k| self.tiles.get(k))
            .filter(|t| fits(t))
            .collect()
    }

    pub fn cache(&self) -> Option<&MeshCache> {
        self.cache.as_deref()
    }

    /// Whether any folder is still loading, and overall progress.
    pub fn progress(&self) -> Option<(usize, usize, String)> {
        let mut busy = None;
        let (mut done, mut total) = (0, 0);
        for f in &self.folders {
            match f.status {
                FolderStatus::Scanning => busy = Some(f.name.clone()),
                FolderStatus::Loading { done: d, total: t } => {
                    busy.get_or_insert_with(|| f.name.clone());
                    done += d;
                    total += t;
                }
                _ => {}
            }
        }
        busy.map(|name| (done, total, name))
    }

    pub fn find_folder(&self, path: &str) -> Option<FolderId> {
        let key = TileKey::new(path);
        self.folders.iter().find(|f| TileKey::new(&f.path) == key).map(|f| f.id)
    }

    /// Starts loading a folder. One that is already open is kept as it is,
    /// unless it wasn't found or its subfolders are now wanted too.
    pub fn add_folder(&mut self, path: &Path, recursive: bool) -> FolderId {
        // Relative folders (from the command line) would be saved relative to
        // the wrong place
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let path = absolute.as_path();
        let path_str = TileKey::from_path(path).as_str().to_owned();
        if let Some(id) = self.find_folder(&path_str) {
            let unavailable = matches!(self.folder(id).map(|f| &f.status), Some(FolderStatus::Unavailable(_)));
            let adds_subfolders = recursive && self.folder(id).is_some_and(|f| !f.recursive);
            if !unavailable && !adds_subfolders {
                return id;
            }
            if !unavailable {
                return self.reload_with_subfolders(id, path);
            }
            self.remove_folder(id);
        }
        let id = FolderId(self.next_id);
        self.next_id += 1;
        let name = path.file_name().map_or_else(|| path_str.clone(), |n| n.to_string_lossy().into_owned());
        let cancel = Arc::new(AtomicBool::new(false));
        self.folders.push(Folder {
            id,
            path: path_str,
            name,
            recursive,
            status: FolderStatus::Scanning,
            tiles: Vec::new(),
            skipped: Vec::new(),
            cancel: cancel.clone(),
        });
        self.start_loading(id, path.to_path_buf(), recursive, cancel);
        id
    }

    /// Loads an open folder again, now with its subfolders, in its place. It
    /// gets a new id so whatever the old load still sends is ignored, and its
    /// tiles stay in the library while the new load runs.
    fn reload_with_subfolders(&mut self, id: FolderId, dir: &Path) -> FolderId {
        let new_id = FolderId(self.next_id);
        self.next_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            f.cancel.store(true, Ordering::Relaxed);
            f.id = new_id;
            f.recursive = true;
            f.status = FolderStatus::Scanning;
            f.skipped.clear();
            f.cancel = cancel.clone();
        }
        for t in self.tiles.values_mut().filter(|t| t.folder == id) {
            t.folder = new_id;
        }
        self.start_loading(new_id, dir.to_path_buf(), true, cancel);
        new_id
    }

    /// Scans and loads a folder on its own thread, reporting through `poll`.
    fn start_loading(&self, id: FolderId, dir: PathBuf, recursive: bool, cancel: Arc<AtomicBool>) {
        let (tx, cache, repaint) = (self.tx.clone(), self.cache.clone(), self.repaint.clone());
        let name = format!("load {}", dir.display());
        let spawned = std::thread::Builder::new().name(name.clone()).spawn(move || {
            let send = |m: Msg| {
                let _ = tx.send(m);
                repaint();
            };
            if !dir.is_dir() {
                return send(Msg::Unavailable { folder: id, message: "The folder isn't there.".into() });
            }
            let paths = match mtp_stl::scan_folder(&dir, recursive) {
                Ok(p) => p,
                Err(e) => {
                    return send(Msg::Unavailable { folder: id, message: format!("The folder can't be read ({e}).") });
                }
            };
            send(Msg::Found { folder: id, total: paths.len() });
            mtp_stl::load_many(&paths, cache.as_deref(), &cancel, |_, result| match result {
                Ok(tile) => send(Msg::Loaded { folder: id, tile: Box::new(tile) }),
                Err(e) => send(Msg::Skipped { folder: id, message: e.to_string() }),
            });
            send(Msg::Finished { folder: id });
        });
        if let Err(e) = spawned {
            tracing::error!("couldn't start the thread to {name}: {e}");
        }
    }

    /// Closes a folder; returns the keys of the tiles that left the library.
    pub fn remove_folder(&mut self, id: FolderId) -> Vec<TileKey> {
        let Some(pos) = self.folders.iter().position(|f| f.id == id) else { return Vec::new() };
        let folder = self.folders.remove(pos);
        folder.cancel.store(true, Ordering::Relaxed);
        // Folders can overlap (a folder with subfolders and one of its
        // subfolders): tiles another folder still lists stay
        let mut removed = Vec::new();
        for key in folder.tiles {
            match self.folders.iter().find(|f| f.tiles.contains(&key)) {
                Some(other) => {
                    if let Some(t) = self.tiles.get_mut(&key) {
                        t.folder = other.id;
                    }
                }
                None => {
                    self.tiles.remove(&key);
                    removed.push(key);
                }
            }
        }
        removed
    }

    pub fn cancel_all(&self) {
        for f in &self.folders {
            f.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Applies what the loading threads have sent.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Found { folder, total } => {
                    if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder) {
                        f.status = FolderStatus::Loading { done: 0, total };
                    }
                }
                Msg::Loaded { folder, tile } => {
                    let Some(f) = self.folders.iter_mut().find(|f| f.id == folder) else { continue };
                    if let FolderStatus::Loading { done, .. } = &mut f.status {
                        *done += 1;
                    }
                    let key = TileKey::from_path(&tile.path);
                    let relative = key.as_str().get(f.path.len()..).unwrap_or("").trim_start_matches('/').to_owned();
                    let entry = Tile {
                        key: key.clone(),
                        name: tile.name.clone(),
                        folder,
                        relative,
                        size: tile.size,
                        unit: tile.unit,
                        volume_mm3: tile.volume_mm3,
                        // The folder the file is in, not the library folder, so
                        // same-named files in two packs of one library differ
                        color: tile_color(key.folder_name(), key.file_name()),
                        mesh: Arc::new(*tile),
                        // A tile loaded again (overlapping folders) keeps its
                        // thumbnail, which is then redrawn in place
                        thumbnail: self.tiles.get(&key).and_then(|t| t.thumbnail),
                    };
                    if !f.tiles.contains(&key) {
                        f.tiles.push(key.clone());
                    }
                    self.tiles.insert(key.clone(), entry);
                    events.push(Event::TileLoaded(key));
                }
                Msg::Skipped { folder, message } => {
                    if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder) {
                        if let FolderStatus::Loading { done, .. } = &mut f.status {
                            *done += 1;
                        }
                        f.skipped.push(message);
                    }
                }
                Msg::Finished { folder } => {
                    if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder) {
                        f.status = FolderStatus::Ready;
                        let names: HashMap<&TileKey, &str> =
                            self.tiles.iter().map(|(k, t)| (k, t.relative.as_str())).collect();
                        f.tiles.sort_by_key(|k| names.get(k).map(|n| n.to_lowercase()));
                        events.push(Event::FolderFinished {
                            name: f.name.clone(),
                            loaded: f.tiles.len(),
                            skipped: f.skipped.clone(),
                        });
                    }
                }
                Msg::Unavailable { folder, message } => {
                    if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder) {
                        f.status = FolderStatus::Unavailable(message.clone());
                        events.push(Event::FolderUnavailable { path: f.path.clone(), message });
                    }
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_folders_keep_shared_tiles() {
        let fixtures = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/stl/good"));
        let mut library = Library::new(None, 0, Arc::new(|| {}));
        let all = library.add_folder(fixtures, true);
        let sub = library.add_folder(&fixtures.join("OpenForge [Stone]"), false);
        let start = std::time::Instant::now();
        while library.progress().is_some() || library.folders().iter().any(|f| f.tiles.is_empty()) {
            library.poll();
            assert!(start.elapsed().as_secs() < 30, "loading took too long");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let shared: Vec<TileKey> = library.folder(sub).map(|f| f.tiles.clone()).unwrap_or_default();
        assert_eq!(shared.len(), 1);
        assert!(library.remove_folder(sub).is_empty(), "the bigger folder still lists it");
        let tile = library.tile(&shared[0]).expect("still in the library");
        assert_eq!(tile.folder, all);
    }

    fn wait_for(library: &mut Library) {
        let start = std::time::Instant::now();
        while library.progress().is_some() {
            library.poll();
            assert!(start.elapsed().as_secs() < 30, "loading took too long");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn asking_for_subfolders_loads_an_open_folder_again() {
        let fixtures = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/stl/good"));
        let mut library = Library::new(None, 0, Arc::new(|| {}));
        library.add_folder(fixtures, false);
        wait_for(&mut library);
        let flat = library.folders()[0].tiles.len();
        library.add_folder(fixtures, false);
        assert_eq!(library.folders()[0].status, FolderStatus::Ready, "asked the same way: kept as it is");
        library.add_folder(fixtures, true);
        wait_for(&mut library);
        assert_eq!(library.folders().len(), 1);
        let folder = &library.folders()[0];
        assert!(folder.recursive);
        assert!(folder.tiles.len() > flat, "the subfolder's tiles are loaded too");
        assert!(folder.tiles.iter().all(|k| library.tile(k).is_some_and(|t| t.folder == folder.id)));
    }
}
