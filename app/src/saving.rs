//! Saving on a worker thread, so writing a big project doesn't hold up the
//! window. The UI thread plans what to write ([`files::plan_save`]), the
//! thread writes it and counts the STL files done.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_channel::{Receiver, TryRecvError};
use mtp_core::history::SaveToken;

use crate::files::{self, SavePlan, Saved};

/// What a job writes.
#[derive(Clone, Debug)]
pub enum Kind {
    /// The project, to `path`. The saved mark goes on the state `token`
    /// names, so edits made meanwhile stay unsaved.
    Save { path: PathBuf, token: SaveToken },
    /// An autosave of the project at `original` (or an untitled one).
    Autosave { file: PathBuf, original: Option<PathBuf> },
}

pub struct Job {
    pub kind: Kind,
    /// The document it was started for, to tell if it was replaced since.
    pub generation: u64,
    /// STL files to write.
    pub total: usize,
    done: Arc<AtomicUsize>,
    rx: Receiver<Result<Saved, String>>,
}

impl Job {
    pub fn start(kind: Kind, plan: SavePlan, generation: u64, repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let total = plan.len();
        let done = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (counter, work) = (done.clone(), kind.clone());
        let spawned = std::thread::Builder::new().name("save".into()).spawn(move || {
            let progress = |n: usize| {
                counter.store(n, Ordering::Relaxed);
                repaint();
            };
            let result = match &work {
                Kind::Save { path, .. } => files::write_save(plan, path, true, true, &progress),
                Kind::Autosave { file, original } => files::write_autosave(plan, file, original.as_deref(), &progress),
            };
            let _ = tx.send(result);
            repaint();
        });
        // A thread that didn't start dropped the sender, so finished() reports it
        if let Err(e) = spawned {
            tracing::error!("couldn't start saving: {e}");
        }
        Self { kind, generation, total, done, rx }
    }

    /// STL files written so far.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// The result, once the thread is done.
    pub fn finished(&self) -> Option<Result<Saved, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("The save stopped unexpectedly.".into())),
        }
    }
}
