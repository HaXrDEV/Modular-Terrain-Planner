//! Project files: every version the Python app wrote reads as it read them,
//! damaged files don't crash, and the new format keeps everything.

use mtp_core::document::Document;
use mtp_core::project::{self, LibraryFolder, ProjectError};
use mtp_core::tile::TileKey;
use serde_json::Value;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/../../tests/fixtures/projects/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn golden() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/python.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("golden file")).expect("golden JSON")
}

#[test]
fn old_versions_read_like_the_python_app() {
    let g = golden();
    let projects = g["projects"].as_object().expect("projects");
    assert_eq!(projects.len(), 6);
    for (name, want) in projects {
        let file = project::read(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(file.warnings.is_empty(), "{name}: {:?}", file.warnings);
        assert_eq!(file.grid.cols as u64, want["grid_cols"].as_u64().expect("cols"), "{name}");
        assert_eq!(file.grid.rows as u64, want["grid_rows"].as_u64().expect("rows"), "{name}");

        // Every saved folder is still there, in order (tiles may add their own)
        let folders: Vec<String> = file.folders.iter().map(|f| f.path.clone()).collect();
        for (i, f) in want["folders"].as_array().expect("folders").iter().enumerate() {
            assert_eq!(TileKey::new(&folders[i]), TileKey::new(f.as_str().expect("path")), "{name}");
        }

        // Nothing found on disk: tiles keep the paths they were saved with
        let found = vec![None; file.folders.len()];
        let state = project::to_state(&file, "C:/Projects", &found, None);
        let tiles = want["tiles"].as_array().expect("tiles");
        assert_eq!(state.tiles.len(), tiles.len(), "{name}");
        for (t, w) in state.tiles.iter().zip(tiles) {
            assert_eq!(t.key, TileKey::new(w["stl_path"].as_str().expect("path")), "{name}");
            assert_eq!(f64::from(t.x), w["grid_x"].as_f64().expect("x"), "{name}");
            assert_eq!(f64::from(t.y), w["grid_y"].as_f64().expect("y"), "{name}");
            assert!((f64::from(t.z) - w["z_offset"].as_f64().expect("z")).abs() < 1e-6, "{name}");
            assert_eq!(u64::from(t.rotation.degrees()), w["rotation"].as_u64().expect("rotation"), "{name}");
            assert!(!t.size_known, "the Python format stored no sizes");
        }

        match (&state.ground_image, want["ground_image"].as_object()) {
            (None, None) => {}
            (Some(img), Some(w)) => {
                assert_eq!(img.path, w["path"].as_str().expect("path"));
                let rect: Vec<f32> =
                    w["rect"].as_array().expect("rect").iter().map(|v| v.as_f64().expect("n") as f32).collect();
                assert_eq!(img.rect.to_vec(), rect);
            }
            other => panic!("{name}: ground image {other:?}"),
        }
    }
}

#[test]
fn overlapping_tiles_survive_open_and_save() {
    // v4.mtp has two tiles in the same place: the Python app dropped one on open
    let file = project::read(&fixture("v4.mtp")).expect("v4");
    let state = project::to_state(&file, "C:/Projects", &vec![None; file.folders.len()], None);
    let doc = Document::new(state.clone());
    assert_eq!(doc.tiles().len(), 2);

    let saved = project::from_state(doc.state(), &[], "C:/Projects");
    let text = project::write(&saved, "0.5.0");
    let again = project::read(text.as_bytes()).expect("v5");
    let restored = project::to_state(&again, "C:/Projects", &vec![None; again.folders.len()], None);
    assert_eq!(restored.tiles, state.tiles);
    assert_eq!(restored.ground_image, state.ground_image);
}

#[test]
fn paths_move_with_the_project() {
    let file = project::read(&fixture("v3.mtp")).expect("v3");
    let state = project::to_state(&file, "C:/Projects", &vec![None; file.folders.len()], None);
    let open = [LibraryFolder { path: "C:/Tiles/OpenForge".into(), recursive: false }];
    let saved = project::from_state(&state, &open, "C:/Maps/Castle");
    assert_eq!(saved.folders[0].path, "../../Tiles/OpenForge");
    assert_eq!(saved.folders[0].absolute.as_deref(), Some("C:/Tiles/OpenForge"));
    // "D:/More Tiles" is on another drive, so it stays absolute
    assert_eq!(saved.folders[1].path, "D:/More Tiles");

    // Moved together to a Linux machine: found through the relative path
    let reread = project::read(project::write(&saved, "0.5.0").as_bytes()).expect("v5");
    let exists = |p: &str| p == "/home/me/Tiles/OpenForge";
    let found = project::locate("/home/me/Maps/Castle", &reread.folders[0], exists);
    assert_eq!(found.as_deref(), Some("/home/me/Tiles/OpenForge"));
}

#[test]
fn a_remapped_folder_moves_its_tiles() {
    let file = project::read(&fixture("v1.mtp")).expect("v1");
    let found = vec![Some("E:/Library/OpenForge".to_owned())];
    let state = project::to_state(&file, "C:/Projects", &found, None);
    assert_eq!(state.tiles[0].key, TileKey::new("E:/Library/OpenForge/floor.stl"));
}

#[test]
fn damaged_files_are_refused_or_repaired_without_crashing() {
    assert_eq!(project::read(&fixture("bad/newer_version.mtp")), Err(ProjectError::TooNew { version: 99 }));
    assert!(matches!(project::read(&fixture("bad/not_json.mtp")), Err(ProjectError::NotJson(_))));

    let missing = project::read(&fixture("bad/missing_field.mtp")).expect("readable");
    assert!(missing.tiles.is_empty());
    assert_eq!(missing.warnings.len(), 1);

    let wrong = project::read(&fixture("bad/wrong_types.mtp")).expect("readable");
    assert_eq!(wrong.version, 4);
    assert!(wrong.folders.is_empty());
    assert_eq!(wrong.warnings.len(), 2, "{:?}", wrong.warnings);

    let odd = project::read(&fixture("bad/odd_values.mtp")).expect("readable");
    let turns: Vec<u16> = odd.tiles.iter().map(|t| t.rotation.degrees()).collect();
    assert_eq!(turns, [90, 90], "45° snaps, NaN position is left out, 450° is 90°");
    assert!(odd.ground_image.is_some(), "a battle map with a bad size is kept, to be placed again");
    assert_eq!(odd.warnings.len(), 3, "{:?}", odd.warnings);
}
