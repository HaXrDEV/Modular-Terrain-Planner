//! Writing files so that a crash, a full disk or a power cut never leaves a
//! half-written project or settings file. The Python version truncated the
//! old file first.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The backup kept next to a project: `castle.mtp` -> `castle.mtp.bak`.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// Writes `contents` to a temporary file next to `path`, flushes it to disk,
/// then renames it over `path`. With `keep_backup`, the previous file is
/// copied to [`backup_path`] first.
pub fn write_atomic(path: &Path, contents: &[u8], keep_backup: bool) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);

    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        drop(f);
        if keep_backup && path.exists() {
            fs::copy(path, backup_path(path))?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_and_keeps_a_backup() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("castle.mtp");
        write_atomic(&path, b"one", true).expect("first write");
        assert!(!backup_path(&path).exists());
        write_atomic(&path, b"two", true).expect("second write");
        assert_eq!(fs::read(&path).expect("read"), b"two");
        assert_eq!(fs::read(backup_path(&path)).expect("backup"), b"one");
        assert_eq!(fs::read_dir(dir.path()).expect("list").count(), 2, "no temporary file left");
    }

    #[test]
    fn a_failed_write_keeps_the_original() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("castle.mtp");
        write_atomic(&path, b"original", false).expect("write");
        // A directory can't be renamed over a file
        let blocked = dir.path().join("blocked");
        fs::create_dir(&blocked).expect("dir");
        assert!(write_atomic(&blocked, b"x", false).is_err());
        assert_eq!(fs::read(&path).expect("read"), b"original");
    }
}
