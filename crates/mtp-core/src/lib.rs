//! The part of Modular Terrain Planner that knows nothing about the GPU or
//! the UI: grid units, tile footprints, the placement rules, undo history and
//! the project file format.

pub mod color;
pub mod document;
pub mod footprint;
pub mod grid;
pub mod history;
pub mod outline;
pub mod persist;
pub mod project;
pub mod rules;
pub mod tile;
pub mod units;
