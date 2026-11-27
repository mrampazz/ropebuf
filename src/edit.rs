use std::ops::Range;

use crate::Rope;

/// Struct that defines what changed in the rope using bytes pos
/// (row, byte column)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start: (usize, usize),
    pub old_end: (usize, usize),
    pub new_end: (usize, usize),
}

impl ByteEdit {
    pub fn from_rope_edit(edit: &RopeEdit, rope: &Rope) -> Self {
        let start_byte = rope.char_to_byte(edit.char_range.start);
        let old_end_byte = start_byte + edit.old_text.len();
        let (start, old_end) = (rope.byte_pos(start_byte), rope.byte_pos(old_end_byte));
        let new_end_byte = start_byte + edit.text.len();
        let new_end = rope.byte_pos(new_end_byte);
        Self {
            start_byte,
            old_end_byte,
            new_end_byte,
            start,
            old_end,
            new_end,
        }
    }
}

/// Struct that defines an edit done on the rope used to handle undo & redo
#[derive(Clone, Debug)]
pub struct RopeEdit {
    pub char_range: Range<usize>,
    pub text: String,
    pub old_text: String,
}

impl RopeEdit {
    pub fn inverse(&self) -> Self {
        let char_len = self.text.chars().count();
        let inverted_char_range = self.char_range.start..(self.char_range.start + char_len);
        RopeEdit {
            char_range: inverted_char_range,
            text: self.old_text.clone(),
            old_text: self.text.clone(),
        }
    }
}
