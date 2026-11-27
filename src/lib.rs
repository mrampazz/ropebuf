use anyhow::{Context, Result};
use ropey::Rope as Ropey;

use std::{
    io::{Read, Write},
    ops::{Deref, DerefMut, Range},
    sync::atomic::AtomicU64,
};

use crate::edit::ByteEdit;

pub mod edit;
pub mod find;
pub mod grapheme;
pub mod line;
pub mod ops;
pub mod pair;
pub mod textobject;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum ObjectMode {
    Inner,
    Around,
}

static NEXT_ROPE_ID: AtomicU64 = AtomicU64::new(0);

pub struct Rope {
    rope: Ropey,
    pub id: u64,
    pub changes: Vec<ByteEdit>,
}

impl Deref for Rope {
    type Target = Ropey;

    fn deref(&self) -> &Self::Target {
        &self.rope
    }
}

impl DerefMut for Rope {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rope
    }
}

impl Rope {
    /// Wraps the rope with an id
    fn with_id(rope: Ropey) -> Self {
        let id = NEXT_ROPE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            rope,
            id,
            changes: vec![],
        }
    }

    /// Returns a rope object given a str
    pub fn from_str(content: &str) -> Self {
        Self::with_id(Ropey::from_str(content))
    }

    /// Returns a rope object given a Reader
    pub fn from_reader<R>(reader: R) -> Result<Self>
    where
        R: Read,
    {
        Ok(Self::with_id(Ropey::from_reader(reader)?))
    }

    /// Writes the content of the rope using a writer
    pub fn write<W>(&self, mut writer: W) -> Result<()>
    where
        W: Write,
    {
        for chunk in self.chunks() {
            writer
                .write_all(chunk.as_bytes())
                .context("Failed to write buffer content to disk")?;
        }
        writer.flush().context("Failed to flush file writer")?;
        Ok(())
    }

    /// Returns a byte position (row, byte column) given a byte_idx
    fn byte_pos(&self, byte_idx: usize) -> (usize, usize) {
        let line_idx = self.byte_to_line(byte_idx);
        (line_idx, byte_idx - self.line_to_byte(line_idx))
    }

    /// Returns the total rope range in chars
    pub fn char_range(&self) -> Range<usize> {
        0..self.len_chars().saturating_sub(1)
    }
}
