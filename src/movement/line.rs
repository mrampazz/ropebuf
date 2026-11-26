use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum LineMovement {
    LineHome,
    LineEnd,
    LineHomeNoWhitespace,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buffer::Buffer, rope::Rope, selection::Selection};

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
        assert_eq!(r.first_non_whitespace_of_line(0), 4);
    }

    #[test]
    fn first_non_whitespace_of_line_empty() {
        let r = Rope::from_str("       ");
        assert_eq!(r.first_non_whitespace_of_line(0), 0);
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
