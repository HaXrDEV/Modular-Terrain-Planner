//! Sizing, normalising and welding a triangle soup.
//!
//! Tiles are stored in a unit cube and scaled to their footprint when drawn,
//! as the Python version did, so every tier of every tile shares one layout.

use std::collections::HashMap;
use std::hash::BuildHasherDefault;

use mtp_core::hash::Fnv1a;
use mtp_core::units::{SourceUnit, TileSize};

use crate::parse::Triangle;

/// Triangles that share corners through an index buffer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexedMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl IndexedMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Smallest and largest corner of the vertices the indices use.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut used = self.indices.iter().map(|&i| self.positions[i as usize]);
        let first = used.next()?;
        Some(used.fold((first, first), |(lo, hi), p| {
            (std::array::from_fn(|a| lo[a].min(p[a])), std::array::from_fn(|a| hi[a].max(p[a])))
        }))
    }

    /// Copies out only the vertices `indices` uses, so each tier uploads no
    /// more than it draws.
    pub fn compacted(positions: &[[f32; 3]], indices: &[u32]) -> Self {
        let mut remap = vec![u32::MAX; positions.len()];
        let mut out = Self { positions: Vec::new(), indices: Vec::with_capacity(indices.len()) };
        for &i in indices {
            let slot = &mut remap[i as usize];
            if *slot == u32::MAX {
                *slot = out.positions.len() as u32;
                out.positions.push(positions[i as usize]);
            }
            out.indices.push(*slot);
        }
        out
    }
}

/// What sizing and welding learn about a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub unit: SourceUnit,
    pub size: TileSize,
    /// Bounding box size in millimetres.
    pub extent_mm: [f64; 3],
    /// Enclosed volume in mm³, for filament estimates. Only meaningful for
    /// closed meshes, which printable tiles are.
    pub volume_mm3: f64,
    /// The welded mesh, normalised to `[0, 1]` on every axis.
    pub mesh: IndexedMesh,
}

/// Sizes the tile and welds its corners. `triangles` must not be empty.
pub fn prepare(triangles: &[Triangle]) -> Prepared {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for corner in triangles.iter().flatten() {
        for a in 0..3 {
            min[a] = min[a].min(corner[a]);
            max[a] = max[a].max(corner[a]);
        }
    }
    let extent: [f64; 3] = std::array::from_fn(|a| f64::from(max[a]) - f64::from(min[a]));
    let unit = SourceUnit::detect(extent[0], extent[1]);
    let size = TileSize::from_extent(extent[0], extent[1], extent[2], unit);
    let extent_mm = extent.map(|e| e * unit.scale());

    // A flat axis divides by 1 so it stays at 0 instead of turning into NaN
    let divisor = extent.map(|e| if e > 0.0 { e } else { 1.0 });
    let normalise = |p: &[f32; 3]| -> [f32; 3] {
        std::array::from_fn(|a| ((f64::from(p[a]) - f64::from(min[a])) / divisor[a]) as f32)
    };

    let mesh = weld(triangles, normalise);
    let volume_mm3 = volume(&mesh, extent_mm);
    Prepared { unit, size, extent_mm, volume_mm3, mesh }
}

/// Merges corners with identical normalised positions and drops triangles
/// that collapse to a line or point.
fn weld(triangles: &[Triangle], normalise: impl Fn(&[f32; 3]) -> [f32; 3]) -> IndexedMesh {
    let mut lookup: HashMap<[u32; 3], u32, BuildHasherDefault<Fnv1a>> =
        HashMap::with_capacity_and_hasher(triangles.len(), BuildHasherDefault::default());
    let mut mesh = IndexedMesh { positions: Vec::new(), indices: Vec::with_capacity(triangles.len() * 3) };
    for tri in triangles {
        let ids = tri.each_ref().map(|corner| {
            let p = normalise(corner);
            // -0.0 and 0.0 are the same place
            let key = p.map(|v| (v + 0.0).to_bits());
            *lookup.entry(key).or_insert_with(|| {
                mesh.positions.push(p);
                (mesh.positions.len() - 1) as u32
            })
        });
        if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
            mesh.indices.extend_from_slice(&ids);
        }
    }
    mesh
}

/// Divergence theorem: the sum of signed tetrahedra from the origin.
fn volume(mesh: &IndexedMesh, extent_mm: [f64; 3]) -> f64 {
    let mm = |i: u32| -> [f64; 3] {
        let p = mesh.positions[i as usize];
        std::array::from_fn(|a| f64::from(p[a]) * extent_mm[a])
    };
    let sum: f64 = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let (a, b, c) = (mm(t[0]), mm(t[1]), mm(t[2]));
            let cross = [b[1] * c[2] - b[2] * c[1], b[2] * c[0] - b[0] * c[2], b[0] * c[1] - b[1] * c[0]];
            a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]
        })
        .sum();
    sum.abs() / 6.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welds_and_drops_degenerates() {
        let tris = [
            [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            [[10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]],
            [[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [10.0, 10.0, 0.0]],
        ];
        let prepared = prepare(&tris);
        assert_eq!(prepared.mesh.positions.len(), 4);
        assert_eq!(prepared.mesh.triangle_count(), 2);
        assert_eq!(prepared.mesh.bounds(), Some(([0.0, 0.0, 0.0], [1.0, 1.0, 0.0])));
        assert_eq!(prepared.volume_mm3, 0.0);
    }

    #[test]
    fn compaction_keeps_only_used_vertices() {
        let positions = [[0.0; 3], [1.0; 3], [2.0; 3], [3.0; 3]];
        let mesh = IndexedMesh::compacted(&positions, &[3, 1, 3]);
        assert_eq!(mesh.positions, vec![[3.0; 3], [1.0; 3]]);
        assert_eq!(mesh.indices, vec![0, 1, 0]);
    }
}
