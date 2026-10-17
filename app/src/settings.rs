//! Where the app keeps its files, and the settings that survive a restart.

use std::path::{Path, PathBuf};

use mtp_core::persist::write_atomic;
use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "Modular Terrain Planner";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The app's folders, from the OS conventions (`%APPDATA%`, `~/Library`,
/// XDG). Everything lives under them; nothing goes in tile or project folders.
#[derive(Clone, Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
}

impl Paths {
    pub fn from_os() -> Self {
        match directories::ProjectDirs::from("", "", APP_NAME) {
            Some(d) => Self {
                config: d.config_dir().to_path_buf(),
                data: d.data_local_dir().to_path_buf(),
                cache: d.cache_dir().to_path_buf(),
            },
            None => {
                let base = std::env::temp_dir().join("modular-terrain-planner");
                Self { config: base.join("config"), data: base.join("data"), cache: base.join("cache") }
            }
        }
    }

    /// Everything inside one folder, for tests and `--portable`.
    pub fn under(root: &Path) -> Self {
        Self { config: root.join("config"), data: root.join("data"), cache: root.join("cache") }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    pub fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }

    pub fn autosave(&self) -> PathBuf {
        self.data.join("autosave")
    }

    pub fn mesh_cache(&self) -> PathBuf {
        self.cache.join("meshes")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Auto,
    Vulkan,
    Dx12,
    Metal,
    OpenGl,
}

impl Backend {
    /// The `WGPU_BACKEND` value for this choice.
    pub fn env_value(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Vulkan => Some("vulkan"),
            Self::Dx12 => Some("dx12"),
            Self::Metal => Some("metal"),
            Self::OpenGl => Some("gl"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Vulkan => "Vulkan",
            Self::Dx12 => "DirectX 12",
            Self::Metal => "Metal",
            Self::OpenGl => "OpenGL (for old drivers)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionFolder {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// UI zoom on top of the monitor's scaling.
    pub ui_scale: f32,
    pub backend: Backend,
    /// Most recent first.
    pub recent_projects: Vec<String>,
    /// Library folders open when the app last closed, in order.
    pub session_folders: Vec<SessionFolder>,
    /// Minutes between autosaves of unsaved changes (0 turns it off).
    pub autosave_minutes: u32,
    pub mesh_cache_mb: u32,
    /// Detail multiplier for the 3D view (lower is faster).
    pub detail: f32,
    pub start_in_3d: bool,
    pub show_stats: bool,
    /// Library thumbnail size in points.
    pub thumbnail_size: f32,
    /// Paper for PDF build plans.
    pub paper: String,
    /// Write absolute paths in CSV print lists (they show usernames).
    pub csv_full_paths: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            ui_scale: 1.0,
            backend: Backend::Auto,
            recent_projects: Vec::new(),
            session_folders: Vec::new(),
            autosave_minutes: 3,
            mesh_cache_mb: 2048,
            detail: 1.0,
            start_in_3d: false,
            show_stats: false,
            thumbnail_size: 88.0,
            paper: "A4".into(),
            csv_full_paths: false,
        }
    }
}

pub const MAX_RECENT: usize = 10;

impl Settings {
    /// Reads the settings. A damaged file is renamed aside (so it can be
    /// looked at) and defaults are used; a missing one tries the Python
    /// version's file for recent projects and folders.
    pub fn load(paths: &Paths) -> (Self, Option<String>) {
        let path = paths.settings_file();
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(s) => (s.sanitised(), None),
                Err(e) => {
                    let aside = path.with_extension("json.broken");
                    let _ = std::fs::rename(&path, &aside);
                    (
                        Self::default(),
                        Some(format!("The settings file couldn't be read ({e}) and was moved to {}.", aside.display())),
                    )
                }
            },
            Err(_) => (Self::from_python_version().unwrap_or_default(), None),
        }
    }

    fn sanitised(mut self) -> Self {
        if !self.ui_scale.is_finite() || !(0.5..=3.0).contains(&self.ui_scale) {
            self.ui_scale = 1.0;
        }
        if !self.detail.is_finite() || !(0.05..=4.0).contains(&self.detail) {
            self.detail = 1.0;
        }
        self.thumbnail_size =
            if self.thumbnail_size.is_finite() { self.thumbnail_size.clamp(48.0, 192.0) } else { 88.0 };
        self.recent_projects.truncate(MAX_RECENT);
        self
    }

    /// Recent projects and folders from `~/.modular-terrain-planner`.
    fn from_python_version() -> Option<Self> {
        let home = directories::BaseDirs::new()?.home_dir().join(".modular-terrain-planner").join("settings.json");
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(home).ok()?).ok()?;
        let strings = |key: &str| -> Vec<String> {
            value
                .get(key)
                .and_then(|v| v.as_array())
                .map_or_else(Vec::new, |a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
        };
        Some(Self {
            recent_projects: strings("recent_projects"),
            session_folders: strings("recent_folders")
                .into_iter()
                .map(|path| SessionFolder { path, recursive: false })
                .collect(),
            ..Self::default()
        })
    }

    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        std::fs::create_dir_all(&paths.config)?;
        let text = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(&paths.settings_file(), &text, false)
    }

    pub fn add_recent_project(&mut self, path: &str) {
        self.recent_projects.retain(|p| p != path);
        self.recent_projects.insert(0, path.to_owned());
        self.recent_projects.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_repairs_bad_values() {
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = Paths::under(dir.path());
        let mut s = Settings { ui_scale: 99.0, ..Settings::default() };
        s.add_recent_project("a.mtp");
        s.add_recent_project("b.mtp");
        s.add_recent_project("a.mtp");
        s.save(&paths).expect("save");
        let (loaded, problem) = Settings::load(&paths);
        assert!(problem.is_none());
        assert_eq!(loaded.recent_projects, ["a.mtp", "b.mtp"]);
        assert_eq!(loaded.ui_scale, 1.0);
    }

    #[test]
    fn a_damaged_file_is_moved_aside() {
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(&paths.config).expect("dir");
        std::fs::write(paths.settings_file(), b"[1, 2").expect("write");
        let (s, problem) = Settings::load(&paths);
        assert_eq!(s, Settings::default());
        assert!(problem.is_some());
        assert!(paths.config.join("settings.json.broken").exists());
    }

    #[test]
    fn wrong_types_fall_back_per_field_or_whole() {
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(&paths.config).expect("dir");
        std::fs::write(paths.settings_file(), br#"{"theme": "dark", "unknown": 1}"#).expect("write");
        let (s, problem) = Settings::load(&paths);
        assert!(problem.is_none());
        assert_eq!(s.theme, ThemeChoice::Dark);
    }
}
