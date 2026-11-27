use std::ops::Range;

use crate::{
    Rope,
    edit::{ByteEdit, RopeEdit},
};

impl Rope {
    /// Edits the actual rope with the given data from a RopeEdit
    pub fn edit(&mut self, edit: &RopeEdit) {
        let old_char_len = edit.old_text.chars().count();
        if old_char_len > 0 {
            self.remove(edit.char_range.start..(edit.char_range.start + old_char_len));
        }
        if !edit.text.is_empty() {
            self.insert(edit.char_range.start, &edit.text);
        }
        self.changes.push(ByteEdit::from_rope_edit(edit, self));
    }

    /// Returns a possible RopeEdit that represents deleting a range
    pub fn delete_range(&mut self, char_range: Range<usize>) -> Option<RopeEdit> {
        if self.len_chars() == 0 || char_range.start > char_range.end {
            return None;
        }
        let old_text = self.slice(char_range.clone()).to_string();
        Some(RopeEdit {
            char_range,
            text: "".to_string(),
            old_text,
        })
    }

    /// Returns a possible RopeEdit that represents inserting a str
    pub fn insert_str(&mut self, text: &str, char_idx: usize) -> Option<RopeEdit> {
        let char_len = text.chars().count();
        if char_len == 0 {
            return None;
        }
        let char_range = char_idx..(char_idx + char_len);
        Some(RopeEdit {
            char_range,
            text: text.to_string(),
            old_text: "".to_string(),
        })
    }

    /// Returns a possible RopeEdit that represents inserting a char
    pub fn insert_char(&mut self, ch: char, char_idx: usize) -> Option<RopeEdit> {
        let mut buf = [0u8; 4];
        return self.insert_str(ch.encode_utf8(&mut buf), char_idx);
    }

    /// Returns a possible RopeEdit that represents replacing a range
    pub fn replace_range(&mut self, char_range: Range<usize>, text: &str) -> Option<RopeEdit> {
        if self.len_chars() == 0 || char_range.start > char_range.end {
            return None;
        }
        let old_text = self.slice(char_range.clone()).to_string();
        let char_len = text.chars().count();
        Some(RopeEdit {
            char_range: char_range.start..(char_range.start + char_len),
            text: text.to_string(),
            old_text,
        })
    }
}
