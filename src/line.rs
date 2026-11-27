use crate::Rope;

impl Rope {
    /// Returns the line indent count for the line at line_idx
    pub fn line_indent(&self, line_idx: usize, tab_width: usize) -> usize {
        let line = self.rope.line(line_idx);
        let mut count = 0;
        for ch in line.chars() {
            if ch == ' ' {
                count += 1;
            } else if ch == '\t' {
                count += tab_width;
            } else {
                break;
            }
        }
        count
    }

    /// Returns true if the line is fully blank
    pub fn line_is_blank(&self, line_idx: usize) -> bool {
        let line = self.rope.line(line_idx);
        line.chars().all(|c| c.is_whitespace())
    }

    /// returns the char index of the first index in a line
    pub fn line_start_at(&self, char_idx: usize) -> usize {
        let line_idx = self.char_to_line(char_idx.min(self.len_chars().saturating_sub(1)));
        self.line_to_char(line_idx)
    }

    /// returns the line end char
    pub fn line_end_at(&self, char_idx: usize) -> usize {
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
    pub fn line_start_no_whitespace_at(&self, char_idx: usize) -> Option<usize> {
        let start = self.line_start_at(char_idx);
        let end = self.line_end_at(char_idx);
        let mut pos = start;
        while pos < end {
            if !self.char(pos).is_whitespace() {
                return Some(pos);
            }
            pos = self.next_grapheme(pos);
        }
        None
    }
}
