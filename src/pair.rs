use std::ops::Range;

use crate::{ObjectMode, Rope};

const PAIR_SEARCH_LIMIT: usize = 100_000;

impl Rope {
    /// Returns the optional range of a delimited object such as quotes / parentheses etc.. at the given pos, if not found goes to the next pair
    pub fn pair_object_at(
        &self,
        char_idx: usize,
        open: char,
        close: char,
        mode: ObjectMode,
    ) -> Option<Range<usize>> {
        if self.len_chars() == 0 {
            return None;
        }
        let idx = char_idx.min(self.len_chars() - 1);

        let (s, e) = if open == close {
            self.same_delimiter_pair_range(idx, open)?
        } else {
            self.enclosing_pair(idx, open, close)
                .or_else(|| self.next_pair(idx, open, close))?
        };

        match mode {
            ObjectMode::Inner => Some(s + 1..e),
            ObjectMode::Around => Some(s..e + 1),
        }
    }

    fn same_delimiter_pair_range(&self, char_idx: usize, del: char) -> Option<(usize, usize)> {
        let line = self.char_to_line(char_idx);
        let line_start = self.line_to_char(line);
        let mut delimiters = Vec::new();
        let mut escaped = false;
        for (i, ch) in self.line(line).chars().enumerate() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == del {
                delimiters.push(line_start + i);
            }
        }
        delimiters
            .chunks_exact(2)
            .map(|p| (p[0], p[1]))
            .find(|&(_, close_idx)| close_idx >= char_idx)
    }

    /// Innermost `open`..`close` around `idx` (a cursor on either bracket counts as inside).
    fn enclosing_pair(&self, idx: usize, open: char, close: char) -> Option<(usize, usize)> {
        let mut depth = 0;
        let mut open_idx = None;
        for (k, ch) in self
            .chars_at(idx + 1)
            .reversed()
            .take(PAIR_SEARCH_LIMIT)
            .enumerate()
        {
            if ch == close && k != 0 {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    open_idx = Some(idx - k);
                    break;
                }
                depth -= 1;
            }
        }
        let open_idx = open_idx?;
        Some((open_idx, self.matching_close(open_idx, open, close)?))
    }

    fn next_pair(&self, idx: usize, open: char, close: char) -> Option<(usize, usize)> {
        let open_idx = idx
            + 1
            + self
                .chars_at(idx + 1)
                .take(PAIR_SEARCH_LIMIT)
                .position(|ch| ch == open)?;
        Some((open_idx, self.matching_close(open_idx, open, close)?))
    }

    fn matching_close(&self, open_idx: usize, open: char, close: char) -> Option<usize> {
        let mut depth = 0;
        for (k, ch) in self
            .chars_at(open_idx + 1)
            .take(PAIR_SEARCH_LIMIT)
            .enumerate()
        {
            if ch == open {
                depth += 1;
            } else if ch == close {
                if depth == 0 {
                    return Some(open_idx + 1 + k);
                }
                depth -= 1;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pair() {
        let r = Rope::from_str("\"clear\"");
        assert_eq!(r.pair_object_at(0, '"', '"', ObjectMode::Inner), Some(1..6));
        assert_eq!(
            r.pair_object_at(0, '"', '"', ObjectMode::Around),
            Some(0..7)
        );
    }

    #[test]
    fn parentheses_next_line() {
        let r = Rope::from_str("ciao\n(clear)");
        assert_eq!(
            r.pair_object_at(0, '(', ')', ObjectMode::Inner),
            Some(6..11)
        );
    }
    #[test]
    fn parentheses_group() {
        let r = Rope::from_str("(food\n(clear))");
        assert_eq!(
            r.pair_object_at(0, '(', ')', ObjectMode::Inner),
            Some(1..13)
        );
    }

    #[test]
    fn quotes_next_line() {
        let r = Rope::from_str("food\n\"clear\"");
        assert_eq!(r.pair_object_at(0, '"', '"', ObjectMode::Inner), None);
    }

    #[test]
    fn parentheses() {
        let r = Rope::from_str("(clear\nclear)");
        assert_eq!(
            r.pair_object_at(0, '(', ')', ObjectMode::Inner),
            Some(1..12)
        );
        assert_eq!(
            r.pair_object_at(0, '(', ')', ObjectMode::Around),
            Some(0..13)
        );
    }
}
