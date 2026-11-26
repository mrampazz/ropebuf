use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum BufferMovement {
    Home,
    End,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buffer::Buffer, selection::Selection};

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
