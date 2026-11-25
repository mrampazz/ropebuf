use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::{buffer::Buffer, rope::Rope, utils::sel_mode};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum BufferMovement {
    Home,
    End,
}

impl Rope {
    pub fn buffer_range(&self) -> Range<usize> {
        0..self.len_chars().saturating_sub(1)
    }
}

impl Buffer {
    pub fn move_in_buffer(&mut self, movement: BufferMovement, extend: bool) {
        self.apply_movement(sel_mode(extend), |buffer, sel| {
            let Range { start, end } = buffer.buffer_range();
            sel.head = match movement {
                BufferMovement::Home => start,
                BufferMovement::End => end,
            };
            true
        });
    }

    /// goes to the first char of the last line
    pub fn goto_last_line(&mut self, extend: bool) {
        self.apply_movement(sel_mode(extend), |rope, sel| {
            let last_line = rope.len_lines().saturating_sub(1);
            sel.head = rope.line_to_char(last_line);
            true
        });
    }

    /// goes to the first char of the given 1-indexed line
    pub fn goto_line(&mut self, line: usize, extend: bool) {
        self.apply_movement(sel_mode(extend), |rope, sel| {
            let last_line = rope.len_lines().saturating_sub(1);
            let target = line.saturating_sub(1).min(last_line);
            sel.head = rope.line_to_char(target);
            true
        });
    }

    /// Jumps to the cursor position right after the last undo-stack entry
    /// (Kakoune's `g .`) - a no-op if nothing has been edited yet.
    pub fn goto_last_modification_position(&mut self, extend: bool) {
        let Some(target) = self
            .history
            .undo_stack
            .last()
            .and_then(|tx| tx.selections_after.first())
            .map(|sel| sel.head)
        else {
            return;
        };
        self.apply_movement(sel_mode(extend), |_, sel| {
            sel.head = target;
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::Selection;

    fn at(buf: &mut Buffer, pos: usize) {
        buf.selections = vec![Selection {
            anchor: pos,
            head: pos,
            pref_col: 0,
        }];
    }

    #[test]
    fn home_and_end_move_to_buffer_boundaries() {
        let mut buf = Buffer::from_str("hello\nworld");
        at(&mut buf, 3);

        buf.move_in_buffer(BufferMovement::End, false);
        assert_eq!(buf.selections[0].head, buf.rope.buffer_range().end);

        buf.move_in_buffer(BufferMovement::Home, false);
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn goto_last_line_lands_on_its_first_char_not_the_buffer_end() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 0);

        buf.goto_last_line(false);
        assert_eq!(buf.selections[0].head, 8); // 't' of "three"
    }

    #[test]
    fn goto_line_is_one_indexed_and_clamps_past_the_end() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 0);

        buf.goto_line(2, false);
        assert_eq!(buf.selections[0].head, 4); // 't' of "two"

        buf.goto_line(99, false);
        assert_eq!(buf.selections[0].head, 8); // clamped to the last line
    }

    #[test]
    fn goto_last_modification_position_returns_to_the_last_edit() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 5); // inside "two"
        buf.enter_insert_before();
        buf.insert_char('X');
        let edit_head = buf.selections[0].head;

        buf.goto_last_line(false);
        assert_ne!(buf.selections[0].head, edit_head);

        buf.goto_last_modification_position(false);
        assert_eq!(buf.selections[0].head, edit_head);
    }

    #[test]
    fn goto_last_modification_position_is_a_no_op_with_no_history() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 5);

        buf.goto_last_modification_position(false);
        assert_eq!(buf.selections[0].head, 5);
    }
}
