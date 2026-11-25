use serde::{Deserialize, Serialize};

use crate::{
    buffer::{Buffer, SelectionMode},
    rope::Rope,
};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindKind {
    To,
    Until,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindDirection {
    Forward,
    Backward,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharSearch {
    pub kind: FindKind,
    pub direction: FindDirection,
    pub target: char,
}

impl Rope {
    /// returns the forward char index of the nth occurrence of the target char
    fn find_char_forward(&self, char_idx: usize, target: char, count: usize) -> Option<usize> {
        let len = self.len_chars();
        let mut pos = char_idx;
        let mut remaining = count;
        while pos < len {
            pos = self.next_grapheme(pos);
            if pos >= len {
                return None;
            }
            if self.char(pos) == target {
                remaining -= 1;
                if remaining == 0 {
                    return Some(pos);
                }
            }
        }
        None
    }

    /// returns the backward char index of the nth occurrence of the target char
    fn find_char_backward(&self, char_idx: usize, target: char, count: usize) -> Option<usize> {
        let mut pos = char_idx;
        let mut remaining = count;
        while pos > 0 {
            pos = self.prev_grapheme(pos);
            if self.char(pos) == target {
                remaining -= 1;
                if remaining == 0 {
                    return Some(pos);
                }
            }
        }
        None
    }
}

impl Buffer {
    pub fn find_char(&mut self, search: CharSearch, count: Option<usize>, extend: bool) {
        self.last_char_search = Some(search);
        self.run_char_search(search, count.unwrap_or(1), extend, false);
    }

    pub fn repeat_char_search(&mut self, extend: bool) {
        if let Some(search) = self.last_char_search {
            self.run_char_search(search, 1, extend, true);
        }
    }

    fn run_char_search(&mut self, search: CharSearch, count: usize, extend: bool, is_repeat: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let from = if is_repeat && search.kind == FindKind::Until {
                match search.direction {
                    FindDirection::Forward => rope.next_grapheme(sel.head),
                    FindDirection::Backward => rope.prev_grapheme(sel.head),
                }
            } else {
                sel.head
            };

            let found = match search.direction {
                FindDirection::Forward => rope.find_char_forward(from, search.target, count),
                FindDirection::Backward => rope.find_char_backward(from, search.target, count),
            };

            if let Some(found) = found {
                let target = match (search.kind, search.direction) {
                    (FindKind::Until, FindDirection::Forward) => rope.prev_grapheme(found),
                    (FindKind::Until, FindDirection::Backward) => rope.next_grapheme(found),
                    (FindKind::To, _) => found,
                };
                if extend {
                    sel.head = target;
                } else {
                    sel.anchor = sel.head;
                    sel.head = target;
                }
            }
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

    fn search(kind: FindKind, direction: FindDirection, target: char) -> CharSearch {
        CharSearch {
            kind,
            direction,
            target,
        }
    }

    #[test]
    fn f_lands_on_the_target_char() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 5); // 't' of "the"
    }

    #[test]
    fn t_lands_just_short_of_the_target_char() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::Until, FindDirection::Forward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 4); // the space right before "the"
    }

    #[test]
    fn capital_f_searches_backward_landing_on_target() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 13); // the 'e' of "target", one before its trailing 't'
        buf.find_char(
            search(FindKind::To, FindDirection::Backward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 9); // the 't' that starts "target"
    }

    #[test]
    fn capital_t_searches_backward_landing_just_short_of_target() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 13); // the 'e' of "target", one before its trailing 't'
        buf.find_char(
            search(FindKind::Until, FindDirection::Backward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 10); // right after that 't'
    }

    #[test]
    fn count_skips_to_the_nth_occurrence() {
        let mut buf = Buffer::from_str("a-b-c-d");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, '-'),
            Some(3),
            false,
        );
        assert_eq!(buf.selections[0].head, 5); // the 3rd '-'
    }

    #[test]
    fn missing_target_leaves_selection_untouched() {
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, 'z'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn non_extending_search_selects_from_the_old_head_to_the_target() {
        // Like `w`/`b`/`e`, a non-extending search still selects the text
        // it swept over - it doesn't collapse to a bare cursor on the
        // target the way plain h/j/k/l movement would.
        let mut buf = Buffer::from_str("find the target");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 2,
            pref_col: 0,
        }];
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].anchor, 2); // the old head
        assert_eq!(buf.selections[0].head, 5);
    }

    #[test]
    fn extending_search_keeps_the_anchor() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, 't'),
            None,
            true,
        );
        assert_eq!(buf.selections[0].anchor, 0);
        assert_eq!(buf.selections[0].head, 5);
    }

    #[test]
    fn repeat_reuses_the_last_search() {
        let mut buf = Buffer::from_str("a-b-c-d");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, '-'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 1);
        buf.repeat_char_search(false);
        assert_eq!(buf.selections[0].head, 3);
        buf.repeat_char_search(false);
        assert_eq!(buf.selections[0].head, 5);
    }

    #[test]
    fn repeat_with_no_prior_search_is_a_no_op() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 0);
        buf.repeat_char_search(false);
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn repeating_until_skips_past_the_char_it_already_stopped_before() {
        // `t-` from the start stops right before the first '-'; repeating
        // it naively would re-find that same '-' and not move at all, so
        // the repeat has to step past it first (matching vim/kakoune).
        let mut buf = Buffer::from_str("a-b-c-d");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::Until, FindDirection::Forward, '-'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 0); // already right before the 1st '-'
        buf.repeat_char_search(false);
        assert_eq!(buf.selections[0].head, 2); // right before the 2nd '-'
    }
}
