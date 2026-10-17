//! A small sample library made by the app itself: stone floors, brick walls
//! and a pillar, sized like OpenForge-style tiles. It gives a first run
//! something to place, and the README screenshots tiles nobody owns.
//! The generated files are free to use for anything (CC0).

use std::path::{Path, PathBuf};

struct Sample {
    file: &'static str,
    /// Size in millimetres.
    size: [f32; 3],
    relief: Relief,
    detail: u32,
}

#[derive(Clone, Copy)]
enum Relief {
    Floor,
    Wall,
    Pillar,
}

const SAMPLES: [Sample; 6] = [
    Sample { file: "Stone floor 2x2.stl", size: [25.0, 25.0, 6.0], relief: Relief::Floor, detail: 96 },
    Sample { file: "Stone floor 1x1.stl", size: [12.5, 12.5, 6.0], relief: Relief::Floor, detail: 48 },
    Sample { file: "Stone floor 4x2.stl", size: [50.0, 25.0, 6.0], relief: Relief::Floor, detail: 128 },
    Sample { file: "Brick wall 2x1.stl", size: [25.0, 12.5, 50.0], relief: Relief::Wall, detail: 96 },
    Sample { file: "Brick wall 4x1.stl", size: [50.0, 12.5, 50.0], relief: Relief::Wall, detail: 128 },
    Sample { file: "Pillar 1x1.stl", size: [12.5, 12.5, 56.0], relief: Relief::Pillar, detail: 48 },
];

type V3 = [f32; 3];
/// Origin, two edge directions, the inward normal and an optional relief
/// (depth, columns, rows).
type Face = (V3, V3, V3, V3, Option<(f32, f32, f32)>);

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash(a: f32, b: f32) -> f32 {
    ((a * 127.1 + b * 311.7).sin() * 43_758.547).fract().abs()
}

/// Groove pattern: 1 on a stone, 0 in the mortar, stones a little uneven.
fn stones(u: f32, v: f32, cols: f32, rows: f32) -> f32 {
    let row = (v * rows).floor();
    let su = u * cols + if row as i32 % 2 == 0 { 0.0 } else { 0.5 };
    let (fu, fv) = (su.fract(), (v * rows).fract());
    let edge = fu.min(1.0 - fu).min(fv.min(1.0 - fv));
    smoothstep(0.0, 0.08, edge) * (0.8 + 0.2 * hash(su.floor(), row))
}

/// Triangles of a unit cube whose faces carry a relief, scaled to `size`.
fn mesh(sample: &Sample) -> Vec<[V3; 3]> {
    let n = sample.detail;
    let [sx, sy, sz] = sample.size;
    // Relief depth in mm, turned into unit-cube units per face direction
    let (top, sides, all_round) = match sample.relief {
        Relief::Floor => (Some((1.2 / sz, (sx / 6.25).max(2.0), (sy / 6.25).max(2.0))), None, false),
        Relief::Wall => (None, Some((1.6 / sy, (sx / 6.25).max(2.0), sz / 6.25)), false),
        Relief::Pillar => (None, Some((1.2 / sy, 2.0, sz / 6.25)), true),
    };
    // (origin, u, v, inward normal, relief)
    let faces: [Face; 6] = [
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0], top),
        ([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], None),
        ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], sides),
        ([1.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0], sides),
        ([0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], if all_round { sides } else { None }),
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], if all_round { sides } else { None }),
    ];
    let mut tris = Vec::new();
    for (origin, u, v, inward, relief) in faces {
        let sub = if relief.is_some() { n } else { 1 };
        let point = |i: u32, j: u32| {
            let (fu, fv) = (i as f32 / sub as f32, j as f32 / sub as f32);
            let depth = relief.map_or(0.0, |(d, cols, rows)| d * (1.0 - stones(fu, fv, cols, rows)));
            let p = add(add(origin, scale(u, fu)), add(scale(v, fv), scale(inward, depth)));
            [p[0] * sx, p[1] * sy, p[2] * sz]
        };
        for j in 0..sub {
            for i in 0..sub {
                let (a, b, c, d) = (point(i, j), point(i + 1, j), point(i + 1, j + 1), point(i, j + 1));
                tris.push([a, b, c]);
                tris.push([a, c, d]);
            }
        }
    }
    tris
}

fn binary_stl(tris: &[[V3; 3]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + tris.len() * 50);
    let mut header = [b' '; 80];
    let text = b"Modular Terrain Planner sample tile (CC0)";
    header[..text.len()].copy_from_slice(text);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in tris {
        out.extend_from_slice(&[0u8; 12]);
        for v in t {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

/// Writes the sample tiles into `dir` (skipping ones already there) and
/// returns the folder.
pub fn write(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for s in &SAMPLES {
        let path = dir.join(s.file);
        if !path.exists() {
            mtp_core::persist::write_atomic(&path, &binary_stl(&mesh(s)), false)?;
        }
    }
    std::fs::write(
        dir.join("About these tiles.txt"),
        "Sample tiles made by Modular Terrain Planner. They are free to use for anything (CC0).\n",
    )?;
    Ok(dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_load_with_the_right_footprints() {
        let dir = tempfile::tempdir().expect("temp dir");
        write(dir.path()).expect("write");
        let mut sizes = Vec::new();
        for s in &SAMPLES {
            let tile = mtp_stl::load_tile(&dir.path().join(s.file), None).expect("loads");
            sizes.push((s.file, tile.size.w, tile.size.h));
        }
        assert_eq!(
            sizes,
            [
                ("Stone floor 2x2.stl", 2, 2),
                ("Stone floor 1x1.stl", 1, 1),
                ("Stone floor 4x2.stl", 4, 2),
                ("Brick wall 2x1.stl", 2, 1),
                ("Brick wall 4x1.stl", 4, 1),
                ("Pillar 1x1.stl", 1, 1),
            ]
        );
    }
}
