//! `--self-test`: checks a built app end to end without a window, for the
//! release workflow and for users whose 3D view stays blank. It loads STL
//! files, lays out tiles, renders a frame on the GPU, and saves and reopens
//! a project.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use mtp_core::document::{DocState, Document};
use mtp_core::footprint::Rotation;
use mtp_core::grid::GridSize;
use mtp_core::rules::{Mode, Proposal};
use mtp_core::tile::TileKey;
use mtp_render::{Camera, DrawKind, DrawTile, Frame, Palette, Projection, Renderer};

use crate::settings::{APP_NAME, VERSION};

fn box_stl(dx: f32, dy: f32, dz: f32) -> Vec<u8> {
    let p = |i: usize| {
        [if i & 1 == 0 { 0.0 } else { dx }, if i & 2 == 0 { 0.0 } else { dy }, if i & 4 == 0 { 0.0 } else { dz }]
    };
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut out = vec![0u8; 80];
    out.extend_from_slice(&12u32.to_le_bytes());
    for q in quads {
        for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            out.extend_from_slice(&[0u8; 12]);
            for v in tri {
                for c in p(v) {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
            out.extend_from_slice(&[0, 0]);
        }
    }
    out
}

struct Report {
    text: String,
    ok: bool,
}

impl Report {
    fn check(&mut self, name: &str, result: Result<String, String>) -> bool {
        match result {
            Ok(detail) => {
                let _ = writeln!(self.text, "ok    {name}: {detail}");
                true
            }
            Err(e) => {
                self.ok = false;
                let _ = writeln!(self.text, "FAIL  {name}: {e}");
                false
            }
        }
    }
}

/// Runs every check, returning the report and whether all passed.
pub fn run(software: bool) -> (String, bool) {
    let mut r = Report {
        text: format!("{APP_NAME} {VERSION} self-test ({} {})\n", std::env::consts::OS, std::env::consts::ARCH),
        ok: true,
    };
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            r.check("temporary folder", Err(e.to_string()));
            return (r.text, false);
        }
    };

    // Loading, from a folder whose name glob-based scanners choke on
    let tiles_dir = dir.path().join("Tiles [Test]");
    let written = std::fs::create_dir_all(&tiles_dir)
        .and_then(|()| std::fs::write(tiles_dir.join("floor_2x2.stl"), box_stl(25.0, 25.0, 6.0)))
        .and_then(|()| std::fs::write(tiles_dir.join("wall_2x1.STL"), box_stl(25.0, 12.5, 50.0)));
    r.check("write test tiles", written.map(|()| "2 files".into()).map_err(|e| e.to_string()));
    let paths = mtp_stl::scan_folder(&tiles_dir, false).unwrap_or_default();
    let results = Mutex::new(Vec::new());
    mtp_stl::load_many(&paths, None, &AtomicBool::new(false), |_, res| {
        if let (Ok(tile), Ok(mut list)) = (res, results.lock()) {
            list.push(tile);
        }
    });
    let mut loaded = results.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    loaded.sort_by(|a, b| a.name.cmp(&b.name));
    let count = loaded.len();
    if !r.check(
        "load STL files",
        if count == 2 { Ok(format!("{count} tiles")) } else { Err(format!("loaded {count} of 2")) },
    ) {
        return (r.text, false);
    }

    // Layout: four floors and a wall that must land on the first floor
    let mut doc = Document::new(DocState::empty(GridSize { cols: 20, rows: 12 }));
    let proposal = |t: &mtp_stl::LoadedTile, x: f32, y: f32| Proposal {
        key: TileKey::from_path(&t.path),
        size: t.size,
        size_known: true,
        x,
        y,
        z: 0.0,
        rotation: Rotation::ZERO,
        level: 0,
    };
    let floors: Vec<Proposal> = (0..4).map(|i| proposal(&loaded[0], 2.0 + 2.0 * i as f32, 4.0)).collect();
    let placed = doc
        .place("Place", floors, Mode::Snap)
        .and_then(|_| doc.place("Place", vec![proposal(&loaded[1], 2.0, 4.0)], Mode::Snap));
    let wall_z = doc.tiles().last().map_or(0.0, |t| t.z);
    r.check(
        "place and stack tiles",
        match placed {
            Ok(_) if (wall_z - 0.48).abs() < 1e-3 => Ok("the wall sits on the floor".into()),
            Ok(_) => Err(format!("the wall is at height {wall_z}")),
            Err(e) => Err(e.to_string()),
        },
    );

    // Rendering
    match mtp_render::device::headless(software) {
        Err(e) => {
            r.check("graphics", Err(e.to_string()));
        }
        Ok((adapter, device, queue)) => {
            let info = adapter.get_info();
            r.check("graphics", Ok(format!("{} ({:?})", info.name, info.backend)));
            let result = render(Renderer::new(device, queue), &loaded, &doc);
            r.check("render a frame", result);
        }
    }

    // Project files
    let project = dir.path().join("Self test.mtp");
    r.check("save and reopen a project", save_and_reopen(&doc, &project));
    (r.text, r.ok)
}

fn render(mut renderer: Renderer, loaded: &[mtp_stl::LoadedTile], doc: &Document) -> Result<String, String> {
    for t in loaded {
        let key = TileKey::from_path(&t.path);
        renderer.register_mesh(key.clone(), t.lods.iter().map(|l| (l.indices.len() / 3) as u32).collect());
        renderer.upload_tier(&key, 0, &t.lods[0].positions, &t.lods[0].indices);
    }
    let draw: Vec<DrawTile> = doc
        .tiles()
        .iter()
        .map(|t| DrawTile {
            key: t.key.clone(),
            x: t.x,
            y: t.y,
            z: t.z,
            size: t.size,
            rotation: t.rotation,
            color: [220, 120, 40],
            selected: true,
            kind: DrawKind::Tile,
        })
        .collect();
    let camera = Camera::for_grid(20, 12, Projection::Perspective);
    let mut target = renderer.create_target([400, 240]);
    let stats = renderer.render(
        &mut target,
        &Frame {
            view_proj: camera.view_proj(400.0 / 240.0),
            grid: [20, 12],
            tiles: &draw,
            ghosts: &[],
            palette: Palette::DARK,
            pixels_per_point: 1.0,
            show_ground: true,
            show_grid: true,
            image_rect: None,
            lod_bias: 1.0,
        },
    );
    let rgba = renderer.read_output(&target);
    let tile_pixels = rgba.as_chunks::<4>().0.iter().filter(|p| p[0] > p[2].saturating_add(60)).count();
    if tile_pixels > 200 {
        Ok(format!("{} triangles in {} draw calls, {tile_pixels} tile pixels", stats.triangles, stats.draw_calls))
    } else {
        Err(format!("only {tile_pixels} tile pixels were drawn"))
    }
}

fn save_and_reopen(doc: &Document, path: &Path) -> Result<String, String> {
    let dir = crate::files::dir_of(path);
    let file = mtp_core::project::from_state(doc.state(), &[], &dir);
    let text = mtp_core::project::write(&file, VERSION);
    mtp_core::persist::write_atomic(path, text.as_bytes(), true).map_err(|e| e.to_string())?;
    let pending = crate::files::read_project(path)?;
    let state = crate::files::finish_open(&pending);
    if state.tiles == doc.state().tiles {
        let folder = pending.file.folders.first().map_or("", |f| f.path.as_str());
        Ok(format!("{} tiles, folder saved as \"{folder}\"", state.tiles.len()))
    } else {
        Err("the reopened tiles differ from the saved ones".into())
    }
}
