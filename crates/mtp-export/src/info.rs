//! What exports need to know about an STL file beyond the document: its name,
//! pack, colour and printed volume. The app knows more (it has read the
//! files); tests and headless use fall back to what the path tells.

use std::collections::HashMap;

use mtp_core::color::tile_color;
use mtp_core::tile::TileKey;

/// How one STL file is described in exports.
#[derive(Clone, Debug, PartialEq)]
pub struct TileMeta {
    /// Shown as the tile's name: the file name without its extension.
    pub name: String,
    /// The pack or folder it came from.
    pub folder: String,
    pub color: [u8; 3],
    /// Solid volume in cubic millimetres, when the mesh has been measured.
    pub volume_mm3: Option<f64>,
}

impl TileMeta {
    /// Everything the path tells: name, folder and the colour the app uses.
    pub fn from_key(key: &TileKey) -> Self {
        Self {
            name: key.stem().to_string(),
            folder: key.folder_name().to_string(),
            color: tile_color(key.folder_name(), key.file_name()),
            volume_mm3: None,
        }
    }
}

/// Looks up how to describe an STL file.
pub trait TileInfo {
    fn meta(&self, key: &TileKey) -> TileMeta;
}

/// Describes every file by its path alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultInfo;

impl TileInfo for DefaultInfo {
    fn meta(&self, key: &TileKey) -> TileMeta {
        TileMeta::from_key(key)
    }
}

/// Known files from the map, anything else from its path.
impl<S: std::hash::BuildHasher> TileInfo for HashMap<TileKey, TileMeta, S> {
    fn meta(&self, key: &TileKey) -> TileMeta {
        self.get(key).cloned().unwrap_or_else(|| TileMeta::from_key(key))
    }
}
