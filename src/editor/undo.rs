//! Undo/redo with typing coalescing.
//!
//! Snapshots are whole-document text plus cursor state: documents are small
//! (a few MB at most) and a full snapshot is impossible to get wrong, which is
//! exactly what an editor's data path needs.

use super::doc::DocPos;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub text: String,
    pub cursor: DocPos,
    pub anchor: DocPos,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    /// Character-by-character typing: coalesced into one undo step.
    Typing,
    /// Structural change (Enter, delete, paste, formatting): own undo step.
    Structural,
}

const COALESCE_WINDOW: Duration = Duration::from_millis(900);
const MAX_DEPTH: usize = 300;

#[derive(Default)]
pub struct Undo {
    past: Vec<Snapshot>,
    future: Vec<Snapshot>,
    last: Option<(Instant, EditKind)>,
}

impl Undo {
    /// Caret movement ends the current typing burst.
    pub fn break_coalescing(&mut self) {
        self.last = None;
    }

    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
        self.last = None;
    }

    /// Record the state *before* a mutation. Consecutive typing within the
    /// coalesce window collapses into the last snapshot (one undo per word
    /// burst, like `TextEdit`).
    pub fn record(&mut self, before: Snapshot, kind: EditKind) {
        let coalesce = kind == EditKind::Typing
            && self
                .last
                .is_some_and(|(at, last_kind)| last_kind == EditKind::Typing && at.elapsed() < COALESCE_WINDOW);
        self.last = Some((Instant::now(), kind));
        self.future.clear();
        if coalesce {
            return;
        }
        self.past.push(before);
        if self.past.len() > MAX_DEPTH {
            self.past.remove(0);
        }
    }

    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let previous = self.past.pop()?;
        self.future.push(current);
        self.last = None;
        Some(previous)
    }

    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.future.pop()?;
        self.past.push(current);
        self.last = None;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(text: &str) -> Snapshot {
        Snapshot {
            text: text.into(),
            cursor: DocPos::default(),
            anchor: DocPos::default(),
        }
    }

    #[test]
    fn typing_coalesces_and_structure_does_not() {
        let mut undo = Undo::default();
        undo.record(snap("a"), EditKind::Typing);
        undo.record(snap("ab"), EditKind::Typing);
        undo.record(snap("abc"), EditKind::Typing);
        assert_eq!(undo.undo(snap("abcd")).unwrap().text, "a");

        let mut undo = Undo::default();
        undo.record(snap("a"), EditKind::Structural);
        undo.record(snap("ab"), EditKind::Structural);
        assert_eq!(undo.undo(snap("abc")).unwrap().text, "ab");
        assert_eq!(undo.undo(snap("ab")).unwrap().text, "a");
    }

    #[test]
    fn undo_redo_round_trip_and_branching() {
        let mut undo = Undo::default();
        undo.record(snap("1"), EditKind::Structural);
        let current = snap("2");
        let restored = undo.undo(current.clone()).unwrap();
        assert_eq!(restored.text, "1");
        let redone = undo.redo(restored).unwrap();
        assert_eq!(redone.text, "2");
        // A new edit after undo discards the redo branch.
        undo.record(snap("2"), EditKind::Structural);
        assert!(undo.redo(snap("x")).is_none());
    }
}
