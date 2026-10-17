//! The tile library: STL folders the user has opened, loaded in the
//! background. It belongs to the session, not the project, so New and Open
//! keep it (the Python version unloaded everything and tied tabs to files).
//! The exception is "In this project": the STL files inside the open
//! project, which fill in for files that aren't on this computer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender};
use mtp_core::color::tile_color;
use mtp_core::tile::TileKey;
use mtp_core::units::{SourceUnit, TileSize};
use mtp_stl::{LoadedTile, MeshCache};

use crate::embedded::{self, Archives};

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
    /// The STL files inside the open project rather than a folder on disk.
    pub embedded: bool,
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
    /// For a tile from inside the project: the folder it was saved from,
    /// named in print lists in place of "In this project".
    pub pack: Option<String>,
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

/// One STL file inside the open project, for [`Library::add_embedded`].
#[derive(Clone, Debug)]
pub struct EmbeddedTile {
    pub key: TileKey,
    /// The folder it was saved from, for its colour and print lists.
    pub pack: String,
    /// Its path inside that folder, as shown in the library.
    pub relative: String,
    pub stored: embedded::Stored,
}

/// Where a loaded tile is listed, when not just in its folder.
struct Label {
    relative: String,
    pack: String,
}

/// The name of the project's own section.
pub const EMBEDDED_NAME: &str = "In this project";

enum Msg {
    Found {
        folder: FolderId,
        total: usize,
    },
    Loaded {
        folder: FolderId,
        tile: Box<LoadedTile>,
        label: Option<Label>,
    },
    Skipped {
        folder: FolderId,
        message: String,
    },
    Finished {
        folder: FolderId,
    },
    Unavailable {
        folder: FolderId,
        message: String,
    },
    /// Files on disk that differ from their copy inside the project.
    Changed {
        folder: FolderId,
        keys: Vec<TileKey>,
    },
}

/// What happened while loading, for the app to react to.
#[derive(Debug)]
pub enum Event {
    TileLoaded(TileKey),
    FolderFinished {
        name: String,
        loaded: usize,
        skipped: Vec<String>,
    },
    FolderUnavailable {
        path: String,
        message: String,
    },
    /// STL files on disk that differ from their copies inside the project
    /// just opened, which only fill in for files that aren't on disk.
    ChangedOnDisk(Vec<TileKey>),
}

pub struct Library {
    folders: Vec<Folder>,
    tiles: HashMap<TileKey, Tile>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    cache: Option<Arc<MeshCache>>,
    next_id: u32,
    repaint: Arc<dyn Fn() + Send + Sync>,
    /// The open project's STL files, as given to [`Library::add_embedded`].
    embedded_items: HashMap<TileKey, EmbeddedTile>,
    /// Files whose copy inside the project is used even though the file
    /// is on disk.
    pinned: HashSet<TileKey>,
    /// Files in the project's section that aren't on this computer.
    embedded_missing: HashSet<TileKey>,
}

impl Library {
    pub fn new(cache_dir: Option<PathBuf>, cache_bytes: u64, repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let cache = cache_dir.map(|dir| Arc::new(MeshCache::new(dir, cache_bytes)));
        Self {
            folders: Vec::new(),
            tiles: HashMap::new(),
            tx,
            rx,
            cache,
            next_id: 1,
            repaint,
            embedded_items: HashMap::new(),
            pinned: HashSet::new(),
            embedded_missing: HashSet::new(),
        }
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

    /// Starts loading a folder (again, if it's already open).
    pub fn add_folder(&mut self, path: &Path, recursive: bool) -> FolderId {
        // Relative folders (from the command line) would be saved relative to
        // the wrong place
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let path = absolute.as_path();
        let path_str = TileKey::from_path(path).as_str().to_owned();
        if let Some(id) = self.find_folder(&path_str) {
            let unavailable = matches!(self.folder(id).map(|f| &f.status), Some(FolderStatus::Unavailable(_)));
            if !unavailable {
                return id;
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
            embedded: false,
            cancel: cancel.clone(),
        });

        let (tx, cache, repaint, dir) = (self.tx.clone(), self.cache.clone(), self.repaint.clone(), path.to_path_buf());
        let spawned = std::thread::Builder::new().name(format!("load {}", dir.display())).spawn(move || {
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
                Ok(tile) => send(Msg::Loaded { folder: id, tile: Box::new(tile), label: None }),
                Err(e) => send(Msg::Skipped { folder: id, message: e.to_string() }),
            });
            send(Msg::Finished { folder: id });
        });
        if let Err(e) = spawned {
            tracing::error!("couldn't start loading {}: {e}", path.display());
        }
        id
    }

    /// The section for the open project's STL files, if it has any.
    pub fn embedded_folder(&self) -> Option<FolderId> {
        self.folders.iter().find(|f| f.embedded).map(|f| f.id)
    }

    /// Lists the open project's STL files in their own section, first in the
    /// library, and loads those that aren't on disk (a file on disk always
    /// wins, so it loads from its folder as usual). Files on disk are
    /// compared with their copy, and [`Event::ChangedOnDisk`] lists the ones
    /// that differ. The caller removes the section of the project open before.
    pub fn add_embedded(&mut self, items: Vec<EmbeddedTile>) -> FolderId {
        let id = FolderId(self.next_id);
        self.next_id += 1;
        let mut keys: Vec<TileKey> = items.iter().map(|i| i.key.clone()).collect();
        keys.sort_by_key(|k| k.stem().to_lowercase());
        self.embedded_missing =
            items.iter().filter(|i| !Path::new(i.key.as_str()).is_file()).map(|i| i.key.clone()).collect();
        let to_load = self.embedded_missing.len();
        self.folders.insert(
            0,
            Folder {
                id,
                path: String::new(),
                name: EMBEDDED_NAME.to_owned(),
                recursive: false,
                status: FolderStatus::Loading { done: 0, total: to_load },
                tiles: keys,
                skipped: Vec::new(),
                embedded: true,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        self.embedded_items = items.iter().map(|i| (i.key.clone(), i.clone())).collect();
        self.load_embedded(id, items, true);
        id
    }

    /// Loads STL files from inside the project into its section `id`. With
    /// `compare`, files that are on disk aren't loaded but compared with
    /// their copy.
    fn load_embedded(&self, id: FolderId, items: Vec<EmbeddedTile>, compare: bool) {
        let Some(cancel) = self.folder(id).map(|f| f.cancel.clone()) else { return };
        let (tx, cache, repaint) = (self.tx.clone(), self.cache.clone(), self.repaint.clone());
        let spawned = std::thread::Builder::new().name("load embedded".into()).spawn(move || {
            let send = |m: Msg| {
                let _ = tx.send(m);
                repaint();
            };
            let mut archives = Archives::default();
            let mut changed = Vec::new();
            for item in items {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let path = Path::new(item.key.as_str());
                if compare && path.is_file() {
                    if embedded::changed_on_disk(&item.key, &item.stored, &mut archives) {
                        changed.push(item.key);
                    }
                    continue;
                }
                let result = archives
                    .read(&item.stored)
                    .map_err(|e| format!("{}: the copy inside the project can't be read ({e})", item.key.as_str()))
                    .and_then(|bytes| mtp_stl::load_bytes(path, &bytes, cache.as_deref()).map_err(|e| e.to_string()));
                send(match result {
                    Ok(tile) => Msg::Loaded {
                        folder: id,
                        tile: Box::new(tile),
                        label: Some(Label { relative: item.relative, pack: item.pack }),
                    },
                    Err(message) => Msg::Skipped { folder: id, message },
                });
            }
            if !changed.is_empty() {
                send(Msg::Changed { folder: id, keys: changed });
            }
            send(Msg::Finished { folder: id });
        });
        if let Err(e) = spawned {
            tracing::error!("couldn't start loading the project's STL files: {e}");
        }
    }

    /// Uses the copies of `keys` inside the project from now on, though the
    /// files are on disk: they load from the project, and loading their
    /// folder again doesn't replace them.
    pub fn pin(&mut self, keys: &[TileKey]) {
        let Some(id) = self.embedded_folder() else { return };
        let items: Vec<EmbeddedTile> = keys.iter().filter_map(|k| self.embedded_items.get(k)).cloned().collect();
        self.pinned.extend(items.iter().map(|i| i.key.clone()));
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            f.status = FolderStatus::Loading { done: 0, total: items.len() };
        }
        self.load_embedded(id, items, false);
    }

    /// Loads files from disk again into the folders listing them, for tiles
    /// that showed the project's copy.
    fn reload_from_disk(&self, keys: Vec<TileKey>) {
        let mut by_folder: HashMap<FolderId, Vec<PathBuf>> = HashMap::new();
        for key in keys {
            if let Some(f) = self.folders.iter().find(|f| !f.embedded && f.tiles.contains(&key)) {
                by_folder.entry(f.id).or_default().push(PathBuf::from(key.as_str()));
            }
        }
        for (id, paths) in by_folder {
            let Some(cancel) = self.folder(id).map(|f| f.cancel.clone()) else { continue };
            let (tx, cache, repaint) = (self.tx.clone(), self.cache.clone(), self.repaint.clone());
            let spawned = std::thread::Builder::new().name("reload".into()).spawn(move || {
                mtp_stl::load_many(&paths, cache.as_deref(), &cancel, |_, result| {
                    let _ = tx.send(match result {
                        Ok(tile) => Msg::Loaded { folder: id, tile: Box::new(tile), label: None },
                        Err(e) => Msg::Skipped { folder: id, message: e.to_string() },
                    });
                    repaint();
                });
            });
            if let Err(e) = spawned {
                tracing::error!("couldn't start loading files again: {e}");
            }
        }
    }

    /// The project's STL files that weren't on this computer when listed.
    pub fn embedded_missing(&self) -> &HashSet<TileKey> {
        &self.embedded_missing
    }

    /// How the project's section names a file: its pack and its path there.
    pub fn embedded_item(&self, key: &TileKey) -> Option<&EmbeddedTile> {
        self.embedded_items.get(key)
    }

    /// After a save: the project's section lists what's inside it now.
    /// Returns the tiles that left the library with the files no longer
    /// inside, so their meshes and thumbnails can go.
    pub fn set_embedded(&mut self, mut keys: Vec<TileKey>) -> Vec<Tile> {
        keys.sort_by_key(|k| k.stem().to_lowercase());
        self.embedded_missing = keys.iter().filter(|k| !Path::new(k.as_str()).is_file()).cloned().collect();
        if let Some(f) = self.folders.iter_mut().find(|f| f.embedded) {
            let gone = std::mem::replace(&mut f.tiles, keys);
            let id = f.id;
            let orphans: Vec<TileKey> = gone
                .into_iter()
                .filter(|k| !self.folders.iter().any(|f| f.tiles.contains(k)))
                .filter(|k| self.tiles.get(k).is_some_and(|t| t.folder == id))
                .collect();
            return orphans.iter().filter_map(|k| self.tiles.remove(k)).collect();
        }
        let id = FolderId(self.next_id);
        self.next_id += 1;
        self.folders.insert(
            0,
            Folder {
                id,
                path: String::new(),
                name: EMBEDDED_NAME.to_owned(),
                recursive: false,
                status: FolderStatus::Ready,
                tiles: keys,
                skipped: Vec::new(),
                embedded: true,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        Vec::new()
    }

    /// Closes a folder; returns the tiles that left the library.
    pub fn remove_folder(&mut self, id: FolderId) -> Vec<Tile> {
        let Some(pos) = self.folders.iter().position(|f| f.id == id) else { return Vec::new() };
        let folder = self.folders.remove(pos);
        folder.cancel.store(true, Ordering::Relaxed);
        // With the project's section gone, pinned files come from disk again
        let unpinned: Vec<TileKey> = if folder.embedded {
            self.embedded_items.clear();
            self.embedded_missing.clear();
            std::mem::take(&mut self.pinned).into_iter().collect()
        } else {
            Vec::new()
        };
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
                None => removed.extend(self.tiles.remove(&key)),
            }
        }
        self.reload_from_disk(unpinned.into_iter().filter(|k| self.tiles.contains_key(k)).collect());
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
        let embedded: HashSet<FolderId> = self.folders.iter().filter(|f| f.embedded).map(|f| f.id).collect();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Found { folder, total } => {
                    if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder) {
                        f.status = FolderStatus::Loading { done: 0, total };
                    }
                }
                Msg::Loaded { folder, tile, label } => {
                    let Some(f) = self.folders.iter_mut().find(|f| f.id == folder) else { continue };
                    if let FolderStatus::Loading { done, .. } = &mut f.status {
                        *done += 1;
                    }
                    let key = TileKey::from_path(&tile.path);
                    let file = key.file_name().to_owned();
                    let (relative, pack) = match label {
                        Some(l) => (l.relative, Some(l.pack)),
                        None => {
                            let inside = key.as_str().get(f.path.len()..).unwrap_or("").trim_start_matches('/');
                            (inside.to_owned(), None)
                        }
                    };
                    // The same colour wherever the file comes from
                    let color = tile_color(pack.as_deref().unwrap_or(&f.name), &file);
                    let entry = Tile {
                        key: key.clone(),
                        name: tile.name.clone(),
                        folder,
                        relative,
                        size: tile.size,
                        unit: tile.unit,
                        volume_mm3: tile.volume_mm3,
                        color,
                        mesh: Arc::new(*tile),
                        thumbnail: None,
                        pack,
                    };
                    if !f.tiles.contains(&key) {
                        f.tiles.push(key.clone());
                    }
                    // A file on disk wins: the copy inside the project only
                    // fills in for one that isn't there, or that was pinned,
                    // and then loading the file again doesn't replace it
                    let pinned = self.pinned.contains(&key);
                    let take = if embedded.contains(&folder) {
                        pinned || !self.tiles.get(&key).is_some_and(|t| !embedded.contains(&t.folder))
                    } else {
                        !pinned || !self.tiles.contains_key(&key)
                    };
                    if take {
                        self.tiles.insert(key.clone(), entry);
                        events.push(Event::TileLoaded(key));
                    }
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
                // Only for the project still open
                Msg::Changed { folder, keys } if embedded.contains(&folder) => {
                    events.push(Event::ChangedOnDisk(keys));
                }
                Msg::Changed { .. } => {}
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/stl/good");

    /// Polls until `done` holds, gathering the events.
    fn wait(library: &mut Library, done: impl Fn(&Library) -> bool) -> Vec<Event> {
        let start = std::time::Instant::now();
        let mut events = Vec::new();
        loop {
            events.extend(library.poll());
            if library.progress().is_none() && done(library) {
                return events;
            }
            assert!(start.elapsed().as_secs() < 30, "loading took too long");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_changed_file_is_reported_and_its_copy_can_be_pinned() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = std::path::absolute(dir.path().join("Tiles")).expect("absolute");
        std::fs::create_dir(&folder).expect("folder");
        let stl = folder.join("floor.stl");
        std::fs::copy(Path::new(FIXTURES).join("floor_2x2_mm.stl"), &stl).expect("copy");
        let key = TileKey::from_path(&stl);
        // The project carries an older version of the file: a wall
        let wall = std::fs::read(Path::new(FIXTURES).join("wall_inch.stl")).expect("fixture");
        let item = EmbeddedTile {
            key: key.clone(),
            pack: "Tiles".into(),
            relative: "floor.stl".into(),
            stored: embedded::Stored::Memory(Arc::new(wall)),
        };
        let mut library = Library::new(None, 0, Arc::new(|| {}));
        let disk = library.add_folder(&folder, false);
        let section = library.add_embedded(vec![item]);
        let events = wait(&mut library, |l| l.tile(&key).is_some());
        let changed: Vec<&Vec<TileKey>> =
            events.iter().filter_map(|e| if let Event::ChangedOnDisk(k) = e { Some(k) } else { None }).collect();
        assert_eq!(changed, [&vec![key.clone()]]);
        let on_disk = library.tile(&key).expect("loaded").size;
        assert_eq!(library.tile(&key).expect("loaded").folder, disk, "the file on disk wins");

        library.pin(std::slice::from_ref(&key));
        wait(&mut library, |l| l.tile(&key).is_some_and(|t| t.folder == section));
        let pinned = library.tile(&key).expect("pinned").size;
        assert_ne!(pinned, on_disk, "the project's copy, a wall");

        // Loading the folder again doesn't replace it
        library.remove_folder(disk);
        let disk = library.add_folder(&folder, false);
        wait(&mut library, |l| l.folder(disk).is_some_and(|f| f.status == FolderStatus::Ready));
        assert_eq!(library.tile(&key).expect("still pinned").size, pinned);

        // Closing the project brings the file on disk back
        library.remove_folder(section);
        wait(&mut library, |l| l.tile(&key).is_some_and(|t| t.folder == disk && t.size == on_disk));
    }

    #[test]
    fn files_no_longer_inside_leave_after_a_save() {
        let floor = std::path::absolute(Path::new(FIXTURES).join("floor_2x2_mm.stl")).expect("absolute");
        let key = TileKey::from_path(&floor);
        let gone = TileKey::new("C:/Gone/Tiles/wall.stl");
        let item = |key: &TileKey| EmbeddedTile {
            key: key.clone(),
            pack: "Tiles".into(),
            relative: key.file_name().into(),
            stored: embedded::Stored::Memory(Arc::new(std::fs::read(&floor).expect("fixture"))),
        };
        let mut library = Library::new(None, 0, Arc::new(|| {}));
        library.add_embedded(vec![item(&key), item(&gone)]);
        wait(&mut library, |l| l.tile(&gone).is_some());
        assert_eq!(library.embedded_missing(), &HashSet::from([gone.clone()]), "the fixture is on disk");
        assert_eq!(library.embedded_item(&gone).map(|i| i.pack.as_str()), Some("Tiles"));

        // The layout stopped using the missing file before the save
        let removed = library.set_embedded(vec![key.clone()]);
        assert_eq!(removed.iter().map(|t| &t.key).collect::<Vec<_>>(), [&gone]);
        assert!(library.tile(&gone).is_none() && library.embedded_missing().is_empty());
    }

    #[test]
    fn overlapping_folders_keep_shared_tiles() {
        let fixtures = Path::new(FIXTURES);
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
}
