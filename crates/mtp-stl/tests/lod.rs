mod common;

use std::path::Path;
use std::time::Instant;

use mtp_stl::{LoadedTile, parse_stl};

#[test]
fn dense_mesh_gets_a_shrinking_chain() {
    let bytes = common::binary_stl(&common::heightfield(200));
    let start = Instant::now();
    let triangles = parse_stl(&bytes).expect("synthetic STL parses");
    let parsed = start.elapsed();
    let tile = LoadedTile::from_triangles(Path::new("heightfield.stl"), &triangles).expect("tile builds");
    let built = start.elapsed() - parsed;

    let counts: Vec<usize> = tile.lods.iter().map(|m| m.triangle_count()).collect();
    println!("80k heightfield: parse {parsed:?}, size + weld + LOD {built:?}, tiers {counts:?}");

    assert_eq!(counts[0], 80_000);
    assert!(counts.len() <= 6);
    assert!(counts.windows(2).all(|w| w[1] < w[0]), "{counts:?}");
    let coarsest = counts.last().copied().expect("at least one tier");
    assert!(coarsest * 100 < counts[0] * 3, "{counts:?}");

    for mesh in &tile.lods {
        assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.positions.len()));
        let (lo, hi) = mesh.bounds().expect("tier has triangles");
        for axis in 0..3 {
            assert!(lo[axis] >= 0.0 && hi[axis] <= 1.0, "{lo:?} {hi:?}");
        }
        // Borders stay put, so the footprint doesn't shrink
        assert_eq!((lo[0], lo[1], hi[0], hi[1]), (0.0, 0.0, 1.0, 1.0));
    }
}

#[test]
fn simple_box_has_one_tier() {
    let box_tile = mtp_stl::load_tile(&common::fixture("good/floor_2x2_mm.stl"), None).expect("box loads");
    assert_eq!(box_tile.lods.len(), 1);
}
