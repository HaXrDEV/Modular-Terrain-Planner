//! Loading tiles from disk, one at a time or a whole library in parallel.

use std::any::Any;
use std::fs;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use mtp_core::units::{SourceUnit, TileSize};
use rayon::prelude::*;
use thiserror::Error;

use crate::cache::MeshCache;
use crate::lod::build_lods;
use crate::mesh::{IndexedMesh, prepare};
use crate::parse::{StlError, Triangle, parse_stl};

/// Bumped whenever parsing, sizing, welding or simplifying changes what a
/// file turns into, so cached meshes from older builds are rebuilt.
pub const LOADER_VERSION: u32 = 1;

/// A library tile ready to place and draw.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedTile {
    pub path: PathBuf,
    /// File name without the extension, shown in the library.
    pub name: String,
    pub unit: SourceUnit,
    pub size: TileSize,
    /// Bounding box size in millimetres.
    pub extent_mm: [f64; 3],
    pub volume_mm3: f64,
    /// Triangles in the file, before welding dropped degenerate ones.
    pub source_triangles: u32,
    /// Finest first, normalised to `[0, 1]` on every axis. Never empty.
    pub lods: Vec<IndexedMesh>,
}

impl LoadedTile {
    /// Runs sizing, welding and simplification on an already parsed file.
    pub fn from_triangles(path: &Path, triangles: &[Triangle]) -> Result<Self, StlError> {
        if triangles.is_empty() {
            return Err(StlError::NoTriangles);
        }
        let prepared = prepare(triangles);
        if prepared.mesh.indices.is_empty() {
            return Err(StlError::NoTriangles);
        }
        Ok(Self {
            path: path.to_path_buf(),
            name: tile_name(path),
            unit: prepared.unit,
            size: prepared.size,
            extent_mm: prepared.extent_mm,
            volume_mm3: prepared.volume_mm3,
            source_triangles: u32::try_from(triangles.len()).unwrap_or(u32::MAX),
            lods: build_lods(prepared.mesh, prepared.extent_mm),
        })
    }

    /// The full-detail mesh.
    pub fn full(&self) -> &IndexedMesh {
        &self.lods[0]
    }
}

pub(crate) fn tile_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Why one file of a library didn't load.
#[derive(Debug, Error)]
#[error("{}: {kind}", .path.display())]
pub struct LoadError {
    pub path: PathBuf,
    pub kind: LoadErrorKind,
}

#[derive(Debug, Error)]
pub enum LoadErrorKind {
    #[error("the file can't be read ({0})")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Stl(#[from] StlError),
    /// A bug in the loader, caught so the rest of the library still loads.
    #[error("loading failed unexpectedly ({0})")]
    Panic(String),
}

/// Loads one tile, from `cache` when it holds an up-to-date copy. The file is
/// read once and a fresh result is written back to the cache.
pub fn load_tile(path: &Path, cache: Option<&MeshCache>) -> Result<LoadedTile, LoadError> {
    let fail = |kind: LoadErrorKind| LoadError { path: path.to_path_buf(), kind };
    // Taken before reading, so a file that changes meanwhile is cached under
    // its old stamp and rebuilt next time rather than the other way round.
    let key = cache.and_then(|c| c.key_for(path).ok().map(|k| (c, k)));
    if let Some((c, k)) = key
        && let Some(tile) = c.get_by_key(path, k)
    {
        return Ok(tile);
    }
    let bytes = fs::read(path).map_err(|e| fail(e.into()))?;
    let triangles = parse_stl(&bytes).map_err(|e| fail(e.into()))?;
    let tile = LoadedTile::from_triangles(path, &triangles).map_err(|e| fail(e.into()))?;
    if let Some((c, k)) = key {
        // The cache is only a speed-up, so failing to write it isn't an error
        let _ = c.put_with_key(&tile, k);
    }
    Ok(tile)
}

/// Loads `paths` in parallel, calling `on_result` with each file's index as
/// it finishes. Every file that is started reports exactly one result. Once
/// `cancel` is set no more files are started, and those files report nothing.
/// Returns how many files reported.
pub fn load_many(
    paths: &[PathBuf],
    cache: Option<&MeshCache>,
    cancel: &AtomicBool,
    on_result: impl Fn(usize, Result<LoadedTile, LoadError>) + Sync,
) -> usize {
    paths
        .par_iter()
        .enumerate()
        .map(|(index, path)| {
            if cancel.load(Ordering::Relaxed) {
                return 0;
            }
            let result = panic::catch_unwind(AssertUnwindSafe(|| load_tile(path, cache))).unwrap_or_else(|payload| {
                Err(LoadError { path: path.clone(), kind: LoadErrorKind::Panic(panic_message(payload.as_ref())) })
            });
            on_result(index, result);
            1
        })
        .sum()
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_owned()
    }
}
