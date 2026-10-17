mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mtp_stl::{MeshCache, load_bytes, load_tile};

/// Copies a fixture into `dir` so its stamp can be changed.
fn copy_fixture(dir: &Path, name: &str) -> PathBuf {
    let to = dir.join(name);
    fs::copy(common::fixture(&format!("good/{name}")), &to).expect("copy fixture");
    to
}

fn cache_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir).map(|r| r.map(|e| e.expect("entry").path()).collect()).unwrap_or_default()
}

#[test]
fn round_trip_and_stale_entries() {
    let src = tempfile::tempdir().expect("temp dir");
    let cache_dir = tempfile::tempdir().expect("temp dir");
    let cache = MeshCache::new(cache_dir.path().join("meshes"), 1 << 30);
    let stl = copy_fixture(src.path(), "two_boxes_zero_normals.stl");

    assert!(cache.get(&stl).is_none());
    let tile = load_tile(&stl, Some(&cache)).expect("loads");
    assert_eq!(cache.get(&stl), Some(tile.clone()));
    assert!(cache.size_bytes() > 0);

    // A new mtime is a different file as far as the cache knows
    let file = fs::File::options().write(true).open(&stl).expect("open");
    file.set_modified(SystemTime::now() + Duration::from_secs(60)).expect("set mtime");
    drop(file);
    assert!(cache.get(&stl).is_none());

    // So is a different length, even with the old mtime
    cache.put(&tile).expect("put");
    assert!(cache.get(&stl).is_some());
    let stamp = fs::metadata(&stl).expect("metadata").modified().expect("mtime");
    let file = fs::File::options().write(true).open(&stl).expect("open");
    file.set_len(2000).expect("grow");
    file.set_modified(stamp).expect("restore mtime");
    drop(file);
    assert!(cache.get(&stl).is_none());

    cache.clear().expect("clear");
    assert_eq!(cache.size_bytes(), 0);
}

#[test]
fn corrupt_entries_are_misses_and_removed() {
    let src = tempfile::tempdir().expect("temp dir");
    let cache_dir = tempfile::tempdir().expect("temp dir");
    let cache = MeshCache::new(cache_dir.path().to_path_buf(), 1 << 30);
    let stl = copy_fixture(src.path(), "floor_2x2_mm.stl");
    load_tile(&stl, Some(&cache)).expect("loads");

    let files = cache_files(cache_dir.path());
    assert_eq!(files.len(), 1);
    let mut bytes = fs::read(&files[0]).expect("read cache file");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    fs::write(&files[0], &bytes).expect("corrupt");

    assert!(cache.get(&stl).is_none());
    assert!(cache_files(cache_dir.path()).is_empty());

    // Truncated files too
    load_tile(&stl, Some(&cache)).expect("loads");
    let files = cache_files(cache_dir.path());
    fs::write(&files[0], &fs::read(&files[0]).expect("read")[..40]).expect("truncate");
    assert!(cache.get(&stl).is_none());
    assert!(cache_files(cache_dir.path()).is_empty());
}

#[test]
fn eviction_keeps_under_the_cap() {
    let src = tempfile::tempdir().expect("temp dir");
    let cache_dir = tempfile::tempdir().expect("temp dir");
    let names = ["floor_2x2_mm.stl", "offset_origin.stl", "wall_inch.stl", "tiny_mm_part.stl"];
    let paths: Vec<PathBuf> = names.iter().map(|n| copy_fixture(src.path(), n)).collect();

    let probe = MeshCache::new(cache_dir.path().join("probe"), u64::MAX);
    load_tile(&paths[0], Some(&probe)).expect("loads");
    let one = probe.size_bytes();
    assert!(one > 0);

    let cap = one * 2 + one / 2;
    let cache = MeshCache::new(cache_dir.path().join("capped"), cap);
    for path in &paths {
        load_tile(path, Some(&cache)).expect("loads");
        assert!(cache.size_bytes() <= cap);
    }
    assert_eq!(cache_files(cache.dir()).len(), 2);
    // The newest survive
    assert!(cache.get(&paths[3]).is_some());
}

#[test]
fn clearing_removes_files_left_by_a_crash() {
    let src = tempfile::tempdir().expect("temp dir");
    let cache_dir = tempfile::tempdir().expect("temp dir");
    let dir = cache_dir.path().join("meshes");
    let cache = MeshCache::new(dir.clone(), 1 << 30);
    let stl = copy_fixture(src.path(), "floor_2x2_mm.stl");
    load_tile(&stl, Some(&cache)).expect("loads");
    fs::write(dir.join("0123456789abcdef.999-0.tmp"), b"half a file").expect("leftover");
    cache.clear().expect("clear");
    assert!(cache_files(&dir).is_empty(), "{:?}", cache_files(&dir));
    assert_eq!(cache.size_bytes(), 0);
}

#[test]
fn bytes_load_like_the_file_and_are_cached_by_contents() {
    let cache_dir = tempfile::tempdir().expect("temp dir");
    let meshes = cache_dir.path().join("meshes");
    let cache = MeshCache::new(meshes.clone(), 1 << 30);
    let path = common::fixture("good/floor_2x2_mm.stl");
    let bytes = fs::read(&path).expect("fixture");
    let from_file = load_tile(&path, None).expect("file");

    // An STL inside a project, named after where its folder used to be
    let named = Path::new("C:/Gone/floor_2x2_mm.stl");
    let first = load_bytes(named, &bytes, Some(&cache)).expect("bytes");
    assert_eq!(cache_files(&meshes).len(), 1);
    let again = load_bytes(named, &bytes, Some(&cache)).expect("cached");
    assert_eq!(cache_files(&meshes).len(), 1, "the same contents are prepared once");
    assert_eq!(first, again);
    assert_eq!((first.path.as_path(), first.name.as_str()), (named, "floor_2x2_mm"));
    assert_eq!(
        (first.size, first.lods.len(), first.volume_mm3),
        (from_file.size, from_file.lods.len(), from_file.volume_mm3)
    );
    assert!(load_bytes(named, b"not an STL file", None).is_err());
}
