//! One window per user: a second launch (double-clicking a project, for
//! instance) hands its files to the window already open and exits.
//!
//! The first instance listens on a local socket (a named pipe on Windows,
//! an abstract socket on Linux, a socket file on macOS). Later ones connect,
//! send their paths one per line, end with an empty line, and wait for
//! "ok".

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream};

/// Paths sent by later launches, and a way to wake the window when they come.
pub struct Incoming {
    pub rx: Receiver<Vec<PathBuf>>,
    pub ctx: Arc<OnceLock<egui::Context>>,
}

pub enum Claim {
    /// This is the first instance.
    First(Incoming),
    /// Another instance took the files.
    Forwarded,
    /// Couldn't tell (no socket support): run on our own.
    Alone,
}

fn socket_name(scope: &str) -> std::io::Result<Name<'static>> {
    // One per user, and per --portable folder
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
    let key = format!("{user}|{scope}");
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let base = format!("modular-terrain-planner-{hash:016x}.sock");
    if GenericNamespaced::is_supported() {
        base.to_ns_name::<GenericNamespaced>().map(|n| n.into_owned())
    } else {
        std::env::temp_dir().join(base).to_fs_name::<GenericFilePath>().map(|n| n.into_owned())
    }
}

fn forward(name: Name<'_>, paths: &[PathBuf]) -> std::io::Result<()> {
    let stream = Stream::connect(name)?;
    let mut conn = BufReader::new(stream);
    let mut message = String::new();
    for p in paths {
        let absolute = std::path::absolute(p).unwrap_or_else(|_| p.clone());
        message.push_str(&absolute.to_string_lossy());
        message.push('\n');
    }
    message.push('\n');
    conn.get_mut().write_all(message.as_bytes())?;
    let mut answer = String::new();
    conn.read_line(&mut answer)?;
    if answer.trim() == "ok" { Ok(()) } else { Err(std::io::Error::other("no answer from the open window")) }
}

fn serve(listener: interprocess::local_socket::Listener, tx: Sender<Vec<PathBuf>>, ctx: Arc<OnceLock<egui::Context>>) {
    for conn in listener.incoming().filter_map(Result::ok) {
        let mut conn = BufReader::new(conn);
        let mut paths = Vec::new();
        let mut line = String::new();
        while conn.read_line(&mut line).is_ok_and(|n| n > 0) {
            let path = line.trim_end_matches(['\r', '\n']);
            if path.is_empty() {
                break;
            }
            paths.push(PathBuf::from(path));
            line.clear();
        }
        let _ = conn.get_mut().write_all(b"ok\n");
        if tx.send(paths).is_err() {
            return;
        }
        if let Some(ctx) = ctx.get() {
            ctx.request_repaint();
        }
    }
}

/// Becomes the first instance, or hands `paths` to it.
pub fn claim(scope: &Path, paths: &[PathBuf]) -> Claim {
    let Ok(name) = socket_name(&scope.to_string_lossy()) else { return Claim::Alone };
    if forward(name.clone(), paths).is_ok() {
        return Claim::Forwarded;
    }
    let listener = match ListenerOptions::new().name(name.clone()).try_overwrite(true).create_sync() {
        Ok(l) => l,
        Err(_) => {
            // Two launches at once: the other one won, so hand it our files
            return if forward(name, paths).is_ok() { Claim::Forwarded } else { Claim::Alone };
        }
    };
    let (tx, rx) = crossbeam_channel::unbounded();
    let ctx = Arc::new(OnceLock::new());
    let shared = ctx.clone();
    let spawned = std::thread::Builder::new().name("single instance".into()).spawn(move || serve(listener, tx, shared));
    match spawned {
        Ok(_) => Claim::First(Incoming { rx, ctx }),
        Err(_) => Claim::Alone,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_launch_hands_over_its_files() {
        let scope = Path::new("test-scope-single-instance");
        let Claim::First(incoming) = claim(scope, &[]) else { panic!("the first claim should win") };
        let files = [PathBuf::from("castle.mtp"), PathBuf::from("Tiles")];
        assert!(matches!(claim(scope, &files), Claim::Forwarded));
        let got = incoming.rx.recv_timeout(std::time::Duration::from_secs(5)).expect("files arrive");
        assert_eq!(got.len(), 2);
        assert!(got[0].ends_with("castle.mtp") && got[0].is_absolute());
    }
}
