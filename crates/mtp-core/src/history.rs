//! Undo history of whole document states, with a mark for the saved one.
//!
//! States are shared (`Arc`), so a step costs one copy of the tile list. With
//! the Python version undoing back to the saved state still showed `*`, and a
//! refused placement still added a step; here only real changes are pushed
//! and the saved mark makes "unsaved" exact.

use std::sync::Arc;

/// How many steps can be undone.
pub const UNDO_LIMIT: usize = 200;

#[derive(Debug)]
struct Entry<T> {
    /// What the step did, e.g. "Place tile", for Undo/Redo menu items.
    label: String,
    state: Arc<T>,
    /// Tells this state apart from every other, for [`SaveToken`].
    serial: u64,
}

/// Names the state a save started from, so the saved mark goes on that state
/// even when edits were made while it was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveToken(u64);

#[derive(Debug)]
pub struct History<T> {
    entries: Vec<Entry<T>>,
    cursor: usize,
    saved: Option<usize>,
    limit: usize,
    next_serial: u64,
}

impl<T> History<T> {
    /// A history whose only state is `initial`, marked as saved.
    pub fn new(initial: Arc<T>) -> Self {
        Self {
            entries: vec![Entry { label: String::new(), state: initial, serial: 0 }],
            cursor: 0,
            saved: Some(0),
            limit: UNDO_LIMIT,
            next_serial: 1,
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.max(1);
        self
    }

    pub fn current(&self) -> &Arc<T> {
        &self.entries[self.cursor].state
    }

    /// Adds a step after the current one, dropping anything that was undone.
    pub fn push(&mut self, label: impl Into<String>, state: Arc<T>) {
        self.entries.truncate(self.cursor + 1);
        if self.saved.is_some_and(|s| s > self.cursor) {
            self.saved = None;
        }
        let serial = self.serial();
        self.entries.push(Entry { label: label.into(), state, serial });
        self.cursor += 1;
        while self.entries.len() > self.limit + 1 {
            self.entries.remove(0);
            self.cursor -= 1;
            self.saved = self.saved.and_then(|s| s.checked_sub(1));
        }
    }

    /// Like [`push`](Self::push), but a repeat of the same unsaved step (a
    /// slider being dragged) replaces it instead of adding another.
    pub fn push_or_merge(&mut self, label: &str, state: Arc<T>) {
        let top = self.cursor == self.entries.len() - 1;
        if top && self.cursor > 0 && self.entries[self.cursor].label == label && self.saved != Some(self.cursor) {
            // A different state, so a save started from the one replaced doesn't mark it
            let serial = self.serial();
            self.entries[self.cursor] = Entry { label: label.to_owned(), state, serial };
        } else {
            self.push(label, state);
        }
    }

    pub fn undo(&mut self) -> Option<&Arc<T>> {
        (self.cursor > 0).then(|| {
            self.cursor -= 1;
            self.current()
        })
    }

    pub fn redo(&mut self) -> Option<&Arc<T>> {
        (self.cursor + 1 < self.entries.len()).then(|| {
            self.cursor += 1;
            self.current()
        })
    }

    pub fn undo_label(&self) -> Option<&str> {
        (self.cursor > 0).then(|| self.entries[self.cursor].label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.entries.get(self.cursor + 1).map(|e| e.label.as_str())
    }

    pub fn is_dirty(&self) -> bool {
        self.saved != Some(self.cursor)
    }

    pub fn mark_saved(&mut self) {
        self.saved = Some(self.cursor);
    }

    /// The current state, for a save about to start.
    pub fn token(&self) -> SaveToken {
        SaveToken(self.entries[self.cursor].serial)
    }

    /// Marks the state `token` names as saved, wherever it is now. If it
    /// has left the history, no state is the saved one.
    pub fn mark_saved_at(&mut self, token: SaveToken) {
        self.saved = self.entries.iter().position(|e| e.serial == token.0);
    }

    fn serial(&mut self) -> u64 {
        self.next_serial += 1;
        self.next_serial - 1
    }

    /// Marks the current state as not saved anywhere (recovered work).
    pub fn mark_unsaved(&mut self) {
        self.saved = None;
    }

    /// Changes every state in the history the same way, for facts learnt
    /// later (a missing tile's real size) that undo must not take back.
    pub fn map_states(&mut self, mut f: impl FnMut(&T) -> Option<T>) {
        for e in &mut self.entries {
            if let Some(new) = f(&e.state) {
                e.state = Arc::new(new);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> History<i32> {
        History::new(Arc::new(0))
    }

    #[test]
    fn undoing_to_the_saved_state_is_clean() {
        let mut h = history();
        h.push("a", Arc::new(1));
        assert!(h.is_dirty());
        h.mark_saved();
        h.push("b", Arc::new(2));
        assert!(h.is_dirty());
        assert_eq!(**h.undo().expect("undo"), 1);
        assert!(!h.is_dirty());
        assert_eq!(h.redo_label(), Some("b"));
    }

    #[test]
    fn a_new_step_after_undo_drops_the_redo_and_the_saved_mark_past_it() {
        let mut h = history();
        h.push("a", Arc::new(1));
        h.mark_saved();
        h.undo();
        h.push("c", Arc::new(3));
        assert!(h.redo().is_none());
        assert!(h.is_dirty(), "the saved state is gone");
    }

    #[test]
    fn the_limit_drops_the_oldest_steps() {
        let mut h = history().with_limit(3);
        for i in 1..=10 {
            h.push("x", Arc::new(i));
        }
        let mut undone = 0;
        while h.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, 3);
        assert_eq!(**h.current(), 7);
    }

    #[test]
    fn a_save_marks_the_state_it_started_from() {
        let mut h = history();
        h.push("a", Arc::new(1));
        let token = h.token();
        // Edits made while the save is written stay unsaved
        h.push("b", Arc::new(2));
        h.mark_saved_at(token);
        assert!(h.is_dirty());
        h.undo();
        assert!(!h.is_dirty(), "the saved state is the one the save started from");

        // A slider dragged on during the save changes the state it started from
        h.push_or_merge("Scale map", Arc::new(3));
        let token = h.token();
        h.push_or_merge("Scale map", Arc::new(4));
        h.mark_saved_at(token);
        assert!(h.is_dirty());

        // A state that left the history (undone and replaced) can't be marked
        h.undo();
        let token = h.token();
        h.undo();
        h.push("c", Arc::new(5));
        h.mark_saved_at(token);
        assert!(h.is_dirty());
        assert!(h.undo().is_some() && h.is_dirty());
    }

    #[test]
    fn repeated_steps_merge() {
        let mut h = history();
        h.push_or_merge("Scale map", Arc::new(1));
        h.push_or_merge("Scale map", Arc::new(2));
        h.push_or_merge("Scale map", Arc::new(3));
        assert_eq!(**h.undo().expect("undo"), 0);
    }
}
