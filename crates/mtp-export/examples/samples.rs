//! Writes a sample PNG, PDF and CSV of a small dungeon to the folder given
//! on the command line, for looking at the exports by eye.
//!
//! `cargo run -p mtp-export --example samples -- <folder>`

use std::collections::HashMap;
use std::path::PathBuf;

use mtp_core::document::DocState;
use mtp_core::footprint::Rotation;
use mtp_core::grid::GridSize;
use mtp_core::tile::{Placed, PlacedId, TileKey};
use mtp_core::units::TileSize;
use mtp_export::{ExportInput, Paper, PathMode, PdfOptions, TileMeta};

fn place(id: u64, path: &str, (w, h): (u32, u32), (x, y): (f32, f32), quarters: i64, z: f32, level: i32) -> Placed {
    Placed {
        id: PlacedId(id),
        key: TileKey::new(path),
        size: TileSize { w, h, gz: 0.48 },
        size_known: true,
        x,
        y,
        z,
        rotation: Rotation::from_quarters(quarters),
        level,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(std::env::args().nth(1).ok_or("give the folder to write to")?);
    std::fs::create_dir_all(&dir)?;

    let floor = "C:/Users/me/Terrain/OpenForge/floor_2x2.stl";
    let mut tiles = Vec::new();
    let mut id = 0;
    let mut next = || {
        id += 1;
        id
    };
    for gx in 0..4 {
        for gy in 0..3 {
            tiles.push(place(next(), floor, (2, 2), (2.0 + gx as f32 * 2.0, 2.0 + gy as f32 * 2.0), 0, 0.0, 0));
        }
    }
    for (i, q) in [0, 1, 2, 3].into_iter().enumerate() {
        tiles.push(place(
            next(),
            "C:/Users/me/Terrain/OpenForge/wall_door.stl",
            (2, 1),
            (12.0, 2.0 + i as f32 * 2.0),
            q,
            0.0,
            0,
        ));
    }
    tiles.push(place(next(), "C:/Users/me/Terrain/Dragonlock/floor_2x2.stl", (2, 2), (12.0, 10.0), 0, 0.0, 0));
    tiles.push(place(next(), "C:/Users/me/Terrain/OpenForge/stairs.stl", (2, 4), (16.0, 2.0), 1, 0.0, 0));
    tiles.push(place(next(), "C:/Users/me/Terrain/OpenForge/platform_1x1.stl", (1, 1), (4.0, 4.0), 0, 0.48, 0));
    let mut missing = place(next(), "C:/Users/me/Terrain/Lost/statue.stl", (1, 1), (8.0, 6.0), 0, 0.0, 0);
    missing.size = TileSize::PLACEHOLDER;
    missing.size_known = false;
    tiles.push(missing);
    tiles.push(place(next(), floor, (2, 2), (4.0, 4.0), 0, 0.0, 1));
    tiles.push(place(
        next(),
        "C:/Users/me/Terrain/OpenForge/a tile with a really quite long descriptive name.stl",
        (4, 2),
        (6.0, 4.0),
        0,
        0.0,
        1,
    ));
    let state = DocState { grid: GridSize { cols: 40, rows: 30 }, tiles, ground_image: None };

    let mut info = HashMap::new();
    let key = TileKey::new(floor);
    info.insert(key.clone(), TileMeta { volume_mm3: Some(9_400.0), ..TileMeta::from_key(&key) });
    let input = ExportInput { title: "Crypt of the Lich King".to_string(), state: &state, info: &info, level: None };

    mtp_export::export_pdf(&dir.join("sample.pdf"), &input, &PdfOptions::default())?;
    mtp_export::export_png(&dir.join("sample.png"), &ExportInput { level: Some(0), ..input.clone() })?;
    mtp_export::export_csv(&dir.join("sample.csv"), &input, PathMode::Relative)?;

    // A convention table: big enough for detail pages
    let mut big = Vec::new();
    for i in 0..60 {
        let (x, y) = ((i % 10) as f32 * 14.0, (i / 10) as f32 * 14.0);
        big.push(place(i + 1, floor, (2, 2), (x, y), 0, 0.0, 0));
        big.push(place(
            i + 1000,
            "C:/Users/me/Terrain/OpenForge/wall_door.stl",
            (2, 1),
            (x + 2.0, y),
            (i % 4) as i64,
            0.0,
            0,
        ));
    }
    let big = DocState { grid: GridSize { cols: 140, rows: 84 }, tiles: big, ground_image: None };
    let input = ExportInput { title: "Convention table".to_string(), state: &big, info: &info, level: None };
    mtp_export::export_pdf(
        &dir.join("sample-large.pdf"),
        &input,
        &PdfOptions { paper: Paper::Letter, ..Default::default() },
    )?;
    mtp_export::export_png(&dir.join("sample-large.png"), &input)?;
    println!("Wrote samples to {}", dir.display());
    Ok(())
}
