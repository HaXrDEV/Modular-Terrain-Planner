//! STL files inside the open project (format 6): which tile each stands in
//! for, and where to read it. A tile uses its file on disk when it's there;
//! these fill in when it isn't, and saving refreshes them. A file whose copy
//! the user chose over a changed file on disk is pinned: the copy is used and
//! kept.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mtp_core::container::{Embedded, Fingerprint};
use mtp_core::tile::TileKey;

/// Where one embedded STL file's bytes are.
#[derive(Clone, Debug)]
pub enum Stored {
    /// An entry in a project file on disk, read when needed.
    Archive { path: PathBuf, entry: String },
    /// Already read, from a recovered autosave that is then deleted.
    Memory(Arc<Vec<u8>>),
}

impl Stored {
    pub fn read(&self) -> io::Result<Vec<u8>> {
        Archives::default().read(self)
    }
}

/// Project files being read, each opened once, for reading many entries.
#[derive(Default)]
pub struct Archives(HashMap<PathBuf, Option<Embedded<File>>>);

impl Archives {
    pub fn open(&mut self, path: &Path) -> io::Result<&mut Embedded<File>> {
        self.0
            .entry(path.to_path_buf())
            .or_insert_with(|| Embedded::open(path).ok())
            .as_mut()
            .ok_or_else(|| io::Error::other(format!("{} can't be read", path.display())))
    }

    pub fn read(&mut self, stored: &Stored) -> io::Result<Vec<u8>> {
        match stored {
            Stored::Archive { path, entry } => self.open(path)?.read(entry),
            Stored::Memory(bytes) => Ok(bytes.to_vec()),
        }
    }

    pub fn fingerprint(&mut self, stored: &Stored) -> io::Result<Fingerprint> {
        match stored {
            Stored::Archive { path, entry } => {
                self.open(path)?.fingerprint(entry).ok_or_else(|| io::Error::other(format!("{entry} isn't there")))
            }
            Stored::Memory(bytes) => Ok(Fingerprint::of(bytes)),
        }
    }
}

/// The open project's embedded STL files, by the tile key each stands in for.
#[derive(Clone, Debug, Default)]
pub struct EmbeddedStore {
    files: HashMap<TileKey, Stored>,
    /// Files whose copy is used even though the file is on disk.
    pinned: HashSet<TileKey>,
}

impl EmbeddedStore {
    /// The files inside the project at `path`, read when needed.
    pub fn in_archive(path: &Path, entries: impl IntoIterator<Item = (TileKey, String)>) -> Self {
        let files = entries
            .into_iter()
            .map(|(key, entry)| (key, Stored::Archive { path: path.to_path_buf(), entry }))
            .collect();
        Self { files, pinned: HashSet::new() }
    }

    /// Reads every file now, for an archive that is about to be deleted.
    pub fn read_into_memory(&self) -> io::Result<Self> {
        let mut archives = Archives::default();
        let files = self
            .files
            .iter()
            .map(|(key, stored)| Ok((key.clone(), Stored::Memory(Arc::new(archives.read(stored)?)))))
            .collect::<io::Result<_>>()?;
        Ok(Self { files, pinned: self.pinned.clone() })
    }

    pub fn get(&self, key: &TileKey) -> Option<&Stored> {
        self.files.get(key)
    }

    /// Uses the copies of `keys` from now on, files on disk or not.
    pub fn pin(&mut self, keys: &[TileKey]) {
        self.pinned.extend(keys.iter().filter(|k| self.files.contains_key(*k)).cloned());
    }

    pub fn is_pinned(&self, key: &TileKey) -> bool {
        self.pinned.contains(key)
    }

    /// Keeps the pins of `other`, the store this one replaces after a save.
    pub fn with_pins_of(mut self, other: &Self) -> Self {
        self.pinned = other.pinned.iter().filter(|k| self.files.contains_key(*k)).cloned().collect();
        self
    }
}

/// Whether a tile's STL file can be saved into a project: it's on disk, or
/// inside the project being replaced.
pub fn available(key: &TileKey, store: &EmbeddedStore) -> bool {
    Path::new(key.as_str()).is_file() || store.get(key).is_some()
}

/// A tile's STL file: from disk when it's there, else from the project
/// (always from the project when pinned).
pub fn bytes(key: &TileKey, store: &EmbeddedStore) -> io::Result<Vec<u8>> {
    if store.is_pinned(key)
        && let Some(stored) = store.get(key)
    {
        return stored.read();
    }
    match std::fs::read(key.as_str()) {
        Ok(bytes) => Ok(bytes),
        Err(e) => match store.get(key) {
            Some(stored) => stored.read(),
            None => Err(e),
        },
    }
}

/// Whether the file on disk differs from its copy inside the project: a
/// different size, or else a different CRC32. False when either can't be
/// read, such as a file that isn't on disk.
pub fn changed_on_disk(key: &TileKey, stored: &Stored, archives: &mut Archives) -> bool {
    archives.fingerprint(stored).is_ok_and(|fp| matches!(fp.matches_file(Path::new(key.as_str())), Ok(false)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_files_are_told_apart_and_pins_win() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("floor.stl");
        let key = TileKey::from_path(&path);
        let copy = Stored::Memory(Arc::new(b"solid copy".to_vec()));
        let mut archives = Archives::default();
        assert!(!changed_on_disk(&key, &copy, &mut archives), "not on disk");
        std::fs::write(&path, b"solid copy").expect("write");
        assert!(!changed_on_disk(&key, &copy, &mut archives), "the same");
        std::fs::write(&path, b"solid disk").expect("write");
        assert!(changed_on_disk(&key, &copy, &mut archives), "same size, other bytes");

        let mut store = EmbeddedStore::default();
        store.files.insert(key.clone(), copy);
        assert_eq!(bytes(&key, &store).expect("disk"), b"solid disk");
        store.pin(&[key.clone(), TileKey::new("C:/Not/inside.stl")]);
        assert_eq!(store.pinned.len(), 1, "only files inside can be pinned");
        assert_eq!(bytes(&key, &store).expect("copy"), b"solid copy");
        let after_save = EmbeddedStore::in_archive(&dir.path().join("p.mtp"), [(key.clone(), "stl/x".into())]);
        assert!(after_save.with_pins_of(&store).is_pinned(&key));
    }
}
