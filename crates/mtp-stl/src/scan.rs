//! Finding the STL files in a library folder.
//!
//! A plain directory walk rather than glob patterns: folder names such as
//! `OpenForge [Stone]` are glob syntax, which made the Python version miss
//! them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// STL files in `dir`, and in its subfolders when `recursive`, sorted by path
/// ignoring case. Hidden files and folders (leading `.`) are skipped, and so
/// are subfolders that can't be read.
pub fn scan_folder(dir: &Path, recursive: bool) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    walk(dir, recursive, &mut found)?;
    let mut keyed: Vec<(String, PathBuf)> = found
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(dir).unwrap_or(&path);
            let key = relative.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect::<Vec<_>>();
            (key.join("/"), path)
        })
        .collect();
    keyed.sort();
    Ok(keyed.into_iter().map(|(_, path)| path).collect())
}

fn walk(dir: &Path, recursive: bool, found: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if recursive {
                // One unreadable subfolder shouldn't hide the rest of the library
                let _ = walk(&path, recursive, found);
            }
        } else if (kind.is_file() || path.is_file()) && is_stl(&path) {
            found.push(path);
        }
    }
    Ok(())
}

fn is_stl(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("stl"))
}
