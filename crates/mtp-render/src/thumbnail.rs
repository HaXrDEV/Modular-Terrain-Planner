//! Library thumbnails: one tile rendered on its own into a small target.

use glam::{Mat4, Vec3};

use crate::renderer::{Frame, Palette, Renderer, Target};
use crate::scene::DrawTile;

/// View that frames a tile from the front-left, slightly above.
pub fn view_for(tile: &DrawTile) -> Mat4 {
    let (min, max) = tile.bounds();
    let centre = (min + max) * 0.5;
    let radius = ((max - min).length() * 0.5).max(0.1);
    let eye = centre + Vec3::new(-0.9, -1.4, 1.1).normalize() * radius * 2.7;
    Mat4::perspective_rh(36f32.to_radians(), 1.0, radius * 0.05, radius * 10.0) * Mat4::look_at_rh(eye, centre, Vec3::Z)
}

impl Renderer {
    /// Draws `tile` alone into `target` (its finest tier on the GPU, or the
    /// placeholder box). Returns whether a real mesh was drawn rather than
    /// the placeholder.
    pub fn render_thumbnail(&self, target: &mut Target, tile: &DrawTile, palette: Palette) -> bool {
        let tiles = std::slice::from_ref(tile);
        let frame = Frame {
            view_proj: view_for(tile),
            grid: [0, 0],
            tiles,
            ghosts: &[],
            palette: Palette { outline: [0.0; 4], ..palette },
            pixels_per_point: 1.0,
            show_ground: false,
            show_grid: false,
            image_rect: None,
            // Thumbnails want the finest tier that is resident
            lod_bias: 1e6,
        };
        let stats = self.render(target, &frame);
        crate::scene::TierSource::tiers(self, &tile.key).is_some() && stats.wanted.is_empty()
    }
}
