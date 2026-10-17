//! Stamps: groups of tiles saved under a name (a corridor piece, a 4 x 4
//! room) to paste into any project. They live in the data folder.

use std::path::{Path, PathBuf};

use mtp_core::footprint::Rotation;
use mtp_core::persist::write_atomic;
use mtp_core::rules::Proposal;
use mtp_core::tile::TileKey;
use mtp_core::units::TileSize;
use serde_json::{Value, json};

use crate::editor::Clipboard;

#[derive(Clone, Debug)]
pub struct Stamp {
    pub name: String,
    pub tiles: Clipboard,
}

#[derive(Debug, Default)]
pub struct Stamps {
    pub list: Vec<Stamp>,
    path: Option<PathBuf>,
}

fn tile_json(p: &Proposal) -> Value {
    json!({
        "file": p.key.as_str(),
        "x": p.x,
        "y": p.y,
        "z": p.z,
        "rotation": p.rotation.degrees(),
        "size": { "w": p.size.w, "h": p.size.h, "gz": p.size.gz },
    })
}

fn tile_from(v: &Value) -> Option<Proposal> {
    let size = v.get("size")?;
    let num = |o: &Value, k: &str| o.get(k).and_then(Value::as_f64).filter(|n| n.is_finite());
    Some(Proposal {
        key: TileKey::new(v.get("file")?.as_str()?),
        size: TileSize { w: num(size, "w")? as u32, h: num(size, "h")? as u32, gz: num(size, "gz")? as f32 },
        size_known: true,
        x: num(v, "x")? as f32,
        y: num(v, "y")? as f32,
        z: num(v, "z").unwrap_or(0.0) as f32,
        rotation: Rotation::from_degrees(num(v, "rotation").unwrap_or(0.0))?,
        level: 0,
    })
}

impl Stamps {
    /// Reads `stamps.json` from `dir`; a damaged file is moved aside.
    pub fn load(dir: &Path) -> (Self, Option<String>) {
        let path = dir.join("stamps.json");
        let mut stamps = Self { list: Vec::new(), path: Some(path.clone()) };
        let Ok(bytes) = std::fs::read(&path) else { return (stamps, None) };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => {
                for s in v.get("stamps").and_then(Value::as_array).into_iter().flatten() {
                    let name = s.get("name").and_then(Value::as_str).unwrap_or("Stamp").to_owned();
                    let items: Vec<Proposal> =
                        s.get("tiles").and_then(Value::as_array).into_iter().flatten().filter_map(tile_from).collect();
                    let w = s.get("w").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                    let h = s.get("h").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                    if !items.is_empty() {
                        stamps.list.push(Stamp { name, tiles: Clipboard { items, w, h } });
                    }
                }
                (stamps, None)
            }
            Err(e) => {
                let _ = std::fs::rename(&path, path.with_extension("json.broken"));
                (stamps, Some(format!("The stamps couldn't be read ({e}) and were moved aside.")))
            }
        }
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let list: Vec<Value> = self
            .list
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "w": s.tiles.w,
                    "h": s.tiles.h,
                    "tiles": s.tiles.items.iter().map(tile_json).collect::<Vec<_>>(),
                })
            })
            .collect();
        let text = serde_json::to_vec_pretty(&json!({ "version": 1, "stamps": list })).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        write_atomic(path, &text, false).map_err(|e| format!("The stamps couldn't be saved: {e}"))
    }

    pub fn add(&mut self, name: String, tiles: Clipboard) -> Result<(), String> {
        self.list.retain(|s| s.name != name);
        self.list.push(Stamp { name, tiles });
        self.list.sort_by_key(|s| s.name.to_lowercase());
        self.save()
    }

    pub fn remove(&mut self, index: usize) -> Result<(), String> {
        if index < self.list.len() {
            self.list.remove(index);
        }
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip() -> Clipboard {
        let p = Proposal {
            key: TileKey::new("C:/Tiles/floor.stl"),
            size: TileSize { w: 2, h: 2, gz: 0.48 },
            size_known: true,
            x: 0.0,
            y: 2.0,
            z: 0.0,
            rotation: Rotation::from_quarters(1),
            level: 3,
        };
        Clipboard { items: vec![p], w: 2.0, h: 4.0 }
    }

    #[test]
    fn stamps_survive_a_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut s, _) = Stamps::load(dir.path());
        s.add("Corridor".into(), clip()).expect("save");
        s.add("Room".into(), clip()).expect("save");
        let (again, problem) = Stamps::load(dir.path());
        assert!(problem.is_none());
        let names: Vec<&str> = again.list.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Corridor", "Room"]);
        let t = &again.list[0].tiles.items[0];
        assert_eq!((t.y, t.rotation.degrees(), t.level), (2.0, 90, 0), "levels come from where it's pasted");
        let mut again = again;
        again.remove(0).expect("save");
        assert_eq!(Stamps::load(dir.path()).0.list.len(), 1);
    }
}
