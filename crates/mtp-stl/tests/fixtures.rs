//! The STL corpus: parity with the Python version, bad files and scanning.

mod common;

use common::fixture;
use mtp_stl::{LoadErrorKind, StlError, load_tile, scan_folder};

const GOLDEN: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/python.json"));

#[test]
fn sizes_match_the_python_version() {
    let golden: serde_json::Value = serde_json::from_str(GOLDEN).expect("golden file is JSON");
    let stl = golden["stl"].as_object().expect("golden has an stl object");
    assert!(!stl.is_empty());
    for (relative, want) in stl {
        let tile = load_tile(&fixture(&format!("good/{relative}")), None).expect(relative);
        let int = |key: &str| want[key].as_u64().expect(key) as u32;
        assert_eq!(tile.size.w, int("grid_w"), "{relative} grid_w");
        assert_eq!(tile.size.h, int("grid_h"), "{relative} grid_h");
        let gz = want["grid_z"].as_f64().expect("grid_z");
        assert!((f64::from(tile.size.gz) - gz).abs() < 1e-4, "{relative} grid_z {} != {gz}", tile.size.gz);
        assert_eq!(tile.full().triangle_count() as u64, want["triangles"].as_u64().expect("triangles"), "{relative}");

        let (lo, hi) = tile.full().bounds().expect("tile has triangles");
        for (got, key) in [(lo, "normalised_min"), (hi, "normalised_max")] {
            for (axis, value) in got.iter().enumerate() {
                let expected = want[key][axis].as_f64().expect(key);
                assert!((f64::from(*value) - expected).abs() < 1e-5, "{relative} {key}[{axis}]: {value} != {expected}");
            }
        }
    }
}

#[test]
fn bad_files_give_specific_errors() {
    type Check = fn(&StlError) -> bool;
    let cases: [(&str, Check); 5] = [
        ("empty.stl", |e| *e == StlError::Empty),
        ("truncated.stl", |e| *e == StlError::Truncated { expected: 100, found: 3 }),
        ("not_an_stl.stl", |e| *e == StlError::NotStl),
        ("ascii_garbage.stl", |e| matches!(e, StlError::Ascii { line: 4, .. })),
        ("zero_triangles.stl", |e| *e == StlError::NoTriangles),
    ];
    for (name, check) in cases {
        let err = load_tile(&fixture(&format!("bad/{name}")), None).expect_err(name);
        match &err.kind {
            LoadErrorKind::Stl(stl) => assert!(check(stl), "{name}: {stl:?}"),
            other => panic!("{name}: {other:?}"),
        }
        println!("{err}");
    }
    let missing = load_tile(&fixture("bad/missing.stl"), None).expect_err("missing file");
    assert!(matches!(missing.kind, LoadErrorKind::Io(_)));
}

#[test]
fn every_bad_file_is_covered() {
    let names: Vec<String> = scan_folder(&fixture("bad"), false)
        .expect("bad folder")
        .iter()
        .map(|p| p.file_name().expect("file name").to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["ascii_garbage.stl", "empty.stl", "not_an_stl.stl", "truncated.stl", "zero_triangles.stl"]);
}

#[test]
fn scans_recursively_with_odd_names() {
    let dir = fixture("good");
    let relative = |recursive| -> Vec<String> {
        scan_folder(&dir, recursive)
            .expect("good folder")
            .iter()
            .map(|p| p.strip_prefix(&dir).expect("inside dir").to_string_lossy().replace('\\', "/"))
            .collect()
    };
    let deep = relative(true);
    assert!(deep.contains(&"OpenForge [Stone]/stone_floor_1x1.stl".to_owned()), "{deep:?}");
    assert!(deep.contains(&"Mixed Case.Stl".to_owned()), "{deep:?}");
    assert!(!deep.iter().any(|p| p.ends_with(".txt")));
    assert_eq!(deep.len(), 10);
    let mut sorted = deep.clone();
    sorted.sort_by_key(|p| p.to_lowercase());
    assert_eq!(deep, sorted);

    let shallow = relative(false);
    assert_eq!(shallow.len(), 9);
    assert!(!shallow.iter().any(|p| p.contains('/')));
}

#[test]
fn box_volume() {
    let tile = load_tile(&fixture("good/floor_2x2_mm.stl"), None).expect("box loads");
    assert_eq!(tile.extent_mm, [25.0, 25.0, 6.0]);
    assert!((tile.volume_mm3 - 3750.0).abs() < 37.5, "{}", tile.volume_mm3);
    assert_eq!(tile.name, "floor_2x2_mm");
    assert_eq!(tile.source_triangles, 12);
}
