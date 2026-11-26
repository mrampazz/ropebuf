use anyhow::{Context, Result};
use regex::Regex;
use ropey::Rope as Ropey;
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};
use std::{
    io::{Read, Write},
    ops::Range,
    sync::LazyLock,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::history::BufferEdit;
use crate::tree::TextChange;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum WordKind {
    Sub,
    Word,
    WORD,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectMode {
    Inner,
    Around,
}

// this is a shortcut
const PAIR_SEARCH_LIMIT: usize = 100_000;

static NEXT_ROPE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub struct Rope {
    rope: Ropey,
    pub id: u64,
    pub changes: Vec<TextChange>,
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
    pub fn from_str(content: &str) -> Self {
        Self::wrap(Ropey::from_str(content))
    }

    pub fn from_reader<R>(reader: R) -> Result<Self>
    where
        R: Read,
    {
        Ok(Self::wrap(Ropey::from_reader(reader)?))
    }

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

    pub fn delete_range(&mut self, char_range: Range<usize>) -> Option<BufferEdit> {
        if char_range.is_empty() {
            return None;
        }
        let old_text = self.slice(char_range.clone()).to_string();
        Some(BufferEdit {
            char_range,
            text: String::new(),
            old_text,
        })
    }

    pub fn insert_str(&mut self, text: &str, char_idx: usize) -> Option<BufferEdit> {
        let char_len = text.chars().count();
        if char_len == 0 {
            return None;
        }
        let char_range = char_idx..(char_idx + char_len);
        let edit = BufferEdit {
            char_range,
            text: text.to_string(),
            old_text: "".to_string(),
        };
        Some(edit)
    }

    pub fn insert_char(&mut self, ch: char, char_idx: usize) -> Option<BufferEdit> {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        let edit = self.insert_str(s, char_idx);
        return edit;
    }

    pub fn replace_range(&mut self, char_range: Range<usize>, text: &str) -> Option<BufferEdit> {
        if char_range.is_empty() && text.is_empty() {
            return None;
        }
        let char_len = text.chars().count();
        let old_text = self.slice(char_range.clone()).to_string();
        Some(BufferEdit {
            char_range: char_range.start..(char_range.start + char_len),
            text: text.to_string(),
            old_text,
        })
    }

    pub fn char_to_col(&self, char_idx: usize) -> usize {
        // `char_idx == len_chars()` (cursor right after the last char) is a valid ropey index, not one to clamp away.
        let char_idx = char_idx.min(self.len_chars());
        let line_idx = self.char_to_line(char_idx);
        let line_start = self.line_to_char(line_idx);
        char_idx.saturating_sub(line_start)
    }

    /// Applies a BufferEdit to the rope and pushes the TextChange with the changed bytes
    pub fn apply_edit(&mut self, edit: &BufferEdit) {
        let old_char_len = edit.old_text.chars().count();
        if old_char_len > 0 {
            self.remove(edit.char_range.start..(edit.char_range.start + old_char_len));
        }
        if !edit.text.is_empty() {
            self.insert(edit.char_range.start, &edit.text);
        }

        // get TextChange from this edit
        let textchange = self.edit_to_textchange(edit);
        self.changes.push(textchange);
    }

    /// Returns true if the line at the line_idx is blank
    pub fn line_is_blank(&self, line_idx: usize) -> bool {
        let line = self.rope.line(line_idx);
        line.chars().all(|c| c.is_whitespace())
    }

    /// Returns count of line indent for a given line_idx
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

    pub fn buffer_range(&self) -> Range<usize> {
        0..self.len_chars().saturating_sub(1)
    }

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

    pub fn pair_object_at(
        &self,
        char_idx: usize,
        open: char,
        close: char,
        mode: ObjectMode,
    ) -> Option<(usize, usize)> {
        if self.len_chars() == 0 {
            return None;
        }
        let idx = char_idx.min(self.len_chars() - 1);
        let (open_idx, close_idx) = if open == close {
            self.same_char_pair_on_line(idx, open)?
        } else {
            self.enclosing_pair(idx, open, close)
                .or_else(|| self.next_pair(idx, open, close))?
        };
        match mode {
            ObjectMode::Inner => Some((open_idx + 1, close_idx)),
            ObjectMode::Around => Some((open_idx, close_idx + 1)),
        }
    }

    pub fn tag_object_at(
        &self,
        start: usize,
        end: usize,
        grow: bool,
        mode: ObjectMode,
    ) -> Option<(usize, usize)> {
        static TAG: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"<(/?)([A-Za-z][\w:.-]*)[^<>]*?(/?)>").unwrap());
        let lo = start.saturating_sub(PAIR_SEARCH_LIMIT);
        let hi = (end + PAIR_SEARCH_LIMIT).min(self.len_chars());
        let slice = self.slice(lo..hi);
        let text = slice.to_string();

        // (open_start, open_end, close_start, close_end); unclosed tags like `<br>` are dropped.
        let mut stack: Vec<(&str, usize, usize)> = Vec::new();
        let mut pairs = Vec::new();
        for c in TAG.captures_iter(&text) {
            let m = c.get(0).unwrap();
            let s = lo + slice.byte_to_char(m.start());
            let e = lo + slice.byte_to_char(m.end());
            let name = c.get(2).unwrap().as_str();
            if &c[1] == "/" {
                if let Some(i) = stack.iter().rposition(|t| t.0 == name) {
                    pairs.push((stack[i].1, stack[i].2, s, e));
                    stack.truncate(i);
                }
            } else if &c[3] != "/" {
                stack.push((name, s, e));
            }
        }

        let range = |&(os, oe, cs, ce): &(usize, usize, usize, usize)| match mode {
            ObjectMode::Inner => (oe, cs),
            ObjectMode::Around => (os, ce),
        };
        let around = pairs
            .iter()
            .filter(|p| p.0 <= start && end <= p.3 && !(grow && range(p) == (start, end)))
            .max_by_key(|p| p.0);
        let ahead = || pairs.iter().filter(|p| p.0 > start).min_by_key(|p| p.0);
        around
            .or_else(|| if grow { None } else { ahead() })
            .map(range)
    }

    pub fn matching_pair_at(&self, char_idx: usize, forward: bool) -> Option<(usize, usize)> {
        const PAIRS: [(char, char); 4] = [('(', ')'), ('{', '}'), ('[', ']'), ('<', '>')];
        let is_bracket = |c: char| PAIRS.iter().any(|&(o, cl)| c == o || c == cl);

        let len = self.len_chars();
        if len == 0 {
            return None;
        }
        let start = char_idx.min(len - 1);
        let bracket_idx = if is_bracket(self.char(start)) {
            start
        } else if forward {
            (start + 1..len).find(|&i| is_bracket(self.char(i)))?
        } else {
            (0..start).rev().find(|&i| is_bracket(self.char(i)))?
        };

        let ch = self.char(bracket_idx);
        let &(open, close) = PAIRS.iter().find(|&&(o, c)| ch == o || ch == c)?;

        if ch == open {
            let mut depth = 0;
            for idx in (bracket_idx + 1)..len {
                let c = self.char(idx);
                if c == open {
                    depth += 1;
                } else if c == close {
                    if depth == 0 {
                        return Some((bracket_idx, idx + 1));
                    }
                    depth -= 1;
                }
            }
            None
        } else {
            let mut depth = 0;
            for idx in (0..bracket_idx).rev() {
                let c = self.char(idx);
                if c == close {
                    depth += 1;
                } else if c == open {
                    if depth == 0 {
                        return Some((idx, bracket_idx + 1));
                    }
                    depth -= 1;
                }
            }
            None
        }
    }

    pub fn whitespace_object_at(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
        }
        let head = char_idx.min(len - 1);
        if !self.char(head).is_whitespace() {
            return (head, head);
        }

        let mut start = head;
        let mut end = head + 1;
        while start > 0 && self.char(start - 1).is_whitespace() {
            if mode == ObjectMode::Inner && self.char(start - 1) == '\n' {
                break;
            }
            start -= 1;
        }
        while end < len && self.char(end).is_whitespace() {
            if mode == ObjectMode::Inner && self.char(end) == '\n' {
                break;
            }
            end += 1;
        }
        (start, end)
    }

    pub fn number_object_at(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
        }
        let head = char_idx.min(len - 1);
        if !self.char(head).is_numeric() {
            return (head, head);
        }

        let mut start = head;
        let mut end = head + 1;
        while start > 0 && (self.char(start - 1).is_numeric() || self.char(start - 1) == '.') {
            start -= 1;
        }
        while end < len && (self.char(end).is_numeric() || self.char(end) == '.') {
            end += 1;
        }
        if mode == ObjectMode::Around {
            while end < len && self.char(end).is_whitespace() && self.char(end) != '\n' {
                end += 1;
            }
        }
        (start, end)
    }

    pub fn indent_object_at(
        &self,
        char_idx: usize,
        mode: ObjectMode,
        tab_width: usize,
    ) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
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
        (start, end)
    }

    pub fn paragraph_object_at(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
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
        (start, end)
    }

    pub fn sentence_object_at(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
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
        (start, end)
    }

    pub fn argument_object_at(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
        }
        let head = char_idx.min(len - 1);
        let mut start = head;
        let mut end = head;

        while start > 0 {
            let ch = self.char(start - 1);
            if ch == ',' || ch == '(' || ch == '[' || ch == '{' {
                break;
            }
            start -= 1;
        }

        while end < len {
            let ch = self.char(end);
            if ch == ',' || ch == ')' || ch == ']' || ch == '}' {
                break;
            }
            end += 1;
        }

        if mode == ObjectMode::Inner {
            while start < end && self.char(start).is_whitespace() {
                start += 1;
            }
            while end > start && self.char(end - 1).is_whitespace() {
                end -= 1;
            }
        } else if mode == ObjectMode::Around && end < len && self.char(end) == ',' {
            end += 1;
        }
        (start, end)
    }

    pub fn current_word(&self, char_idx: usize, kind: WordKind) -> Range<usize> {
        let len = self.len_chars();

        if char_idx >= len || len == 0 {
            return char_idx..char_idx;
        }

        let mut start = char_idx;
        while start > 0 {
            let prev = self.prev_grapheme(start);
            let prev_char = self.char(prev);
            let current_char = self.char(start);
            if is_boundary(prev_char, current_char, kind) {
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

            if is_boundary(current_char, next_char, kind) {
                end = next_idx;
                break;
            }
            end = next_idx;
        }

        start..end
    }

    pub fn word_object_at(
        &self,
        char_idx: usize,
        kind: WordKind,
        mode: ObjectMode,
    ) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
        }

        let word_range = self.current_word(char_idx, kind);

        match mode {
            ObjectMode::Inner => (word_range.start, word_range.end),
            ObjectMode::Around => {
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

                (start, end)
            }
        }
    }

    pub fn next_word(&self, char_idx: usize, kind: WordKind) -> Range<usize> {
        let len = self.len_chars();
        if char_idx >= len {
            return len..len;
        }

        let current = self.current_word(char_idx, kind);

        let mut pos = if char_idx < current.end {
            current.end
        } else {
            char_idx
        };

        while pos < len && self.char(pos).is_whitespace() {
            pos = self.next_grapheme(pos);
        }

        if pos >= len {
            return len..len;
        }

        self.current_word(pos, kind)
    }

    pub fn prev_word(&self, char_idx: usize, kind: WordKind) -> Range<usize> {
        if char_idx == 0 {
            return 0..0;
        }

        let current = self.current_word(char_idx, kind);

        let mut pos = if char_idx >= current.start && current.start > 0 {
            self.prev_grapheme(current.start)
        } else {
            char_idx
        };

        while pos > 0 && self.char(pos).is_whitespace() {
            pos = self.prev_grapheme(pos);
        }

        if pos == 0 && self.char(0).is_whitespace() {
            return 0..0;
        }

        self.current_word(pos, kind)
    }

    pub fn next_word_start_range(&self, char_idx: usize, kind: WordKind) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (char_idx, char_idx);
        }
        let at = char_idx.min(len - 1);
        let current = self.current_word(char_idx, kind);
        let nothing_left_here =
            self.char(at).is_whitespace() || char_idx >= current.end.saturating_sub(1);

        if nothing_left_here {
            let next = self.next_word(char_idx, kind);
            if next.start >= next.end {
                return (char_idx, char_idx);
            }
            return (next.start, self.skip_line_whitespace_forward(next.end, len));
        }

        let head = self.skip_line_whitespace_forward(current.end, len);
        (char_idx, head.max(char_idx))
    }

    pub fn prev_word_start_range(&self, char_idx: usize, kind: WordKind) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (char_idx, char_idx);
        }
        let current = self.current_word(char_idx, kind);
        let nothing_left_here =
            self.char(char_idx.min(len - 1)).is_whitespace() || char_idx <= current.start;

        if nothing_left_here {
            let prev = self.prev_word(char_idx, kind);
            if prev.start >= prev.end {
                return (char_idx, char_idx); // nothing before - start of buffer
            }
            return (
                self.skip_line_whitespace_forward(prev.end, current.start),
                prev.start,
            );
        }

        let head = current.start;
        (char_idx.max(head), head)
    }

    pub fn next_word_end_range(&self, char_idx: usize, kind: WordKind) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (char_idx, char_idx);
        }
        let at = char_idx.min(len - 1);
        let current = self.current_word(char_idx, kind);
        let nothing_left_here =
            self.char(at).is_whitespace() || char_idx >= current.end.saturating_sub(1);

        if nothing_left_here {
            let next = self.next_word(char_idx, kind);
            if next.start >= next.end {
                return (char_idx, char_idx); // nothing after - end of buffer
            }
            return (
                self.skip_line_whitespace_backward(next.start, current.end),
                next.end.saturating_sub(1),
            );
        }

        let head = current.end.saturating_sub(1);
        (char_idx.min(head), head)
    }

    /// returns the line start char
    pub fn line_start(&self, char_idx: usize) -> usize {
        let line_idx = self.char_to_line(char_idx.min(self.len_chars().saturating_sub(1)));
        self.line_to_char(line_idx)
    }

    /// returns the line end char
    pub fn line_end(&self, char_idx: usize) -> usize {
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
    pub fn first_non_whitespace_of_line(&self, char_idx: usize) -> usize {
        let start = self.line_start(char_idx);
        let end = self.line_end(char_idx);
        let mut pos = start;
        while pos < end {
            if !self.char(pos).is_whitespace() {
                return pos;
            }
            pos = self.next_grapheme(pos);
        }
        start
    }

    /// returns the forward char index of the nth occurrence of the target char
    pub fn find_char_forward(&self, char_idx: usize, target: char, count: usize) -> Option<usize> {
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
    pub fn find_char_backward(&self, char_idx: usize, target: char, count: usize) -> Option<usize> {
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

    pub fn extend_line_selection(&self, anchor: usize, head: usize) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
        }

        let start_idx = anchor.min(head).min(len - 1);
        let end_idx = anchor.max(head).min(len - 1);

        let start_line = self.char_to_line(start_idx);
        let end_line = self.char_to_line(end_idx);

        let (line_start, line_end) = self.line_bounds_inclusive(start_line, end_line);
        let is_full_line_selection = anchor.min(head) == line_start && anchor.max(head) == line_end;

        if is_full_line_selection && end_line + 1 < self.len_lines() {
            self.line_bounds_inclusive(start_line, end_line + 1)
        } else {
            (line_start, line_end)
        }
    }
}

/// private API
impl Rope {
    fn wrap(rope: Ropey) -> Self {
        let id = NEXT_ROPE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            rope,
            id,
            changes: Vec::new(),
        }
    }

    /// Returns the position (line, byte_idx) given an absolute byte_idx (needed for generating TextChange from edit)
    fn byte_pos(&self, byte: usize) -> (usize, usize) {
        let line = self.byte_to_line(byte);
        (line, byte - self.line_to_byte(line))
    }

    /// Returns a TextChange for tree sitter and lsp from a BufferEdit
    fn edit_to_textchange(&mut self, edit: &BufferEdit) -> TextChange {
        let start_byte = self.char_to_byte(edit.char_range.start);
        let old_end_byte = start_byte + edit.old_text.len();
        let (start, old_end) = (self.byte_pos(start_byte), self.byte_pos(old_end_byte));
        let new_end_byte = start_byte + edit.text.len();
        let new_end = self.byte_pos(new_end_byte);
        TextChange {
            start_byte,
            old_end_byte,
            new_end_byte,
            start,
            old_end,
            new_end,
        }
    }

    fn same_char_pair_on_line(&self, idx: usize, q: char) -> Option<(usize, usize)> {
        let line = self.char_to_line(idx);
        let line_start = self.line_to_char(line);
        let mut quotes = Vec::new();
        let mut escaped = false;
        for (i, ch) in self.line(line).chars().enumerate() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quotes.push(line_start + i);
            }
        }
        quotes
            .chunks_exact(2)
            .map(|p| (p[0], p[1]))
            .find(|&(_, close_idx)| close_idx >= idx)
    }

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

    fn skip_line_whitespace_forward(&self, start: usize, bound: usize) -> usize {
        let mut p = start;
        while p < bound && is_line_whitespace(self.char(p)) {
            p = self.next_grapheme(p);
        }
        p.saturating_sub(1)
    }

    fn skip_line_whitespace_backward(&self, start: usize, bound: usize) -> usize {
        let mut p = start;
        while p > bound && is_line_whitespace(self.char(p - 1)) {
            p = self.prev_grapheme(p);
        }
        p
    }

    fn line_bounds_inclusive(&self, start_line: usize, end_line: usize) -> (usize, usize) {
        let start = self.line_to_char(start_line);
        let end = if end_line + 1 < self.len_lines() {
            self.line_to_char(end_line + 1).saturating_sub(1)
        } else {
            self.len_chars().saturating_sub(1)
        };
        (start, end.max(start))
    }
}

pub fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub fn is_big_word(ch: char) -> bool {
    !ch.is_whitespace()
}

fn is_line_whitespace(ch: char) -> bool {
    ch.is_whitespace() && ch != '\n'
}

// returns true if the two chars are a boundary for the given WordKind
pub fn is_boundary(left: char, right: char, kind: WordKind) -> bool {
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
            if is_boundary(left, right, WordKind::Word) {
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
    fn word_range() {
        let r = Rope::from_str("pollo");
        assert_eq!(r.current_word(0, WordKind::Word), 0..5);
    }

    #[test]
    fn adjacent_word() {
        let r = Rope::from_str("ciao come stai");
        assert_eq!(r.next_word(0, WordKind::Word), 5..9);
        assert_eq!(r.prev_word(4, WordKind::Word), 0..4);
        assert_eq!(r.next_word(4, WordKind::Word), 5..9);
        assert_eq!(r.prev_word(9, WordKind::Word), 5..9);
    }

    #[test]
    fn big_word() {
        let r = Rope::from_str("word.exe");
        assert_eq!(r.current_word(0, WordKind::WORD), 0..8);
    }

    #[test]
    fn sub_word() {
        let r = Rope::from_str("camelCase snake_case");
        assert_eq!(r.next_word(0, WordKind::Sub), 5..9);
        assert_eq!(r.next_word(6, WordKind::Sub), 10..15);
    }

    #[test]
    fn first_non_whitespace_of_line_spaces() {
        let r = Rope::from_str("    lmfao");
        assert_eq!(r.first_non_whitespace_of_line(0), 4);
    }

    #[test]
    fn first_non_whitespace_of_line_empty() {
        let r = Rope::from_str("       ");
        assert_eq!(r.first_non_whitespace_of_line(0), 0);
    }
}
