use std::ops::Range;

use crate::{ObjectMode, Rope};

pub enum WordKind {
    Word,
    WORD,
    Sub,
}

impl Rope {
    /// Returns the range of the sentence object at the given char index
    pub fn sentence_object_at(&self, char_idx: usize, mode: ObjectMode) -> Range<usize> {
        let len = self.len_chars();
        if len == 0 {
            return 0..0;
        }
        let head = char_idx.min(len - 1);
        let mut start = head;
        let mut end = head;

        while start > 0 {
            let ch = self.char(start - 1);
            if (ch == '.' || ch == '!' || ch == '?') && start < head {
                break;
            }
            start -= 1;
        }
        while start < head && self.char(start).is_whitespace() {
            start += 1;
        }

        while end < len {
            let ch = self.char(end);
            end += 1;
            if ch == '.' || ch == '!' || ch == '?' {
                break;
            }
        }

        if mode == ObjectMode::Around {
            while end < len && self.char(end) == ' ' {
                end += 1;
            }
        }
        start..end
    }

    /// Returns the range of the paragraph object at the given char index
    pub fn paragraph_object_at(&self, char_idx: usize, mode: ObjectMode) -> Range<usize> {
        let len = self.len_chars();
        if len == 0 {
            return 0..0;
        }
        let curr_line = self.char_to_line(char_idx);
        let mut start_line = curr_line;
        let mut end_line = curr_line;

        while start_line > 0 && !self.line_is_blank(start_line - 1) {
            start_line -= 1;
        }
        while end_line + 1 < self.len_lines() && !self.line_is_blank(end_line + 1) {
            end_line += 1;
        }

        let start = self.line_to_char(start_line);
        let mut end = self.line_to_char(end_line + 1);

        if mode == ObjectMode::Around && end < len {
            while end < len && self.char(end) == '\n' {
                end += 1;
            }
        }
        start..end
    }

    /// Returns the range of the indent object at the given char index
    pub fn indent_object_at(
        &self,
        char_idx: usize,
        mode: ObjectMode,
        tab_width: usize,
    ) -> Range<usize> {
        let len = self.len_chars();
        if len == 0 {
            return 0..0;
        }
        let line_idx = self.char_to_line(char_idx);
        let base_indent = self.line_indent(line_idx, tab_width);

        let mut start_line = line_idx;
        let mut end_line = line_idx;

        while start_line > 0 && self.line_indent(start_line - 1, tab_width) >= base_indent {
            start_line -= 1;
        }
        while end_line + 1 < self.len_lines()
            && self.line_indent(end_line + 1, tab_width) >= base_indent
        {
            end_line += 1;
        }

        let start = self.line_to_char(start_line);
        let mut end = self.line_to_char(end_line + 1);

        if mode == ObjectMode::Inner {
            end = end.saturating_sub(1);
        }
        start..end
    }

    pub fn word_at(&self, char_idx: usize, kind: WordKind, mode: ObjectMode) -> Range<usize> {
        match mode {
            ObjectMode::Inner => self.inner_word_at(char_idx, kind),
            ObjectMode::Around => self.around_word_at(char_idx, kind),
        }
    }

    /// Returns the inclusive range of the given WordKind at the char_idx
    pub fn inner_word_at(&self, char_idx: usize, kind: WordKind) -> Range<usize> {
        let len = self.len_chars();

        if char_idx >= len || len == 0 {
            return char_idx..char_idx;
        }

        let mut start = char_idx;
        while start > 0 {
            let prev = self.prev_grapheme(start);
            let prev_char = self.char(prev);
            let current_char = self.char(start);
            if is_boundary(prev_char, current_char, &kind) {
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

            if is_boundary(current_char, next_char, &kind) {
                end = next_idx;
                break;
            }
            end = next_idx;
        }

        start..end
    }

    /// Returns the range around the word (containing next whitespace)
    pub fn around_word_at(&self, char_idx: usize, kind: WordKind) -> Range<usize> {
        let len = self.len_chars();
        if len == 0 {
            return 0..0;
        }

        let word_range = self.inner_word_at(char_idx, kind);
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

        start..end
    }
}

pub fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub fn is_big_word(ch: char) -> bool {
    !ch.is_whitespace()
}

pub fn is_boundary(left: char, right: char, kind: &WordKind) -> bool {
    match kind {
        WordKind::WORD => is_big_word(left) != is_big_word(right),
        WordKind::Word => {
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
            if is_boundary(left, right, &WordKind::Word) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn word() {
        let r = Rope::from_str(" clear ");
        assert_eq!(r.inner_word_at(1, WordKind::Word), 1..6);
        assert_eq!(r.around_word_at(1, WordKind::Word), 0..7);
    }
}
