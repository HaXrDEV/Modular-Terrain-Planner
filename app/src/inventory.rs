//! How many prints of each STL file the user owns. It belongs to the user,
//! not a project, so every layout knows what's already on the shelf.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use mtp_core::persist::write_atomic;
use mtp_core::tile::TileKey;

#[derive(Debug, Default)]
pub struct Inventory {
    owned: HashMap<TileKey, u32>,
    path: Option<PathBuf>,
}

impl Inventory {
    /// Reads `inventory.json` from `dir`; a missing or damaged file is empty.
    pub fn load(dir: &Path) -> (Self, Option<String>) {
        let path = dir.join("inventory.json");
        let mut inv = Self { owned: HashMap::new(), path: Some(path.clone()) };
        let Ok(bytes) = std::fs::read(&path) else { return (inv, None) };
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(v) => {
                if let Some(map) = v.get("owned").and_then(|o| o.as_object()) {
                    for (k, n) in map {
                        if let Some(n) = n.as_u64() {
                            inv.owned.insert(TileKey::new(k), n.min(u64::from(u32::MAX)) as u32);
                        }
                    }
                }
                (inv, None)
            }
            Err(e) => {
                let _ = std::fs::rename(&path, path.with_extension("json.broken"));
                (inv, Some(format!("The inventory couldn't be read ({e}) and was moved aside.")))
            }
        }
    }

    /// Whether the user has recorded anything yet (exports only show
    /// owned counts then).
    pub fn is_empty(&self) -> bool {
        self.owned.is_empty()
    }

    pub fn owned(&self, key: &TileKey) -> u32 {
        self.owned.get(key).copied().unwrap_or(0)
    }

    /// Sets the count and saves.
    pub fn set(&mut self, key: &TileKey, count: u32) -> Result<(), String> {
        if count == 0 {
            self.owned.remove(key);
        } else {
            self.owned.insert(key.clone(), count);
        }
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let sorted: BTreeMap<&str, u32> = self.owned.iter().map(|(k, n)| (k.as_str(), *n)).collect();
        let text = serde_json::to_vec_pretty(&serde_json::json!({ "version": 1, "owned": sorted }))
            .map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        write_atomic(path, &text, false).map_err(|e| format!("The inventory couldn't be saved: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_survive_a_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let key = TileKey::new("C:/Tiles/floor.stl");
        let (mut inv, problem) = Inventory::load(dir.path());
        assert!(problem.is_none());
        inv.set(&key, 7).expect("save");
        let (again, _) = Inventory::load(dir.path());
        assert_eq!(again.owned(&TileKey::new("c:\\tiles\\FLOOR.stl")), 7);
        let mut again = again;
        again.set(&key, 0).expect("save");
        assert_eq!(Inventory::load(dir.path()).0.owned(&key), 0);
    }
}
