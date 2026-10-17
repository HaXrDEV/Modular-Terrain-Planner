//! The view: an orbiting perspective camera for checking a layout in 3D, and
//! a top-down orthographic one for laying it out in 2D.

use glam::{Mat4, Vec2, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// Straight down, no perspective: the default for laying out.
    TopDown,
    Perspective,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub projection: Projection,
    /// The point the camera looks at and orbits.
    pub target: Vec3,
    /// Degrees around +Z, from +X.
    pub azimuth: f32,
    /// Degrees above the ground (perspective only).
    pub elevation: f32,
    /// Distance from the target in perspective; half the visible height in
    /// cells top-down.
    pub distance: f32,
}

pub const FOV_Y_DEGREES: f32 = 45.0;
const MIN_DISTANCE: f32 = 1.0;
const MAX_DISTANCE: f32 = 4000.0;

impl Camera {
    /// Looking at the middle of a grid, all of it in view.
    pub fn for_grid(cols: u32, rows: u32, projection: Projection) -> Self {
        let mut cam = Self { projection, target: Vec3::ZERO, azimuth: -90.0, elevation: 55.0, distance: 1.0 };
        cam.frame(Vec3::ZERO, Vec3::new(cols as f32, rows as f32, 0.0), 1.1);
        cam
    }

    pub fn eye(&self) -> Vec3 {
        match self.projection {
            Projection::TopDown => self.target + Vec3::Z * (self.distance * 4.0 + 200.0),
            Projection::Perspective => {
                let (az, el) = (self.azimuth.to_radians(), self.elevation.to_radians());
                self.target + Vec3::new(el.cos() * az.cos(), el.cos() * az.sin(), el.sin()) * self.distance
            }
        }
    }

    pub fn view(&self) -> Mat4 {
        match self.projection {
            Projection::TopDown => Mat4::look_at_rh(self.eye(), self.target, Vec3::Y),
            Projection::Perspective => Mat4::look_at_rh(self.eye(), self.target, Vec3::Z),
        }
    }

    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        let aspect = aspect.max(1e-3);
        match self.projection {
            Projection::TopDown => {
                let h = self.distance;
                let depth = self.distance * 8.0 + 1000.0;
                Mat4::orthographic_rh(-h * aspect, h * aspect, -h, h, 0.1, depth)
            }
            Projection::Perspective => {
                // Near and far follow the distance, so big grids aren't
                // clipped (the Python far plane was fixed at 500 cells)
                let near = (self.distance * 0.01).max(0.02);
                let far = self.distance * 6.0 + 2000.0;
                Mat4::perspective_rh(FOV_Y_DEGREES.to_radians(), aspect, near, far)
            }
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.projection_matrix(aspect) * self.view()
    }

    /// World ray through a viewport point, `uv` in 0..1 with y down.
    pub fn ray(&self, uv: Vec2, aspect: f32) -> (Vec3, Vec3) {
        let inv = self.view_proj(aspect).inverse();
        let ndc = Vec2::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
        let near = inv.project_point3(ndc.extend(0.0));
        let far = inv.project_point3(ndc.extend(1.0));
        (near, (far - near).normalize_or(Vec3::NEG_Z))
    }

    /// Where a viewport point meets the horizontal plane at `height`.
    pub fn ground_point(&self, uv: Vec2, aspect: f32, height: f32) -> Option<Vec2> {
        let (o, d) = self.ray(uv, aspect);
        if d.z.abs() < 1e-6 {
            return None;
        }
        let t = (height - o.z) / d.z;
        (t > 0.0).then(|| (o + d * t).truncate())
    }

    /// Drag by `delta` pixels: turns the camera (perspective only).
    pub fn orbit(&mut self, delta: Vec2) {
        if self.projection == Projection::Perspective {
            self.azimuth -= delta.x * 0.4;
            self.elevation = (self.elevation + delta.y * 0.4).clamp(5.0, 89.0);
        }
    }

    /// Drag by `delta` pixels in a viewport `height_px` tall, so the ground
    /// follows the pointer.
    pub fn pan(&mut self, delta: Vec2, height_px: f32) {
        let per_px = self.cells_per_pixel(height_px);
        match self.projection {
            Projection::TopDown => {
                self.target.x -= delta.x * per_px;
                self.target.y += delta.y * per_px;
            }
            Projection::Perspective => {
                let az = self.azimuth.to_radians();
                let right = Vec3::new(-az.sin(), az.cos(), 0.0);
                let back = Vec3::new(az.cos(), az.sin(), 0.0);
                self.target += (-right * delta.x - back * delta.y) * per_px;
            }
        }
    }

    /// Moves the view by whole cells (keyboard panning).
    pub fn nudge(&mut self, cells: Vec2) {
        match self.projection {
            Projection::TopDown => self.target += cells.extend(0.0),
            Projection::Perspective => {
                let az = self.azimuth.to_radians();
                let right = Vec3::new(-az.sin(), az.cos(), 0.0);
                let forward = Vec3::new(-az.cos(), -az.sin(), 0.0);
                self.target += right * cells.x + forward * cells.y;
            }
        }
    }

    /// Size of one pixel at the target, in cells.
    pub fn cells_per_pixel(&self, height_px: f32) -> f32 {
        let visible = match self.projection {
            Projection::TopDown => self.distance * 2.0,
            Projection::Perspective => 2.0 * self.distance * (FOV_Y_DEGREES.to_radians() / 2.0).tan(),
        };
        visible / height_px.max(1.0)
    }

    /// Zooms by wheel `steps` (positive is closer; a notch is 1, trackpads
    /// send fractions) toward the viewport point `uv`, which stays put.
    pub fn zoom_at(&mut self, steps: f32, uv: Vec2, aspect: f32) {
        if steps == 0.0 || !steps.is_finite() {
            return;
        }
        let before = self.ground_point(uv, aspect, 0.0);
        self.distance = (self.distance * 0.85f32.powf(steps)).clamp(MIN_DISTANCE, MAX_DISTANCE);
        if let (Some(before), Some(after)) = (before, self.ground_point(uv, aspect, 0.0)) {
            self.target += (before - after).extend(0.0);
        }
    }

    /// Fits a box into view, with some margin.
    pub fn frame(&mut self, min: Vec3, max: Vec3, margin: f32) {
        let centre = (min + max) / 2.0;
        let span = (max - min).truncate().max_element().max(2.0);
        self.target = Vec3::new(centre.x, centre.y, 0.0);
        self.distance = match self.projection {
            Projection::TopDown => span / 2.0 * margin,
            Projection::Perspective => (span / 2.0) / (FOV_Y_DEGREES.to_radians() / 2.0).tan() * margin,
        }
        .clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: Vec2, b: Vec2) {
        assert!((a - b).length() < 1e-2, "{a} != {b}");
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        for projection in [Projection::TopDown, Projection::Perspective] {
            let mut cam = Camera::for_grid(80, 60, projection);
            let uv = Vec2::new(0.8, 0.3);
            let before = cam.ground_point(uv, 1.5, 0.0).expect("hits the ground");
            cam.zoom_at(3.0, uv, 1.5);
            assert_close(cam.ground_point(uv, 1.5, 0.0).expect("hits the ground"), before);
        }
    }

    #[test]
    fn top_down_shows_north_up() {
        let cam = Camera::for_grid(10, 10, Projection::TopDown);
        let top = cam.ground_point(Vec2::new(0.5, 0.0), 1.0, 0.0).expect("hit");
        let bottom = cam.ground_point(Vec2::new(0.5, 1.0), 1.0, 0.0).expect("hit");
        assert!(top.y > bottom.y);
    }

    #[test]
    fn pan_follows_the_pointer_top_down() {
        let mut cam = Camera::for_grid(40, 40, Projection::TopDown);
        let before = cam.ground_point(Vec2::new(0.5, 0.5), 1.0, 0.0).expect("hit");
        cam.pan(Vec2::new(100.0, 0.0), 800.0);
        let moved = cam.ground_point(Vec2::new(0.5 + 100.0 / 800.0, 0.5), 1.0, 0.0).expect("hit");
        assert_close(moved, before);
    }

    #[test]
    fn far_plane_reaches_a_huge_grid() {
        let cam = Camera::for_grid(2000, 2000, Projection::Perspective);
        let far_corner = cam.view_proj(1.0).project_point3(Vec3::new(2000.0, 2000.0, 0.0));
        assert!(far_corner.z <= 1.0);
    }
}
