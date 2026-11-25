use serde::{Deserialize, Serialize};

use crate::{
    buffer::{Buffer, SelectionMode},
    rope::Rope,
    utils::sel_mode,
};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum LineMovement {
    LineHome,
    LineEnd,
    LineHomeNoWhitespace,
}

impl Rope {
    /// returns the line start char
    pub fn line_start(&self, char_idx: usize) -> usize {
        let line_idx = self.char_to_line(char_idx.min(self.len_chars().saturating_sub(1)));

        self.line_to_char(line_idx)
    }

    /// returns the first non whitespace char index
    pub fn line_start_no_whitespace(&self, char_idx: usize) -> usize {
        let line_start_char = self.line_start(char_idx);
        let mut slice = self.chars_at(line_start_char);
        if let Some(pos) = slice.position(|c| !c.is_whitespace()) {
            line_start_char + pos
        } else {
            line_start_char
        }
    }

    /// returns the line end char
    pub fn line_end(&self, char_idx: usize) -> usize {
        let len = self.len_chars();
        if len == 0 {
            return 0;
        }

        let line_idx = self.char_to_line(char_idx.min(len - 1));
        if line_idx + 1 >= self.len_lines() {
            return len;
        }

        let next_line_start = self.line_to_char(line_idx + 1);
        if next_line_start > 0 && self.char(next_line_start - 1) == '\n' {
            next_line_start - 1
        } else {
            next_line_start
        }
    }

    /// returns the char index of the first non whitespace in a line
    pub fn first_non_whitespace_of_line(&self, char_idx: usize) -> Option<usize> {
        let start = self.line_start(char_idx);
        let end = self.line_end(char_idx);
        let mut pos = start;
        while pos < end {
            if !self.char(pos).is_whitespace() {
                return Some(pos);
            }
            pos = self.next_grapheme(pos);
        }
        None
    }

    fn line_bounds_inclusive(&self, start_line: usize, end_line: usize) -> (usize, usize) {
        let start = self.line_to_char(start_line);
        let end = if end_line + 1 < self.len_lines() {
            self.line_to_char(end_line + 1).saturating_sub(1)
        } else {
            self.len_chars().saturating_sub(1)
        };
        (start, end.max(start))
    }

    fn extend_line_selection(&self, anchor: usize, head: usize) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
        }

        let start_idx = anchor.min(head).min(len - 1);
        let end_idx = anchor.max(head).min(len - 1);

        let start_line = self.char_to_line(start_idx);
        let end_line = self.char_to_line(end_idx);

        let (line_start, line_end) = self.line_bounds_inclusive(start_line, end_line);
        let is_full_line_selection = anchor.min(head) == line_start && anchor.max(head) == line_end;

        if is_full_line_selection && end_line + 1 < self.len_lines() {
            self.line_bounds_inclusive(start_line, end_line + 1)
        } else {
            (line_start, line_end)
        }
    }
}

impl Buffer {
    pub fn move_in_line(&mut self, movement: LineMovement, extend: bool) {
        self.apply_movement(sel_mode(extend), |buffer, sel| {
            sel.head = match movement {
                LineMovement::LineHome => buffer.line_start(sel.head),
                LineMovement::LineEnd => buffer.line_end(sel.head),
                LineMovement::LineHomeNoWhitespace => buffer.line_start_no_whitespace(sel.head),
            };
            true
        });
    }

    pub fn select_line(&mut self) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.extend_line_selection(sel.anchor, sel.head);
            sel.anchor = start;
            sel.head = end;
            true
        });
    }

    /// Shrinks each selection down to the full lines it fully contains,
    /// dropping any partial line at either edge and the last line's
    /// trailing end-of-line (Kakoune's `<a-x>`, the inverse of `select_line`).
    pub fn trim_line_selection(&mut self) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let len = rope.len_chars();
            if len == 0 {
                return false;
            }
            let start = sel.start().min(len - 1);
            let end_excl = (sel.end() + 1).min(len);

            let start_line = rope.char_to_line(start);
            let first_full_line = if start == rope.line_to_char(start_line) {
                start_line
            } else {
                start_line + 1
            };

            let end_line = rope.char_to_line((end_excl - 1).min(len - 1));
            let next_line_start = if end_line + 1 < rope.len_lines() {
                rope.line_to_char(end_line + 1)
            } else {
                len
            };
            let last_full_line = if end_excl >= next_line_start {
                Some(end_line)
            } else {
                end_line.checked_sub(1)
            };
            let Some(last_full_line) = last_full_line else {
                return false;
            };
            if first_full_line > last_full_line || first_full_line >= rope.len_lines() {
                return false;
            }

            let new_start = rope.line_to_char(first_full_line);
            let new_end_excl = rope.line_end(rope.line_to_char(last_full_line));
            sel.anchor = new_start;
            sel.head = new_end_excl.saturating_sub(1).max(new_start);
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
    fn first_non_whitespace_of_line_spaces() {
        let r = Rope::from_str("    lmfao");
        assert_eq!(r.first_non_whitespace_of_line(0), Some(4));
    }

    #[test]
    fn first_non_whitespace_of_line_empty() {
        let r = Rope::from_str("       ");
        assert_eq!(r.first_non_whitespace_of_line(0), None);
    }

    #[test]
    fn line_end_then_line_home_moves_to_line_boundaries() {
        let mut buf = Buffer::from_str("hello world\nnext line");
        at(&mut buf, 3); // middle of "hello", on the first line
        buf.move_in_line(LineMovement::LineEnd, false);
        assert_eq!(buf.selections[0].head, 11); // end of "hello world"

        buf.move_in_line(LineMovement::LineHome, false);
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn line_home_no_whitespace_skips_leading_indent() {
        let mut buf = Buffer::from_str("    indented");
        at(&mut buf, 10);
        buf.move_in_line(LineMovement::LineHomeNoWhitespace, false);
        assert_eq!(buf.selections[0].head, 4);
    }

    #[test]
    fn select_line_first_press_selects_enclosing_line() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 5); // 'w' of "two"
        buf.select_line();
        assert_eq!(buf.get_selected_text(), "two\n");
    }

    #[test]
    fn select_line_consecutive_presses_select_lines_below() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 5); // 'w' of "two"
        buf.select_line(); // "two\n"
        buf.select_line(); // extends down to "three"
        assert_eq!(buf.get_selected_text(), "two\nthree");
    }

    #[test]
    fn select_line_expands_partial_multi_line_selection() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        buf.selections = vec![Selection {
            anchor: 1, // inside "one"
            head: 5,   // inside "two"
            pref_col: 0,
        }];
        buf.select_line();
        assert_eq!(buf.get_selected_text(), "one\ntwo\n");
    }

    #[test]
    fn select_line_on_last_line_has_no_trailing_newline_and_stops_extending() {
        let mut buf = Buffer::from_str("one\ntwo");
        at(&mut buf, 5); // 'w' of "two"
        buf.select_line();
        assert_eq!(buf.get_selected_text(), "two");
        buf.select_line(); // nothing below to extend to
        assert_eq!(buf.get_selected_text(), "two");
    }
}
