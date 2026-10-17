//! Which tile is under the pointer: a box test over every tile (9 µs per ray
//! for 5,000 tiles in the stack spike, so no tree is needed), then an exact
//! triangle test on the candidates, nearest box first.

use glam::{Mat4, Vec3};
use mtp_core::footprint::model_matrix;

use crate::scene::DrawTile;

/// A mesh in the unit cube, as positions and triangle indices.
#[derive(Clone, Copy, Debug)]
pub struct MeshView<'a> {
    pub positions: &'a [[f32; 3]],
    pub indices: &'a [u32],
}

/// Distance along the ray to a box, if it hits.
pub fn ray_box(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let inv = dir.recip();
    let t0 = (min - origin) * inv;
    let t1 = (max - origin) * inv;
    let near = t0.min(t1).max_element();
    let far = t0.max(t1).min_element();
    (near <= far && far >= 0.0).then_some(near.max(0.0))
}

/// Möller-Trumbore: distance to a triangle, if the ray hits it from either side.
fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

/// The nearest tile hit by a world ray, with the distance. `mesh` gives the
/// mesh to test exactly; a tile without one is picked by its box.
pub fn pick<'a>(
    tiles: &[DrawTile],
    origin: Vec3,
    dir: Vec3,
    mesh: impl Fn(&DrawTile) -> Option<MeshView<'a>>,
) -> Option<(usize, f32)> {
    let mut candidates: Vec<(usize, f32)> = tiles
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let (min, max) = t.bounds();
            ray_box(origin, dir, min, max).map(|near| (i, near))
        })
        .collect();
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));

    let mut best: Option<(usize, f32)> = None;
    for (i, near) in candidates {
        if best.is_some_and(|(_, t)| near > t) {
            break;
        }
        let t = &tiles[i];
        let hit = match mesh(t) {
            None => Some(near),
            Some(m) => {
                let model = Mat4::from_cols_array(&model_matrix(t.x, t.y, t.z, t.size, t.rotation));
                let inv = model.inverse();
                let lo = inv.transform_point3(origin);
                let ld = inv.transform_vector3(dir);
                // Local distances are along `ld`; convert hits back to world distances
                m.indices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .filter_map(|tri| {
                        let [a, b, c] = tri.map(|i| Vec3::from(m.positions[i as usize]));
                        ray_triangle(lo, ld, a, b, c)
                    })
                    .map(|lt| model.transform_point3(lo + ld * lt).distance(origin))
                    .min_by(f32::total_cmp)
            }
        };
        if let Some(d) = hit
            && best.is_none_or(|(_, b)| d < b)
        {
            best = Some((i, d));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::DrawKind;
    use mtp_core::footprint::Rotation;
    use mtp_core::tile::TileKey;
    use mtp_core::units::TileSize;

    fn tile(x: f32, gz: f32) -> DrawTile {
        DrawTile {
            key: TileKey::new("t.stl"),
            x,
            y: 0.0,
            z: 0.0,
            size: TileSize { w: 1, h: 1, gz },
            rotation: Rotation::ZERO,
            color: [0; 3],
            selected: false,
            kind: DrawKind::Tile,
        }
    }

    /// A thin vertical wall standing at x = 0.5 inside the unit cube.
    const WALL_POS: [[f32; 3]; 4] = [[0.5, 0.0, 0.0], [0.5, 1.0, 0.0], [0.5, 1.0, 1.0], [0.5, 0.0, 1.0]];
    const WALL_IDX: [u32; 6] = [0, 1, 2, 0, 2, 3];

    #[test]
    fn picks_the_nearest_box() {
        let tiles = [tile(0.0, 1.0), tile(5.0, 1.0)];
        let hit = pick(&tiles, Vec3::new(5.5, 0.5, 10.0), Vec3::NEG_Z, |_| None);
        assert_eq!(hit.map(|h| h.0), Some(1));
    }

    #[test]
    fn exact_test_sees_through_empty_parts_of_the_box() {
        // Looking down past the thin wall in tile 0 onto the tile behind it
        let tiles = [tile(0.0, 4.0), tile(0.0, 1.0)];
        let wall = MeshView { positions: &WALL_POS, indices: &WALL_IDX };
        let only_first = |t: &DrawTile| (t.size.gz > 2.0).then_some(wall);
        let hit = pick(&tiles, Vec3::new(0.2, 0.5, 10.0), Vec3::NEG_Z, only_first);
        assert_eq!(hit.map(|h| h.0), Some(1));
        // Aiming at the wall itself from the side hits it
        let side = pick(&tiles[..1], Vec3::new(-5.0, 0.5, 2.0), Vec3::X, only_first).expect("hit");
        assert!((side.1 - 5.5).abs() < 1e-4);
    }
}
