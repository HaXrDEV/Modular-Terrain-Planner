//! The CPU side of a frame: frustum culling, choosing a level of detail per
//! tile, and grouping instances into draw calls.

use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3, Vec4};
use mtp_core::footprint::{Rotation, effective_size};
use mtp_core::tile::TileKey;
use mtp_core::units::TileSize;

/// Triangles drawn per pixel a tile covers on screen. The Python version
/// used 2, which meant ~10 M triangles at 1080p and made integrated GPUs
/// crawl; at 0.25 the difference isn't visible.
pub const TRIANGLES_PER_PIXEL: f32 = 0.25;

/// How a tile is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawKind {
    Tile,
    /// Preview of a placement or move that would succeed.
    Ghost,
    /// Preview of one that would be refused: red and hatched.
    RefusedGhost,
    /// A tile on a level below the one being edited, this many levels down
    /// (1 or more). Drawn like a ghost, and a little further back for each
    /// level down, so levels in the same place don't fight.
    Faded(u8),
}

impl DrawKind {
    /// The shader's code: 0 tile, 1 ghost, 2 refused ghost, 3 placeholder,
    /// and 3 + levels down for faded tiles.
    fn code(self, placeholder: bool) -> f32 {
        match (self, placeholder) {
            (Self::Tile, false) => 0.0,
            (Self::Tile, true) => 3.0,
            (Self::Ghost, _) => 1.0,
            (Self::RefusedGhost, _) => 2.0,
            (Self::Faded(down), _) => 3.0 + f32::from(down.max(1)),
        }
    }
}

/// One tile to draw, as the app sees it.
#[derive(Clone, Debug)]
pub struct DrawTile {
    pub key: TileKey,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub size: TileSize,
    pub rotation: Rotation,
    pub color: [u8; 3],
    pub selected: bool,
    pub kind: DrawKind,
}

impl DrawTile {
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let (w, h) = effective_size(self.size, self.rotation);
        let min = Vec3::new(self.x, self.y, self.z);
        (min, min + Vec3::new(w as f32, h as f32, self.size.gz))
    }
}

/// Per-instance data for the vertex shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Instance {
    pub offset: [f32; 3],
    pub rot: f32,
    pub scale: [f32; 3],
    pub kind: f32,
    pub color: [f32; 4],
}

/// Which mesh a draw call uses.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MeshRef {
    Tier(TileKey, usize),
    /// The built-in box drawn for tiles whose STL isn't loaded.
    Placeholder,
}

#[derive(Clone, Debug)]
pub struct DrawGroup {
    pub mesh: MeshRef,
    pub start: u32,
    pub count: u32,
}

#[derive(Default, Debug)]
pub struct DrawList {
    pub instances: Vec<Instance>,
    pub tiles: Vec<DrawGroup>,
    pub ghosts: Vec<DrawGroup>,
    pub selection: Vec<DrawGroup>,
    /// Tiers that would have been better than what was drawn, finest
    /// wanted first, for the app to upload.
    pub wanted: Vec<(TileKey, usize)>,
}

/// What the renderer knows about a tile's levels of detail.
pub trait TierSource {
    /// Triangle counts of every tier, finest first; `None` if the mesh isn't known.
    fn tiers(&self, key: &TileKey) -> Option<&[u32]>;
    /// Whether a tier is on the GPU.
    fn resident(&self, key: &TileKey, tier: usize) -> bool;
    /// Whether a tier can go on the GPU at all (it fits in one buffer).
    fn usable(&self, _key: &TileKey, _tier: usize) -> bool {
        true
    }
}

/// Picks the tier whose triangle count is closest to what the tile's screen
/// size deserves, then the nearest one actually on the GPU (coarser first,
/// so a missing fine tier never costs more than wanted).
fn choose(
    tiers: &[u32],
    target: f32,
    usable: impl Fn(usize) -> bool,
    resident: impl Fn(usize) -> bool,
) -> (Option<usize>, Option<usize>) {
    let ideal = (0..tiers.len())
        .filter(|&t| usable(t))
        .min_by(|&a, &b| (tiers[a] as f32 - target).abs().total_cmp(&(tiers[b] as f32 - target).abs()));
    let Some(ideal) = ideal else { return (None, None) };
    let drawn = (ideal..tiers.len()).find(|&t| resident(t)).or_else(|| (0..ideal).rev().find(|&t| resident(t)));
    (drawn, Some(ideal))
}

/// Instances grouped by mesh, in the order each mesh first comes up, so
/// the draw order is the same every frame. Ghosts are translucent and
/// blend in draw order; a `HashMap`'s order changes from one map to the
/// next, which made overlapping faded levels flicker.
#[derive(Default)]
struct Groups<T> {
    order: Vec<(MeshRef, T)>,
    index: HashMap<MeshRef, usize>,
}

impl<T: Default> Groups<T> {
    fn entry(&mut self, mesh: MeshRef) -> &mut T {
        let i = *self.index.entry(mesh.clone()).or_insert_with(|| {
            self.order.push((mesh, T::default()));
            self.order.len() - 1
        });
        &mut self.order[i].1
    }
}

pub fn build(
    view_proj: Mat4,
    viewport: Vec2,
    tiles: &[DrawTile],
    ghosts: &[DrawTile],
    source: &impl TierSource,
    lod_bias: f32,
) -> DrawList {
    let planes = frustum_planes(&view_proj);
    let mut buckets: Groups<(Vec<Instance>, Vec<Instance>)> = Groups::default();
    let mut ghost_buckets: Groups<Vec<Instance>> = Groups::default();
    let mut wanted: HashSet<(TileKey, usize)> = HashSet::new();

    let mut classify = |t: &DrawTile| -> Option<(MeshRef, Instance)> {
        let (min, max) = t.bounds();
        if !aabb_visible(&planes, min, max) {
            return None;
        }
        let mesh = match source.tiers(&t.key) {
            None => MeshRef::Placeholder,
            Some(tiers) => {
                let target = screen_area(&view_proj, min, max, viewport) * TRIANGLES_PER_PIXEL * lod_bias;
                let (drawn, ideal) =
                    choose(tiers, target, |i| source.usable(&t.key, i), |i| source.resident(&t.key, i));
                if let Some(ideal) = ideal
                    && drawn != Some(ideal)
                {
                    wanted.insert((t.key.clone(), ideal));
                }
                drawn.map_or(MeshRef::Placeholder, |d| MeshRef::Tier(t.key.clone(), d))
            }
        };
        let placeholder = mesh == MeshRef::Placeholder;
        let rgb = match t.kind {
            DrawKind::RefusedGhost => [0.86, 0.22, 0.2],
            _ if placeholder => [0.6, 0.6, 0.62],
            _ => t.color.map(|c| f32::from(c) / 255.0),
        };
        let alpha = if t.kind == DrawKind::Tile { 1.0 } else { 0.5 };
        let instance = Instance {
            offset: [t.x, t.y, t.z],
            rot: t.rotation.radians(),
            scale: (max - min).to_array(),
            kind: t.kind.code(placeholder),
            color: [rgb[0], rgb[1], rgb[2], alpha],
        };
        Some((mesh, instance))
    };

    for t in tiles {
        if let Some((mesh, inst)) = classify(t) {
            let entry = buckets.entry(mesh);
            entry.0.push(inst);
            if t.selected {
                entry.1.push(inst);
            }
        }
    }
    for t in ghosts {
        if let Some((mesh, inst)) = classify(t) {
            ghost_buckets.entry(mesh).push(inst);
        }
    }

    let mut list = DrawList::default();
    let push = |out: &mut Vec<DrawGroup>, instances: &mut Vec<Instance>, mesh: MeshRef, items: Vec<Instance>| {
        if !items.is_empty() {
            out.push(DrawGroup { mesh, start: instances.len() as u32, count: items.len() as u32 });
            instances.extend(items);
        }
    };
    for (mesh, (all, selected)) in buckets.order {
        push(&mut list.tiles, &mut list.instances, mesh.clone(), all);
        push(&mut list.selection, &mut list.instances, mesh, selected);
    }
    for (mesh, items) in ghost_buckets.order {
        push(&mut list.ghosts, &mut list.instances, mesh, items);
    }
    let mut wanted: Vec<_> = wanted.into_iter().collect();
    wanted.sort_by_key(|(key, tier)| (*tier, key.clone()));
    list.wanted = wanted;
    list
}

fn frustum_planes(m: &Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2]
}

fn aabb_visible(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| {
        let v = Vec3::new(
            if p.x >= 0.0 { max.x } else { min.x },
            if p.y >= 0.0 { max.y } else { min.y },
            if p.z >= 0.0 { max.z } else { min.z },
        );
        p.truncate().dot(v) + p.w >= 0.0
    })
}

fn corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    })
}

/// Pixels a box covers on screen (a tile reaching behind the camera counts
/// as filling the screen).
pub fn screen_area(vp: &Mat4, min: Vec3, max: Vec3, viewport: Vec2) -> f32 {
    match screen_rect(vp, min, max, viewport) {
        Some((lo, hi)) => {
            let span = (hi.min(viewport) - lo.max(Vec2::ZERO)).max(Vec2::ZERO);
            span.x * span.y
        }
        None => viewport.x * viewport.y,
    }
}

/// The box's bounding rectangle in viewport pixels (y down), or `None` when
/// part of it is behind the camera.
pub fn screen_rect(vp: &Mat4, min: Vec3, max: Vec3, viewport: Vec2) -> Option<(Vec2, Vec2)> {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for c in corners(min, max) {
        let clip = *vp * c.extend(1.0);
        if clip.w <= 1e-5 {
            return None;
        }
        let ndc = clip.truncate().truncate() / clip.w;
        let px = Vec2::new((ndc.x + 1.0) * 0.5 * viewport.x, (1.0 - ndc.y) * 0.5 * viewport.y);
        lo = lo.min(px);
        hi = hi.max(px);
    }
    Some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Camera, Projection};

    struct Source(HashMap<TileKey, (Vec<u32>, Vec<bool>)>);

    impl TierSource for Source {
        fn tiers(&self, key: &TileKey) -> Option<&[u32]> {
            self.0.get(key).map(|(t, _)| t.as_slice())
        }
        fn resident(&self, key: &TileKey, tier: usize) -> bool {
            self.0.get(key).is_some_and(|(_, r)| r[tier])
        }
    }

    fn tile(key: &str, x: f32) -> DrawTile {
        DrawTile {
            key: TileKey::new(key),
            x,
            y: 0.0,
            z: 0.0,
            size: TileSize { w: 2, h: 2, gz: 0.5 },
            rotation: Rotation::ZERO,
            color: [200, 100, 50],
            selected: x == 0.0,
            kind: DrawKind::Tile,
        }
    }

    #[test]
    fn coarser_resident_tiers_stand_in() {
        let all = |_| true;
        assert_eq!(choose(&[1000, 100, 10], 900.0, all, |t| t == 2), (Some(2), Some(0)));
        assert_eq!(choose(&[1000, 100, 10], 12.0, all, |t| t == 0), (Some(0), Some(2)));
        assert_eq!(choose(&[1000, 100, 10], 12.0, all, |_| false), (None, Some(2)));
        // A tier too big for the GPU is never wanted
        assert_eq!(choose(&[1000, 100, 10], 900.0, |t| t > 0, |_| false), (None, Some(1)));
        assert_eq!(choose(&[1000], 900.0, |_| false, |_| false), (None, None));
    }

    #[test]
    fn groups_cull_and_ask_for_better_tiers() {
        let cam = Camera::for_grid(20, 20, Projection::TopDown);
        let vp = cam.view_proj(1.0);
        let source =
            Source(HashMap::from([(TileKey::new("a.stl"), (vec![50_000, 5_000, 500], vec![false, false, true]))]));
        let tiles = [tile("a.stl", 0.0), tile("a.stl", 4.0), tile("missing.stl", 8.0), tile("a.stl", 500.0)];
        let list = build(vp, Vec2::new(800.0, 800.0), &tiles, &[], &source, 1.0);
        let drawn: u32 = list.tiles.iter().map(|g| g.count).sum();
        assert_eq!(drawn, 3, "the tile far off the grid is culled");
        assert!(list.tiles.iter().any(|g| g.mesh == MeshRef::Placeholder));
        assert_eq!(list.selection.iter().map(|g| g.count).sum::<u32>(), 1);
        assert!(list.wanted.iter().all(|(k, _)| k == &TileKey::new("a.stl")));
    }

    #[test]
    fn groups_come_in_the_order_of_their_tiles() {
        // Ghosts are translucent and blend in draw order, so an order that
        // changed from frame to frame made overlapping faded levels flicker
        let cam = Camera::for_grid(20, 20, Projection::TopDown);
        let vp = cam.view_proj(1.0);
        let keys: Vec<String> = (0..8).map(|i| format!("k{i}.stl")).collect();
        let source = Source(keys.iter().map(|k| (TileKey::new(k), (vec![100], vec![true]))).collect());
        let tiles: Vec<DrawTile> = keys.iter().enumerate().map(|(i, k)| tile(k, 2.0 * i as f32)).collect();
        let ghosts: Vec<DrawTile> =
            tiles.iter().rev().map(|t| DrawTile { kind: DrawKind::Ghost, ..t.clone() }).collect();
        let list = build(vp, Vec2::new(800.0, 800.0), &tiles, &ghosts, &source, 1.0);
        let order = |groups: &[DrawGroup]| groups.iter().map(|g| g.mesh.clone()).collect::<Vec<_>>();
        let expected = |ts: &[DrawTile]| ts.iter().map(|t| MeshRef::Tier(t.key.clone(), 0)).collect::<Vec<_>>();
        assert_eq!(order(&list.tiles), expected(&tiles));
        assert_eq!(order(&list.ghosts), expected(&ghosts));
    }
}
