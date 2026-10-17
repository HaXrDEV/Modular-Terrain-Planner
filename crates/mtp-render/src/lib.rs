//! Draws a Modular Terrain Planner layout with wgpu: the ground, battle map
//! and grid, every placed tile instanced at a level of detail that suits
//! its size on screen, ghosts for placements, and an outline around the
//! selection. Frames go into offscreen textures, which the app shows in its
//! UI and tests read back.
//!
//! Only features every backend has are used (vertex-buffer instancing, no
//! storage buffers or compute), so the OpenGL fallback works on old drivers.

pub mod camera;
pub mod device;
pub mod picking;
pub mod renderer;
pub mod scene;
pub mod thumbnail;

pub use camera::{Camera, Projection};
pub use renderer::{Frame, FrameStats, Palette, Renderer, Target};
pub use scene::{DrawKind, DrawTile};
pub use wgpu;
