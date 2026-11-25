use std::ops::Range;

use crate::buffer::SelectionMode;

pub fn sel_mode(extend: bool) -> SelectionMode {
    if extend {
        SelectionMode::Extend
    } else {
        SelectionMode::Jump
    }
}

pub fn clamp_range(range: Range<usize>, len_chars: usize) -> Range<usize> {
    range.start.min(len_chars)..range.end.min(len_chars)
}
