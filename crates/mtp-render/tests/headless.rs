//! Renders real frames on whatever GPU is there (a software one in CI), so a
//! shader that doesn't compile on a backend fails here instead of on users'
//! machines, like the Python version's macOS build did.
//!
//! Without an adapter the test is skipped, unless `MTP_REQUIRE_GPU=1` (set by
//! the CI GPU job). `MTP_SOFTWARE_GPU=1` asks for a CPU adapter.

use std::sync::{Mutex, MutexGuard, PoisonError};

use mtp_core::footprint::Rotation;
use mtp_core::tile::TileKey;
use mtp_core::units::TileSize;
use mtp_render::{Camera, DrawKind, DrawTile, Frame, Palette, Projection, Renderer, device};

/// The tests take turns on the GPU. On a Windows CI runner, two WARP
/// devices created at once on separate threads crashed the test process
/// (access violation) before either test had its device.
fn gpu_turn() -> MutexGuard<'static, ()> {
    static GPU: Mutex<()> = Mutex::new(());
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

fn renderer() -> Option<Renderer> {
    let software = std::env::var_os("MTP_SOFTWARE_GPU").is_some_and(|v| v == "1");
    match device::headless(software) {
        Ok((adapter, device, queue)) => {
            let info = adapter.get_info();
            eprintln!("adapter: {} ({:?})", info.name, info.backend);
            Some(Renderer::new(device, queue))
        }
        Err(e) if std::env::var_os("MTP_REQUIRE_GPU").is_some_and(|v| v == "1") => panic!("{e}"),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn cube() -> (Vec<[f32; 3]>, Vec<u32>) {
    let p: Vec<[f32; 3]> = (0..8).map(|i| [(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32]).collect();
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let idx = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
    (p, idx)
}

fn tile(key: &TileKey, x: f32, selected: bool, kind: DrawKind) -> DrawTile {
    DrawTile {
        key: key.clone(),
        x,
        y: 4.0,
        z: 0.0,
        size: TileSize { w: 2, h: 2, gz: 1.0 },
        rotation: Rotation::ZERO,
        color: [200, 120, 40],
        selected,
        kind,
    }
}

fn pixel(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn draws_tiles_ghosts_and_the_selection_outline() {
    let _turn = gpu_turn();
    let Some(mut r) = renderer() else { return };
    let key = TileKey::new("cube.stl");
    let (p, i) = cube();
    r.register_mesh(key.clone(), vec![12]);
    r.upload_tier(&key, 0, &p, &i);

    let tiles = [tile(&key, 2.0, true, DrawKind::Tile), tile(&TileKey::new("missing.stl"), 6.0, false, DrawKind::Tile)];
    let ghosts = [tile(&key, 10.0, false, DrawKind::RefusedGhost)];
    let camera = Camera::for_grid(16, 10, Projection::TopDown);
    let mut target = r.create_target([320, 200]);
    let stats = r.render(
        &mut target,
        &Frame {
            view_proj: camera.view_proj(1.6),
            grid: [16, 10],
            tiles: &tiles,
            ghosts: &ghosts,
            palette: Palette::DARK,
            pixels_per_point: 1.0,
            show_ground: true,
            show_grid: true,
            image_rect: None,
            lod_bias: 1.0,
        },
    );
    assert!(stats.draw_calls >= 5, "{stats:?}");
    let rgba = r.read_output(&target);
    assert_eq!(rgba.len(), 320 * 200 * 4);

    // Grid units to pixels: the camera shows 10 rows plus a margin
    let to_px = |x: f32, y: f32| {
        let p = camera.view_proj(1.6).project_point3(glam::Vec3::new(x, y, 1.0));
        (((p.x + 1.0) * 160.0) as u32, ((1.0 - p.y) * 100.0) as u32)
    };
    let (cx, cy) = to_px(3.0, 5.0);
    let lit = pixel(&rgba, 320, cx, cy);
    assert!(lit[0] > lit[2] + 40, "the orange tile is drawn: {lit:?}");

    let outline = Palette::DARK.outline.map(|c| (c * 255.0).round() as u8);
    let found_outline = rgba.as_chunks::<4>().0.iter().any(|p| p[..3] == outline[..3]);
    assert!(found_outline, "the selected tile has an outline");

    let (gx, gy) = to_px(11.0, 5.0);
    let ghost = pixel(&rgba, 320, gx, gy);
    assert!(ghost[0] > ghost[1] + 20, "the refused ghost is red: {ghost:?}");
}

/// Renders a 16 × 10 grid from above and returns the pixel over (x, y).
fn pixel_at(r: &mut Renderer, tiles: &[DrawTile], ghosts: &[DrawTile], x: f32, y: f32) -> [u8; 4] {
    let camera = Camera::for_grid(16, 10, Projection::TopDown);
    let mut target = r.create_target([320, 200]);
    let frame = Frame {
        view_proj: camera.view_proj(1.6),
        grid: [16, 10],
        tiles,
        ghosts,
        palette: Palette::DARK,
        pixels_per_point: 1.0,
        show_ground: true,
        show_grid: false,
        image_rect: None,
        lod_bias: 1.0,
    };
    r.render(&mut target, &frame);
    let p = camera.view_proj(1.6).project_point3(glam::Vec3::new(x, y, 1.0));
    pixel(&r.read_output(&target), 320, ((p.x + 1.0) * 160.0) as u32, ((1.0 - p.y) * 100.0) as u32)
}

#[test]
fn a_ghost_in_the_same_place_as_a_tile_stays_behind_it() {
    // A faded level is drawn where the level being edited is, so their
    // floors share a plane; without a bias the two fought pixel by pixel
    let _turn = gpu_turn();
    let Some(mut r) = renderer() else { return };
    let key = TileKey::new("cube.stl");
    let (p, i) = cube();
    r.register_mesh(key.clone(), vec![12]);
    r.upload_tier(&key, 0, &p, &i);
    let floor = [tile(&key, 2.0, false, DrawKind::Tile)];
    let blue = DrawTile { color: [40, 120, 220], ..tile(&key, 2.0, false, DrawKind::Ghost) };
    let alone = pixel_at(&mut r, &floor, &[], 3.0, 5.0);
    assert_eq!(pixel_at(&mut r, &floor, &[blue], 3.0, 5.0), alone);
}

#[test]
fn overlapping_faded_levels_look_the_same_in_any_order() {
    // See-through tiles used to blend in draw order, and the order changed
    // as the camera moved, so overlaps changed colour
    let _turn = gpu_turn();
    let Some(mut r) = renderer() else { return };
    let key = TileKey::new("cube.stl");
    let (p, i) = cube();
    r.register_mesh(key.clone(), vec![12]);
    r.upload_tier(&key, 0, &p, &i);
    let below = DrawTile { color: [220, 60, 60], ..tile(&key, 2.0, false, DrawKind::Faded(1)) };
    let further = DrawTile { color: [60, 60, 220], ..tile(&key, 3.0, false, DrawKind::Faded(2)) };
    let one_way = pixel_at(&mut r, &[], &[below.clone(), further.clone()], 3.5, 5.0);
    let other_way = pixel_at(&mut r, &[], &[further, below], 3.5, 5.0);
    assert_eq!(one_way, other_way);
    assert!(one_way[0] > one_way[2], "the level right below is in front: {one_way:?}");
}

#[test]
fn thumbnails_render_the_finest_resident_tier() {
    let _turn = gpu_turn();
    let Some(mut r) = renderer() else { return };
    let key = TileKey::new("cube.stl");
    let (p, i) = cube();
    r.register_mesh(key.clone(), vec![12]);
    r.upload_tier(&key, 0, &p, &i);
    let mut target = r.create_target([96, 96]);
    assert!(r.render_thumbnail(&mut target, &tile(&key, 0.0, false, DrawKind::Tile), Palette::LIGHT));
    let rgba = r.read_output(&target);
    let centre = pixel(&rgba, 96, 48, 48);
    assert_ne!(centre[..3], Palette::LIGHT.background.map(|c| (c * 255.0).round() as u8));
}
