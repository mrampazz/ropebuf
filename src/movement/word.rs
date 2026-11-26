use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum WordMovement {
    NextStart,
    NextEnd,
    PrevStart,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        buffer::Buffer,
        rope::{ObjectMode, Rope, WordKind},
        selection::Selection,
    };

    fn at(buf: &mut Buffer, pos: usize) {
        buf.selections = vec![Selection {
            anchor: pos,
            head: pos,
            pref_col: 0,
        }];
    }

    #[test]
    fn word_range() {
        let r = Rope::from_str("pollo");
        assert_eq!(r.current_word(0, WordKind::Word), 0..5);
    }

    #[test]
    fn adjacent_word() {
        let r = Rope::from_str("ciao come stai");
        assert_eq!(r.next_word(0, WordKind::Word), 5..9);
        assert_eq!(r.prev_word(4, WordKind::Word), 0..4);
        assert_eq!(r.next_word(4, WordKind::Word), 5..9);
        assert_eq!(r.prev_word(9, WordKind::Word), 5..9);
    }

    #[test]
    fn big_word() {
        let r = Rope::from_str("word.exe");
        assert_eq!(r.current_word(0, WordKind::WORD), 0..8);
    }

    #[test]
    fn sub_word() {
        let r = Rope::from_str("camelCase snake_case");
        assert_eq!(r.next_word(0, WordKind::Sub), 5..9);
        assert_eq!(r.next_word(6, WordKind::Sub), 10..15);
    }

    #[test]
    fn w_from_whitespace_selects_the_whole_next_word() {
        // Whitespace isn't a word: starting on the separator skips it
        // entirely and selects the full next word, nothing before it.
        let mut buf = Buffer::from_str("ciao come");
        at(&mut buf, 4); // the space between the two words
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "come");
    }

    #[test]
    fn w_from_word_start_selects_word_plus_trailing_separator() {
        // Starting inside a word keeps the selection from the cursor's
        // position through the trailing separator, stopping just before the
        // next word starts (not entering it).
        let mut buf = Buffer::from_str("hello world hi");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello ");
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "world ");
    }

    #[test]
    fn b_from_next_word_start_selects_prev_word_plus_leading_separator() {
        // Starting right on the next word's first char jumps fully back:
        // the gap between the two words is reclaimed as "preceding
        // whitespace", but the char the cursor started on is not pulled in.
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 6); // 'w' of "world"
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello ");
    }

    #[test]
    fn b_mid_token_finishes_the_token_only() {
        // Still mid-token: finish backward to this word's own start, no
        // separator involved since no gap was crossed.
        let mut buf = Buffer::from_str(" hello world hi");
        at(&mut buf, 9); // 'r', middle of "world"
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "wor");
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello ");
    }

    #[test]
    fn b_three_presses_walks_use_rpc_colon_colon_client_backward() {
        let mut buf = Buffer::from_str("use rpc::client");
        at(&mut buf, 14); // 't', last char of "client"
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "client");
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "::");
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "rpc");
    }

    #[test]
    fn w_does_not_absorb_the_trailing_newline() {
        let mut buf = Buffer::from_str("hello\nworld");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn e_does_not_absorb_a_newline_crossed_while_jumping() {
        let mut buf = Buffer::from_str("hi\nworld");
        at(&mut buf, 1); // last char of "hi"
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "world");
    }

    #[test]
    fn e_keeps_leading_indentation_on_the_destination_line() {
        let mut buf = Buffer::from_str("hi\n  world");
        at(&mut buf, 1); // last char of "hi"
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "  world");
    }

    #[test]
    fn b_does_not_absorb_a_newline_crossed_while_jumping() {
        let mut buf = Buffer::from_str("hello\nworld");
        at(&mut buf, 6); // 'w' of "world"
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn e_from_word_start_finishes_current_word_only() {
        // Mid/at the start of a word: `e` just lands on this word's own end,
        // no separator is touched since nothing was jumped over.
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn e_from_word_end_jumps_to_next_word_end_with_leading_separator() {
        // Already at the current word's last char: jump the gap (captured
        // as "preceding whitespace") and land on the next word's last char.
        let mut buf = Buffer::from_str("hello world hi");
        at(&mut buf, 4); // last 'o' of "hello"
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), " world");
    }

    #[test]
    fn e_from_whitespace_selects_through_next_words_end() {
        let mut buf = Buffer::from_str("ciao come");
        at(&mut buf, 4); // the space between the two words
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "come");
    }

    #[test]
    fn w_from_last_char_of_token_adjacent_to_punctuation_enters_next_cleanly() {
        // "rpc" is directly followed by "::" with no whitespace in between.
        // At the LAST char of "rpc" there's nothing left of it to extend
        // through, so `w` jumps fully into "::" (anchor included) rather
        // than dragging the already-finished 'c' along as "c::".
        let mut buf = Buffer::from_str("rpc::client");
        at(&mut buf, 2); // last 'c' of "rpc"
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "::");
    }

    #[test]
    fn w_mid_token_adjacent_to_punctuation_finishes_the_token_only() {
        // Still mid-token (not yet at its last char): finish "rpc" itself,
        // but don't cross into "::" in the same press.
        let mut buf = Buffer::from_str("rpc::client");
        at(&mut buf, 1); // 'p', middle of "rpc"
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "pc");
    }

    #[test]
    fn w_four_presses_walks_use_rpc_colon_colon_client() {
        let mut buf = Buffer::from_str("use rpc::client");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "use ");
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "rpc");
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "::");
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "client");
    }

    #[test]
    fn shift_w_extends_head_without_moving_anchor() {
        let mut buf = Buffer::from_str("hello world next");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Word, false); // "hello "
        buf.move_word(WordMovement::NextStart, WordKind::Word, true); // extend
        assert_eq!(buf.get_selected_text(), "hello world "); // anchor stayed at 0
    }

    #[test]
    fn object_selection_selects_the_whole_word() {
        // Regression test: `apply_range` stores `head` as the last included
        // character, which silently under-selected by one char before
        // `Selection::range()` became an inclusive, closed interval.
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 0);
        buf.select_word_at(WordKind::Word, ObjectMode::Inner, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }
}
