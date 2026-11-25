use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::{
    buffer::{Buffer, SelectionMode},
    rope::Rope,
    textobject::ObjectMode,
};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum WordKind {
    Default,
    Big,
    Sub,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum WordMovement {
    NextStart,
    NextEnd,
    PrevStart,
}

pub(crate) fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn is_big_word(ch: char) -> bool {
    !ch.is_whitespace()
}

fn is_line_whitespace(ch: char) -> bool {
    ch.is_whitespace() && ch != '\n'
}

fn is_boundary(left: char, right: char, kind: WordKind) -> bool {
    match kind {
        WordKind::Big => is_big_word(left) != is_big_word(right),
        WordKind::Default => {
            // boundary happens if one is a word char and the other isn't,
            // or if both are whitespace but we transitioned across them.
            if is_word(left) != is_word(right) {
                return true;
            }
            if left.is_whitespace() != right.is_whitespace() {
                return true;
            }
            false
        }
        WordKind::Sub => {
            if is_boundary(left, right, WordKind::Default) {
                return true;
            }
            // snake case transitions
            if left == '_' || right == '_' {
                return true;
            }
            // camel case transitions
            if left.is_lowercase() && right.is_uppercase() {
                return true;
            }
            // type changes (e.g., letters to digits like 'a' -> '1')
            if left.is_alphabetic() != right.is_alphabetic()
                || left.is_numeric() != right.is_numeric()
            {
                return true;
            }
            false
        }
    }
}

impl Rope {
    pub fn current_word(&self, char_idx: usize, kind: WordKind) -> Range<usize> {
        let len = self.len_chars();

        if char_idx >= len || len == 0 {
            return char_idx..char_idx;
        }

        let mut start = char_idx;
        while start > 0 {
            let prev = self.prev_grapheme(start);
            let prev_char = self.char(prev);
            let current_char = self.char(start);
            if is_boundary(prev_char, current_char, kind) {
                break;
            }
            start = prev;
        }

        let mut end = char_idx;
        while end < len {
            let current_char = self.char(end);
            let next_idx = self.next_grapheme(end);

            if next_idx >= len {
                end = len;
                break;
            }
            let next_char = self.char(next_idx);

            if is_boundary(current_char, next_char, kind) {
                end = next_idx;
                break;
            }
            end = next_idx;
        }

        start..end
    }

    pub fn next_word(&self, char_idx: usize, kind: WordKind) -> Range<usize> {
        let len = self.len_chars();
        if char_idx >= len {
            return len..len;
        }

        let current = self.current_word(char_idx, kind);

        let mut pos = if char_idx < current.end {
            current.end
        } else {
            char_idx
        };

        while pos < len && self.char(pos).is_whitespace() {
            pos = self.next_grapheme(pos);
        }

        if pos >= len {
            return len..len;
        }

        self.current_word(pos, kind)
    }

    pub fn prev_word(&self, char_idx: usize, kind: WordKind) -> Range<usize> {
        if char_idx == 0 {
            return 0..0;
        }

        let current = self.current_word(char_idx, kind);

        let mut pos = if char_idx >= current.start && current.start > 0 {
            self.prev_grapheme(current.start)
        } else {
            char_idx
        };

        while pos > 0 && self.char(pos).is_whitespace() {
            pos = self.prev_grapheme(pos);
        }

        if pos == 0 && self.char(0).is_whitespace() {
            return 0..0;
        }

        self.current_word(pos, kind)
    }

    fn skip_line_whitespace_forward(&self, start: usize, bound: usize) -> usize {
        let mut p = start;
        while p < bound && is_line_whitespace(self.char(p)) {
            p = self.next_grapheme(p);
        }
        p.saturating_sub(1)
    }

    fn skip_line_whitespace_backward(&self, start: usize, bound: usize) -> usize {
        let mut p = start;
        while p > bound && is_line_whitespace(self.char(p - 1)) {
            p = self.prev_grapheme(p);
        }
        p
    }

    pub fn select_to_next_word_start(&self, char_idx: usize, kind: WordKind) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (char_idx, char_idx);
        }
        let at = char_idx.min(len - 1);
        let current = self.current_word(char_idx, kind);
        let nothing_left_here =
            self.char(at).is_whitespace() || char_idx >= current.end.saturating_sub(1);

        if nothing_left_here {
            let next = self.next_word(char_idx, kind);
            if next.start >= next.end {
                return (char_idx, char_idx);
            }
            return (next.start, self.skip_line_whitespace_forward(next.end, len));
        }

        let head = self.skip_line_whitespace_forward(current.end, len);
        (char_idx, head.max(char_idx))
    }

    pub fn select_to_prev_word_start(&self, char_idx: usize, kind: WordKind) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (char_idx, char_idx);
        }
        let current = self.current_word(char_idx, kind);
        let nothing_left_here =
            self.char(char_idx.min(len - 1)).is_whitespace() || char_idx <= current.start;

        if nothing_left_here {
            let prev = self.prev_word(char_idx, kind);
            if prev.start >= prev.end {
                return (char_idx, char_idx); // nothing before - start of buffer
            }
            return (
                self.skip_line_whitespace_forward(prev.end, current.start),
                prev.start,
            );
        }

        let head = current.start;
        (char_idx.max(head), head)
    }

    pub fn select_to_next_word_end(&self, char_idx: usize, kind: WordKind) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (char_idx, char_idx);
        }
        let at = char_idx.min(len - 1);
        let current = self.current_word(char_idx, kind);
        let nothing_left_here =
            self.char(at).is_whitespace() || char_idx >= current.end.saturating_sub(1);

        if nothing_left_here {
            let next = self.next_word(char_idx, kind);
            if next.start >= next.end {
                return (char_idx, char_idx); // nothing after - end of buffer
            }
            return (
                self.skip_line_whitespace_backward(next.start, current.end),
                next.end.saturating_sub(1),
            );
        }

        let head = current.end.saturating_sub(1);
        (char_idx.min(head), head)
    }

    pub fn select_word_object(
        &self,
        char_idx: usize,
        kind: WordKind,
        mode: ObjectMode,
    ) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
        }

        let word_range = self.current_word(char_idx, kind);

        match mode {
            ObjectMode::Inner => (word_range.start, word_range.end),
            ObjectMode::Around => {
                let mut start = word_range.start;
                let mut end = word_range.end;

                while end < len && self.char(end).is_whitespace() {
                    end += 1;
                }

                if end == word_range.end {
                    while start > 0 && self.char(start - 1).is_whitespace() {
                        start -= 1;
                    }
                }

                (start, end)
            }
        }
    }
}

impl Buffer {
    pub fn move_word(&mut self, movement: WordMovement, kind: WordKind, extend: bool) {
        let selector = match movement {
            WordMovement::NextStart => Rope::select_to_next_word_start,
            WordMovement::PrevStart => Rope::select_to_prev_word_start,
            WordMovement::NextEnd => Rope::select_to_next_word_end,
        };

        self.apply_movement(SelectionMode::SetExplicit, |buffer, sel| {
            let (anchor, head) = selector(buffer, sel.head, kind);
            if extend {
                sel.head = head;
            } else {
                sel.anchor = anchor;
                sel.head = head;
            }
            true
        });
    }

    pub fn select_word_at(&mut self, kind: WordKind, mode: ObjectMode, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.select_word_object(sel.head, kind, mode);
            sel.apply_range(start, end, extend);
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
    fn word_range() {
        let r = Rope::from_str("pollo");
        assert_eq!(r.current_word(0, WordKind::Default), 0..5);
    }

    #[test]
    fn adjacent_word() {
        let r = Rope::from_str("ciao come stai");
        assert_eq!(r.next_word(0, WordKind::Default), 5..9);
        assert_eq!(r.prev_word(4, WordKind::Default), 0..4);
        assert_eq!(r.next_word(4, WordKind::Default), 5..9);
        assert_eq!(r.prev_word(9, WordKind::Default), 5..9);
    }

    #[test]
    fn big_word() {
        let r = Rope::from_str("word.exe");
        assert_eq!(r.current_word(0, WordKind::Big), 0..8);
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
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "come");
    }

    #[test]
    fn w_from_word_start_selects_word_plus_trailing_separator() {
        // Starting inside a word keeps the selection from the cursor's
        // position through the trailing separator, stopping just before the
        // next word starts (not entering it).
        let mut buf = Buffer::from_str("hello world hi");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "hello ");
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "world ");
    }

    #[test]
    fn b_from_next_word_start_selects_prev_word_plus_leading_separator() {
        // Starting right on the next word's first char jumps fully back:
        // the gap between the two words is reclaimed as "preceding
        // whitespace", but the char the cursor started on is not pulled in.
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 6); // 'w' of "world"
        buf.move_word(WordMovement::PrevStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "hello ");
    }

    #[test]
    fn b_mid_token_finishes_the_token_only() {
        // Still mid-token: finish backward to this word's own start, no
        // separator involved since no gap was crossed.
        let mut buf = Buffer::from_str(" hello world hi");
        at(&mut buf, 9); // 'r', middle of "world"
        buf.move_word(WordMovement::PrevStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "wor");
        buf.move_word(WordMovement::PrevStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "hello ");
    }

    #[test]
    fn b_three_presses_walks_use_rpc_colon_colon_client_backward() {
        let mut buf = Buffer::from_str("use rpc::client");
        at(&mut buf, 14); // 't', last char of "client"
        buf.move_word(WordMovement::PrevStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "client");
        buf.move_word(WordMovement::PrevStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "::");
        buf.move_word(WordMovement::PrevStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "rpc");
    }

    #[test]
    fn w_does_not_absorb_the_trailing_newline() {
        let mut buf = Buffer::from_str("hello\nworld");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn e_does_not_absorb_a_newline_crossed_while_jumping() {
        let mut buf = Buffer::from_str("hi\nworld");
        at(&mut buf, 1); // last char of "hi"
        buf.move_word(WordMovement::NextEnd, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "world");
    }

    #[test]
    fn e_keeps_leading_indentation_on_the_destination_line() {
        let mut buf = Buffer::from_str("hi\n  world");
        at(&mut buf, 1); // last char of "hi"
        buf.move_word(WordMovement::NextEnd, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "  world");
    }

    #[test]
    fn b_does_not_absorb_a_newline_crossed_while_jumping() {
        let mut buf = Buffer::from_str("hello\nworld");
        at(&mut buf, 6); // 'w' of "world"
        buf.move_word(WordMovement::PrevStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn e_from_word_start_finishes_current_word_only() {
        // Mid/at the start of a word: `e` just lands on this word's own end,
        // no separator is touched since nothing was jumped over.
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextEnd, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn e_from_word_end_jumps_to_next_word_end_with_leading_separator() {
        // Already at the current word's last char: jump the gap (captured
        // as "preceding whitespace") and land on the next word's last char.
        let mut buf = Buffer::from_str("hello world hi");
        at(&mut buf, 4); // last 'o' of "hello"
        buf.move_word(WordMovement::NextEnd, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), " world");
    }

    #[test]
    fn e_from_whitespace_selects_through_next_words_end() {
        let mut buf = Buffer::from_str("ciao come");
        at(&mut buf, 4); // the space between the two words
        buf.move_word(WordMovement::NextEnd, WordKind::Default, false);
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
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "::");
    }

    #[test]
    fn w_mid_token_adjacent_to_punctuation_finishes_the_token_only() {
        // Still mid-token (not yet at its last char): finish "rpc" itself,
        // but don't cross into "::" in the same press.
        let mut buf = Buffer::from_str("rpc::client");
        at(&mut buf, 1); // 'p', middle of "rpc"
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "pc");
    }

    #[test]
    fn w_four_presses_walks_use_rpc_colon_colon_client() {
        let mut buf = Buffer::from_str("use rpc::client");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "use ");
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "rpc");
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "::");
        buf.move_word(WordMovement::NextStart, WordKind::Default, false);
        assert_eq!(buf.get_selected_text(), "client");
    }

    #[test]
    fn shift_w_extends_head_without_moving_anchor() {
        let mut buf = Buffer::from_str("hello world next");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Default, false); // "hello "
        buf.move_word(WordMovement::NextStart, WordKind::Default, true); // extend
        assert_eq!(buf.get_selected_text(), "hello world "); // anchor stayed at 0
    }

    #[test]
    fn object_selection_selects_the_whole_word() {
        // Regression test: `apply_range` stores `head` as the last included
        // character, which silently under-selected by one char before
        // `Selection::range()` became an inclusive, closed interval.
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 0);
        buf.select_word_at(WordKind::Default, ObjectMode::Inner, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }
}
