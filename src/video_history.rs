//! Session undo/redo of complete video edits, grouped by input gesture.
// Ported from omnishot/video_history.py
//!
//! The Qt editor filters input events to decide when a gesture ends. The part
//! worth keeping is the bounded stack: a snapshot is committable only when it
//! differs from the current one, the past is capped at 100 entries, and a new
//! commit clears the redo stack. Reusing the same arbitrary `T` keeps the stack
//! independent of the widget tree.

/// The maximum number of past snapshots retained.
pub const LIMIT: usize = 100;

/// A bounded undo/redo stack of snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct EditHistory<T> {
    past: Vec<T>,
    current: T,
    future: Vec<T>,
}

impl<T: PartialEq> EditHistory<T> {
    /// Start a history with `current` as the initial snapshot.
    #[must_use]
    pub fn new(current: T) -> Self {
        Self {
            past: Vec::new(),
            current,
            future: Vec::new(),
        }
    }

    /// The live snapshot.
    #[must_use]
    pub fn current(&self) -> &T {
        &self.current
    }

    /// Whether an undo is available.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    /// Whether a redo is available.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Commit a snapshot, returning `true` when it changed anything.
    ///
    /// A snapshot equal to the current one is ignored; committing a new one
    /// clears the redo stack and caps the past at [`LIMIT`].
    // Ported from omnishot/video_history.py (EditHistory.flush)
    pub fn commit(&mut self, snapshot: T) -> bool {
        if snapshot == self.current {
            return false;
        }
        self.past
            .push(std::mem::replace(&mut self.current, snapshot));
        if self.past.len() > LIMIT {
            let excess = self.past.len() - LIMIT;
            self.past.drain(0..excess);
        }
        self.future.clear();
        true
    }

    /// Undo, returning the restored snapshot when one was available.
    // Ported from omnishot/video_history.py (EditHistory.undo)
    pub fn undo(&mut self) -> Option<T>
    where
        T: Clone,
    {
        let previous = self.past.pop()?;
        self.future
            .push(std::mem::replace(&mut self.current, previous.clone()));
        Some(previous)
    }

    /// Redo, returning the restored snapshot when one was available.
    // Ported from omnishot/video_history.py (EditHistory.redo)
    pub fn redo(&mut self) -> Option<T>
    where
        T: Clone,
    {
        let next = self.future.pop()?;
        self.past
            .push(std::mem::replace(&mut self.current, next.clone()));
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committing_an_unchanged_snapshot_is_a_no_op() {
        let mut history = EditHistory::new(1);
        assert!(!history.commit(1));
        assert!(!history.can_undo());
        assert!(history.commit(2));
        assert!(history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn undo_and_redo_walk_the_stack() {
        let mut history = EditHistory::new("a");
        assert!(history.commit("b"));
        assert!(history.commit("c"));
        assert_eq!(history.undo(), Some("b"));
        assert_eq!(history.current(), &"b");
        assert!(history.can_redo());
        assert_eq!(history.redo(), Some("c"));
        assert_eq!(history.current(), &"c");
        assert!(!history.can_redo());
        assert_eq!(history.undo(), Some("b"));
        // A new commit clears the redo stack.
        assert!(history.commit("d"));
        assert!(!history.can_redo());
    }

    #[test]
    fn the_past_is_capped_at_the_limit() {
        let mut history = EditHistory::new(0);
        for step in 1..=LIMIT + 20 {
            assert!(history.commit(step));
        }
        let mut undone = 0;
        while history.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, LIMIT);
        // The oldest retained snapshot is reached.
        assert_eq!(history.current(), &(LIMIT + 20 - LIMIT));
    }
}
