//! STL files inside the open project (format 6): which tile each stands in
//! for, and where to read it. A tile uses its file on disk when it's there;
//! these fill in when it isn't, and saving refreshes them.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mtp_core::container::Embedded;
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
        match self {
            Self::Archive { path, entry } => Embedded::open(path)?.read(entry),
            Self::Memory(bytes) => Ok(bytes.to_vec()),
        }
    }
}

/// The open project's embedded STL files, by the tile key each stands in for.
#[derive(Clone, Debug, Default)]
pub struct EmbeddedStore {
    files: HashMap<TileKey, Stored>,
}

impl EmbeddedStore {
    /// The files inside the project at `path`, read when needed.
    pub fn in_archive(path: &Path, entries: impl IntoIterator<Item = (TileKey, String)>) -> Self {
        let files = entries
            .into_iter()
            .map(|(key, entry)| (key, Stored::Archive { path: path.to_path_buf(), entry }))
            .collect();
        Self { files }
    }

    /// Reads every file now, for an archive that is about to be deleted.
    pub fn read_into_memory(&self) -> io::Result<Self> {
        let files = self
            .files
            .iter()
            .map(|(key, stored)| Ok((key.clone(), Stored::Memory(Arc::new(stored.read()?)))))
            .collect::<io::Result<_>>()?;
        Ok(Self { files })
    }

    pub fn get(&self, key: &TileKey) -> Option<&Stored> {
        self.files.get(key)
    }
}

/// Whether a tile's STL file can be saved into a project: it's on disk, or
/// inside the project being replaced.
pub fn available(key: &TileKey, store: &EmbeddedStore) -> bool {
    Path::new(key.as_str()).is_file() || store.get(key).is_some()
}

/// A tile's STL file: from disk when it's there, else from the project.
pub fn bytes(key: &TileKey, store: &EmbeddedStore) -> io::Result<Vec<u8>> {
    match std::fs::read(key.as_str()) {
        Ok(bytes) => Ok(bytes),
        Err(e) => match store.get(key) {
            Some(stored) => stored.read(),
            None => Err(e),
        },
    }
}
