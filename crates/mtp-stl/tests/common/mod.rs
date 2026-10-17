#![allow(dead_code)]

use std::path::PathBuf;

use mtp_stl::Triangle;

pub fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/stl")).join(relative)
}

/// A 50 × 50 mm tile whose top is a gentle wave of `cells × cells` quads.
pub fn heightfield(cells: usize) -> Vec<Triangle> {
    let point = |i: usize, j: usize| -> [f32; 3] {
        let (x, y) = (i as f32 / cells as f32 * 50.0, j as f32 / cells as f32 * 50.0);
        [x, y, 3.0 + (x * 0.15).sin() * (y * 0.12).cos()]
    };
    let mut tris = Vec::with_capacity(cells * cells * 2);
    for i in 0..cells {
        for j in 0..cells {
            let (a, b, c, d) = (point(i, j), point(i + 1, j), point(i + 1, j + 1), point(i, j + 1));
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
    }
    tris
}

/// Encodes triangles as a binary STL.
pub fn binary_stl(tris: &[Triangle]) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for tri in tris {
        out.extend_from_slice(&[0u8; 12]);
        for v in tri.iter().flatten() {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&[0u8; 2]);
    }
    out
}
