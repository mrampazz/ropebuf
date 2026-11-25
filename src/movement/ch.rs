use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::{buffer::Buffer, rope::Rope, utils::sel_mode};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Rope {
    /// returns the char_idx of the next grapheme given a char index
    pub fn next_grapheme(&self, char_idx: usize) -> usize {
        if char_idx >= self.len_chars() {
            return char_idx;
        }

        // Take a small slice ahead of the cursor to find the next boundary (in chars)
        let search_range = 10.min(self.len_chars() - char_idx);
        let text_slice = self.slice(char_idx..char_idx + search_range);
        let text_str = text_slice.to_string(); // Temporary small allocation

        if let Some(grapheme) = text_str.graphemes(true).next() {
            return char_idx + grapheme.chars().count();
        }
        char_idx + 1
    }

    /// returns the char_idx of the prev grapheme given a char index
    pub fn prev_grapheme(&self, char_idx: usize) -> usize {
        if char_idx == 0 {
            return 0;
        }

        // Take a small slice behind the cursor (in chars)
        let search_range = char_idx.min(10);
        let text_slice = self.slice(char_idx - search_range..char_idx);
        let text_str = text_slice.to_string();

        if let Some(grapheme) = text_str.graphemes(true).last() {
            return char_idx - grapheme.chars().count();
        }
        char_idx - 1
    }

    /// returns the char_idx of the grapheme above the given a char index
    pub fn above_grapheme(&self, char_idx: usize, pref_col: usize) -> usize {
        let line_idx = self.char_to_line(char_idx);

        if line_idx == 0 {
            return char_idx;
        }
        // get the previous line
        let prev_line_idx = line_idx - 1;
        let prev_line_start = self.line_to_char(prev_line_idx);
        let prev_line = self.line(prev_line_idx);

        // calculate previous line length excluding trailing newlines (\n only as we normalise them)
        let mut prev_len = prev_line.len_chars();
        if prev_len > 0 && prev_line.char(prev_len - 1) == '\n' {
            prev_len -= 1;
        }

        // clamp the column to the previous line's length and return new char index
        let target_col = std::cmp::min(pref_col, prev_len);
        prev_line_start + target_col
    }

    /// returns the char_idx of the grapheme below the given a char index
    pub fn below_grapheme(&self, char_idx: usize, pref_col: usize) -> usize {
        let line_idx = self.char_to_line(char_idx);
        let total_lines = self.len_lines();

        if line_idx + 1 >= total_lines {
            return char_idx;
        }

        // get the next line
        let next_line_idx = line_idx + 1;
        let next_line_start = self.line_to_char(next_line_idx);
        let next_line = self.line(next_line_idx);

        // calculate next line length excluding trailing newlines
        let mut next_len = next_line.len_chars();
        if next_len > 0 && next_line.char(next_len - 1) == '\n' {
            next_len -= 1;
        }

        // clamp the column to the next line's length and return new char index
        let target_col = std::cmp::min(pref_col, next_len);
        next_line_start + target_col
    }
}

impl Buffer {
    pub fn move_char(&mut self, dir: Direction, extend: bool) {
        self.apply_movement(sel_mode(extend), |buffer, sel| match dir {
            Direction::Left => {
                sel.head = buffer.prev_grapheme(sel.head);
                true
            }
            Direction::Right => {
                sel.head = buffer.next_grapheme(sel.head);
                true
            }
            Direction::Up => {
                sel.head = buffer.above_grapheme(sel.head, sel.pref_col);
                false
            }
            Direction::Down => {
                sel.head = buffer.below_grapheme(sel.head, sel.pref_col);
                false
            }
        });
    }
}

// TODO: let's add some proper grapheme tests with emojis and EOL behaviour, no need for just normal movement
