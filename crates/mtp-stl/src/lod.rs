//! The chain of simplified tiers drawn for tiles that cover few pixels.
//!
//! Each tier is simplified from the one before it with meshoptimizer, which
//! needs the welded mesh to collapse edges. Tiers are chosen by screen
//! coverage, so each one's error limit is relative to the tile's size.

use meshopt::{SimplifyOptions, VertexDataAdapter};

use crate::mesh::IndexedMesh;

/// Most tiers a tile gets, the full mesh included.
pub const MAX_TIERS: usize = 6;
/// No tier goes below this many triangles: a box needs 12.
pub const MIN_TRIANGLES: usize = 12;
/// Share of the source triangles each tier after the first aims for.
const TIER_TARGETS: [f64; MAX_TIERS - 1] = [0.40, 0.15, 0.06, 0.025, 0.01];
/// Largest deviation each tier may add, as a share of the tile's largest
/// side. Without it a tier could eat a wall's silhouette to reach its target.
const TIER_ERRORS: [f32; MAX_TIERS - 1] = [0.002, 0.005, 0.01, 0.02, 0.04];
/// A tier must have at most this share of the previous tier's triangles, or
/// the chain stops: near-copies only cost memory and uploads.
const MIN_REDUCTION: f64 = 0.85;

/// Builds the chain, finest first. Tier 0 is `full` itself.
///
/// `extent_mm` scales the normalised positions back to the tile's real
/// proportions while simplifying, so a flattened axis isn't over-weighted.
pub fn build_lods(full: IndexedMesh, extent_mm: [f64; 3]) -> Vec<IndexedMesh> {
    let source = full.triangle_count();
    let real: Vec<[f32; 3]> =
        full.positions.iter().map(|p| std::array::from_fn(|a| (f64::from(p[a]) * extent_mm[a]) as f32)).collect();
    let Ok(adapter) = VertexDataAdapter::new(bytemuck::cast_slice(&real), std::mem::size_of::<[f32; 3]>(), 0) else {
        return vec![full];
    };

    let mut previous = full.indices.clone();
    let mut tiers = vec![full];
    for (share, error) in TIER_TARGETS.into_iter().zip(TIER_ERRORS) {
        let previous_count = previous.len() / 3;
        let target = ((source as f64 * share).round() as usize).max(MIN_TRIANGLES);
        if target as f64 > previous_count as f64 * MIN_REDUCTION {
            break;
        }
        let indices = meshopt::simplify(&previous, &adapter, target * 3, error, SimplifyOptions::None, None);
        let count = indices.len() / 3;
        if count == 0 || count as f64 > previous_count as f64 * MIN_REDUCTION {
            break;
        }
        tiers.push(IndexedMesh::compacted(&tiers[0].positions, &indices));
        previous = indices;
    }
    tiers
}
