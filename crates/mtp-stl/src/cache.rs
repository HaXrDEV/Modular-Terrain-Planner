//! On-disk cache of loaded tiles, so reopening a library skips parsing and
//! simplifying.
//!
//! One file per tile, named after a hash of the STL's path, length, change
//! time and [`LOADER_VERSION`]. Editing or replacing an STL therefore misses
//! the cache instead of showing a stale mesh. The format is a flat binary
//! layout rather than a serde format: it's read on every launch for every
//! tile, and positions and indices copy straight in.
//!
//! Layout (little-endian):
//! `magic, format version, loader version, key, unit (u8 + 3 padding), w, h,
//! gz, extent_mm[3], volume_mm3, source_triangles, tier count`, then per tier
//! `vertex count, index count, positions, indices`, then an FNV-1a checksum of
//! everything before it.

use std::fs;
use std::hash::Hasher;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mtp_core::units::{SourceUnit, TileSize};

use crate::hash::{Fnv1a, fnv1a};
use crate::load::{LOADER_VERSION, LoadedTile, tile_name};
use crate::lod::MAX_TIERS;
use crate::mesh::IndexedMesh;

const MAGIC: &[u8; 8] = b"MTPMESH\0";
const FORMAT_VERSION: u32 = 1;
const EXTENSION: &str = "mtpmesh";

/// Distinguishes temporary files written by threads of this process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
/// The running total isn't known yet.
const UNKNOWN: u64 = u64::MAX;
/// Temporary files older than this were left by a crash (a write takes
/// well under a second).
const STALE_TEMP: Duration = Duration::from_secs(600);

/// A folder of cached tiles, kept under a size cap.
#[derive(Debug, Clone)]
pub struct MeshCache {
    dir: PathBuf,
    max_bytes: u64,
    /// Bytes in the cache as far as this process knows, so a write only
    /// scans the folder when the cap may be passed.
    total: Arc<AtomicU64>,
}

impl MeshCache {
    /// The folder is created on the first write.
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        Self { dir, max_bytes, total: Arc::new(AtomicU64::new(UNKNOWN)) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The cached copy of the STL at `path`, if there is a current one.
    pub fn get(&self, path: &Path) -> Option<LoadedTile> {
        self.get_by_key(path, self.key_for(path).ok()?)
    }

    /// Stores `tile` under the current stamp of its file.
    pub fn put(&self, tile: &LoadedTile) -> io::Result<()> {
        self.put_with_key(tile, self.key_for(&tile.path)?)
    }

    /// Deletes every cached tile.
    pub fn clear(&self) -> io::Result<()> {
        for (path, _, _) in self.entries()? {
            remove_quietly(&path)?;
        }
        self.sweep_temps(Duration::ZERO);
        self.total.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// Total size of the cached tiles.
    pub fn size_bytes(&self) -> u64 {
        self.entries().map(|e| e.iter().map(|(_, len, _)| len).sum()).unwrap_or(0)
    }

    /// Identifies one version of one file.
    pub(crate) fn key_for(&self, path: &Path) -> io::Result<u64> {
        let meta = fs::metadata(path)?;
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        let mut h = Fnv1a::default();
        h.write(canonical.to_string_lossy().as_bytes());
        h.write(&meta.len().to_le_bytes());
        h.write(&mtime.to_le_bytes());
        h.write(&LOADER_VERSION.to_le_bytes());
        Ok(h.finish())
    }

    fn file_for(&self, key: u64) -> PathBuf {
        self.dir.join(format!("{key:016x}.{EXTENSION}"))
    }

    pub(crate) fn get_by_key(&self, path: &Path, key: u64) -> Option<LoadedTile> {
        let file = self.file_for(key);
        let bytes = fs::read(&file).ok()?;
        match decode(&bytes, key, path) {
            Some(tile) => {
                // Refresh the age so eviction drops tiles that went unused longest
                if let Ok(f) = fs::File::options().write(true).open(&file) {
                    let _ = f.set_modified(SystemTime::now());
                }
                Some(tile)
            }
            None => {
                let _ = fs::remove_file(&file);
                None
            }
        }
    }

    pub(crate) fn put_with_key(&self, tile: &LoadedTile, key: u64) -> io::Result<()> {
        let bytes = encode(tile, key).ok_or_else(|| io::Error::other("the cache only works on little-endian CPUs"))?;
        fs::create_dir_all(&self.dir)?;
        // Written beside the target and renamed, so a crash or a reader on
        // another thread never sees half a file
        let temp = self.dir.join(format!(
            "{key:016x}.{}-{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let written = fs::File::create(&temp).and_then(|mut f| {
            f.write_all(&bytes)?;
            f.sync_all()
        });
        if let Err(e) = written.and_then(|()| fs::rename(&temp, self.file_for(key))) {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
        let added = bytes.len() as u64;
        let known = self.total.load(Ordering::Relaxed);
        if known == UNKNOWN || known.saturating_add(added) > self.max_bytes {
            // The first write, or the cap may be passed: count for real
            self.evict()
        } else {
            self.total.fetch_add(added, Ordering::Relaxed);
            Ok(())
        }
    }

    /// Removes temporary files at least `age` old: those a crash left.
    fn sweep_temps(&self, age: Duration) {
        let Ok(read) = fs::read_dir(&self.dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|elapsed| elapsed >= age);
            if path.extension().is_some_and(|e| e == "tmp") && old {
                let _ = fs::remove_file(&path);
            }
        }
    }

    /// Deletes the least recently used tiles until the cache fits its cap.
    fn evict(&self) -> io::Result<()> {
        self.sweep_temps(STALE_TEMP);
        let mut entries = self.entries()?;
        let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
        self.total.store(total, Ordering::Relaxed);
        if total <= self.max_bytes {
            return Ok(());
        }
        entries.sort_by_key(|(_, _, modified)| *modified);
        for (path, len, _) in entries {
            if total <= self.max_bytes {
                break;
            }
            remove_quietly(&path)?;
            total -= len;
        }
        self.total.store(total, Ordering::Relaxed);
        Ok(())
    }

    /// Cache files with their size and modified time.
    fn entries(&self) -> io::Result<Vec<(PathBuf, u64, SystemTime)>> {
        let read = match fs::read_dir(&self.dir) {
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for entry in read {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == EXTENSION)
                && let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                out.push((path, meta.len(), meta.modified().unwrap_or(UNIX_EPOCH)));
            }
        }
        Ok(out)
    }
}

/// Another thread may have deleted the file already.
fn remove_quietly(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn encode(tile: &LoadedTile, key: u64) -> Option<Vec<u8>> {
    // Positions and indices are copied as raw memory
    if cfg!(target_endian = "big") {
        return None;
    }
    let mesh_bytes: usize = tile.lods.iter().map(|m| 8 + m.positions.len() * 12 + m.indices.len() * 4).sum();
    let mut out = Vec::with_capacity(96 + mesh_bytes);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&LOADER_VERSION.to_le_bytes());
    out.extend_from_slice(&key.to_le_bytes());
    out.extend_from_slice(&[u8::from(tile.unit == SourceUnit::Inches), 0, 0, 0]);
    out.extend_from_slice(&tile.size.w.to_le_bytes());
    out.extend_from_slice(&tile.size.h.to_le_bytes());
    out.extend_from_slice(&tile.size.gz.to_le_bytes());
    for v in tile.extent_mm {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&tile.volume_mm3.to_le_bytes());
    out.extend_from_slice(&tile.source_triangles.to_le_bytes());
    out.extend_from_slice(&u32::try_from(tile.lods.len()).ok()?.to_le_bytes());
    for mesh in &tile.lods {
        out.extend_from_slice(&u32::try_from(mesh.positions.len()).ok()?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(mesh.indices.len()).ok()?.to_le_bytes());
        out.extend_from_slice(bytemuck::cast_slice(&mesh.positions));
        out.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
    }
    let checksum = fnv1a(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Some(out)
}

/// Reads fields front to back. Every read fails cleanly past the end.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.array().map(u64::from_le_bytes)
    }

    fn f32(&mut self) -> Option<f32> {
        self.array().map(f32::from_le_bytes)
    }

    fn f64(&mut self) -> Option<f64> {
        self.array().map(f64::from_le_bytes)
    }
}

/// `None` for anything unexpected: the caller deletes the file and reloads.
fn decode(bytes: &[u8], key: u64, path: &Path) -> Option<LoadedTile> {
    if cfg!(target_endian = "big") {
        return None;
    }
    let (body, checksum) = bytes.split_last_chunk::<8>()?;
    if fnv1a(body) != u64::from_le_bytes(*checksum) {
        return None;
    }
    let mut r = Reader(body);
    let header_ok = r.take(8)? == MAGIC && r.u32()? == FORMAT_VERSION && r.u32()? == LOADER_VERSION && r.u64()? == key;
    if !header_ok {
        return None;
    }
    let unit = match r.array::<4>()?[0] {
        0 => SourceUnit::Millimetres,
        1 => SourceUnit::Inches,
        _ => return None,
    };
    let size = TileSize { w: r.u32()?, h: r.u32()?, gz: r.f32()? };
    let extent_mm = [r.f64()?, r.f64()?, r.f64()?];
    let volume_mm3 = r.f64()?;
    let source_triangles = r.u32()?;
    let tiers = r.u32()? as usize;
    if !(1..=MAX_TIERS).contains(&tiers) {
        return None;
    }
    let mut lods = Vec::with_capacity(tiers);
    for _ in 0..tiers {
        let vertices = r.u32()? as usize;
        let indices = r.u32()? as usize;
        let positions: Vec<[f32; 3]> = bytemuck::pod_collect_to_vec(r.take(vertices.checked_mul(12)?)?);
        let indices: Vec<u32> = bytemuck::pod_collect_to_vec(r.take(indices.checked_mul(4)?)?);
        if indices.is_empty() || !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= vertices) {
            return None;
        }
        lods.push(IndexedMesh { positions, indices });
    }
    if !r.0.is_empty() {
        return None;
    }
    Some(LoadedTile {
        path: path.to_path_buf(),
        name: tile_name(path),
        unit,
        size,
        extent_mm,
        volume_mm3,
        source_triangles,
        lods,
    })
}
