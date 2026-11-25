//! Overlays: char ranges of a buffer that move with its edits, with an optional face (a
//! highlight group), end-of-line virtual text and a priority - how a script colours or
//! annotates text. A marker is an overlay with nothing inside (`start == end`).
//!
//! Like Emacs's default markers, an end doesn't move for text inserted right at it: text
//! typed at an overlay's start joins it, text typed at its end stays outside.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::history::BufferEdit;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    /// Unique across buffers, for the script to name it by.
    pub id: u64,
    pub start: usize,
    pub end: usize,
    pub face: Option<String>,
    pub virtual_text: Option<String>,
    /// Faces of higher priority paint over lower ones.
    pub priority: i32,
}

impl Overlay {
    pub fn new(start: usize, end: usize) -> Self {
        let (start, end) = (start.min(end), start.max(end));
        Overlay {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            start,
            end,
            face: None,
            virtual_text: None,
            priority: 0,
        }
    }

    /// Follows `edit`, applied to the text just now.
    pub fn follow(&mut self, edit: &BufferEdit) {
        let (at, removed, inserted) = (
            edit.char_range.start,
            edit.old_text.chars().count(),
            edit.text.chars().count(),
        );
        let map = |p: usize| {
            if p <= at {
                p
            } else if p >= at + removed {
                p - removed + inserted
            } else {
                at
            }
        };
        self.start = map(self.start);
        self.end = map(self.end).max(self.start);
    }

    /// `pos` is inside it (or, for a marker, at it).
    pub fn covers(&self, pos: usize) -> bool {
        if self.start == self.end {
            pos == self.start
        } else {
            (self.start..self.end).contains(&pos)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::buffer::Buffer;
    use crate::selection::Selection;

    use super::*;

    #[test]
    fn overlays_follow_edits_around_inside_and_at_their_ends() {
        let mut b = Buffer::from_str("one two three");
        let two = Overlay::new(4, 7);
        let mark = Overlay::new(8, 8);
        b.overlays = vec![two.clone(), mark.clone()];
        let at = |b: &Buffer, id| {
            b.overlays
                .iter()
                .find(|o| o.id == id)
                .map(|o| (o.start, o.end))
                .unwrap()
        };

        b.replace_range(0..3, "1"); // before both: they shift left
        assert_eq!(at(&b, two.id), (2, 5));
        assert_eq!(at(&b, mark.id), (6, 6));
        b.replace_range(2..2, ">"); // at the start: joins it
        assert_eq!(at(&b, two.id), (2, 6));
        b.replace_range(6..6, "<"); // at the end: stays outside
        assert_eq!(at(&b, two.id), (2, 6));
        assert_eq!(b.rope.to_string(), "1 >two< three");
        b.replace_range(3..5, ""); // inside: it shrinks
        assert_eq!(at(&b, two.id), (2, 4));
        assert!(b.undo() && b.undo());
        assert_eq!(b.rope.to_string(), "1 >two three");
        assert_eq!(at(&b, two.id), (2, 6));

        // Typing through the selections moves them too.
        b.selections = vec![Selection {
            anchor: 0,
            head: 0,
            pref_col: 0,
        }];
        b.enter_insert_before();
        b.insert_char('x');
        assert_eq!(at(&b, two.id), (3, 7));
        assert!(b.overlays[0].covers(3) && !b.overlays[0].covers(7));
        assert!(b.overlays[1].covers(b.overlays[1].start));
    }
}
