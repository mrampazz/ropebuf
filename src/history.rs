use std::{
    ops::Range,
    time::{Duration, Instant},
};

use crate::selection::Selection;

#[derive(Clone, Debug)]
pub struct BufferEdit {
    pub byte_range: Range<usize>,
    pub char_range: Range<usize>,
    pub text: String,
    pub old_text: String,
}

impl BufferEdit {
    pub fn inverse(&self) -> Self {
        let new_len = self.text.len();
        let char_len = self.text.chars().count();
        let inverted_range = self.byte_range.start..(self.byte_range.start + new_len);
        let inverted_char_range = self.char_range.start..(self.char_range.start + char_len);
        BufferEdit {
            byte_range: inverted_range,
            char_range: inverted_char_range,
            text: self.old_text.clone(),
            old_text: self.text.clone(),
        }
    }
}

// TODO: we need proper testing for every action then undo / redo
#[derive(Debug, Clone)]
pub struct Transaction {
    pub edits: Vec<BufferEdit>,
    pub time: Instant,
    pub selections_before: Vec<Selection>,
    pub selections_after: Vec<Selection>,
}

impl Transaction {
    pub fn coalesce(&mut self, tx: &Transaction) -> bool {
        if tx.time.duration_since(self.time) <= Duration::from_millis(500) {
            self.edits.extend(tx.edits.clone());
            self.time = tx.time;
            self.selections_after = tx.selections_after.clone();
            return true;
        }
        false
    }
}

#[derive(Default, Debug)]
pub struct History {
    pub undo_stack: Vec<Transaction>,
    pub redo_stack: Vec<Transaction>,
}

impl History {
    pub fn push(&mut self, tx: Transaction) {
        self.redo_stack.clear();
        self.undo_stack.push(tx);
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}
