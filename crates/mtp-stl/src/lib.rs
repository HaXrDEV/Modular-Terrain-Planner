//! Turns STL files into tiles the planner can place and draw.
//!
//! The pipeline for one file:
//!
//! 1. [`parse`] reads binary or ASCII STL into a triangle soup. Binary files
//!    are recognised by their size, since many start with `solid` too.
//! 2. [`mesh`] measures the bounding box, guesses the unit, sizes the tile in
//!    grid cells, normalises every axis to `[0, 1]`, welds shared corners into
//!    an indexed mesh and estimates the printed volume.
//! 3. [`lod`] simplifies the welded mesh into a short chain of coarser tiers
//!    so far-away tiles cost little to draw.
//! 4. [`cache`] stores the result on disk, so the next launch skips 1-3.
//!
//! [`scan`] finds the STL files in a library folder and [`load_many`] runs
//! the pipeline over them in parallel, one result per file.

pub mod cache;
mod hash;
pub mod load;
pub mod lod;
pub mod mesh;
pub mod parse;
pub mod scan;

pub use cache::MeshCache;
pub use load::{LOADER_VERSION, LoadError, LoadErrorKind, LoadedTile, load_many, load_tile};
pub use mesh::IndexedMesh;
pub use parse::{StlError, Triangle, parse_stl};
pub use scan::scan_folder;
