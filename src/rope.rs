use anyhow::{Context, Result};
use regex::Regex;
use ropey::Rope as Ropey;
use std::{
    io::{Read, Write},
    ops::Range,
    sync::LazyLock,
};

use std::ops::{Deref, DerefMut};

use crate::{history::BufferEdit, textobject::ObjectMode, tree::TextChange};

// shortcut: pair search gives up after this many chars per direction, raise it if huge blocks stop matching.
const PAIR_SEARCH_LIMIT: usize = 100_000;

static NEXT_ROPE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub struct Rope {
    rope: Ropey,
    /// Unique per Rope, so a consumer of `changes` can tell a wholesale replacement (reload, swap recovery) apart.
    pub id: u64,
    /// Every edit since the last drain, oldest first. Drained by the editor's syntax layer.
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

    fn wrap(rope: Ropey) -> Self {
        let id = NEXT_ROPE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            rope,
            id,
            changes: Vec::new(),
        }
    }

    fn byte_pos(&self, byte: usize) -> (usize, usize) {
        let line = self.byte_to_line(byte);
        (line, byte - self.line_to_byte(line))
    }

    pub fn save<W>(&self, mut writer: W) -> Result<()>
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
        let byte_start = self.char_to_byte(char_range.start);
        let byte_end = self.char_to_byte(char_range.end);
        let byte_range = byte_start..byte_end;
        let edit = BufferEdit {
            byte_range: byte_range.clone(),
            char_range,
            text: "".to_string(),
            old_text: self.byte_slice(byte_range).to_string(),
        };
        Some(edit)
    }

    pub fn insert_str(&mut self, text: &str, char_idx: usize) -> Option<BufferEdit> {
        let byte_len = text.len();
        let char_len = text.chars().count();

        if byte_len == 0 || char_len == 0 {
            return None;
        }

        let byte_idx = self.char_to_byte(char_idx);

        let byte_range = byte_idx..(byte_idx + byte_len);
        let char_range = char_idx..(char_idx + char_len);

        let edit = BufferEdit {
            byte_range: byte_range.clone(),
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

    pub fn char_to_col(&self, char_idx: usize) -> usize {
        // `char_idx == len_chars()` (cursor right after the last char) is a valid ropey index, not one to clamp away.
        let char_idx = char_idx.min(self.len_chars());
        let line_idx = self.char_to_line(char_idx);
        let line_start = self.line_to_char(line_idx);
        char_idx.saturating_sub(line_start)
    }

    pub fn col_to_char(&mut self, line_idx: usize, col: usize) -> usize {
        let total_lines = self.len_lines();
        if total_lines == 0 || line_idx >= total_lines {
            return self.len_chars().saturating_sub(1);
        }

        let line_start = self.line_to_char(line_idx);
        let line = self.line(line_idx);

        let mut line_len = line.len_chars();
        if line_len > 0 && line.char(line_len - 1) == '\n' {
            line_len -= 1;
        }

        line_start + col.min(line_len)
    }

    pub fn replace_range(&mut self, char_range: Range<usize>, text: &str) -> Option<BufferEdit> {
        let byte_start = self.char_to_byte(char_range.start);
        let byte_end = self.char_to_byte(char_range.end);
        let old_text = self.byte_slice(byte_start..byte_end).to_string();
        let char_len = text.chars().count();
        let edit = BufferEdit {
            byte_range: byte_start..(byte_start + text.len()),
            char_range: char_range.start..(char_range.start + char_len),
            text: text.to_string(),
            old_text,
        };
        Some(edit)
    }

    pub fn apply_edit(&mut self, edit: &BufferEdit) {
        let start_byte = self.char_to_byte(edit.char_range.start);
        let old_end_byte = start_byte + edit.old_text.len();
        let (start, old_end) = (self.byte_pos(start_byte), self.byte_pos(old_end_byte));
        let old_char_len = edit.old_text.chars().count();
        if old_char_len > 0 {
            self.remove(edit.char_range.start..(edit.char_range.start + old_char_len));
        }
        if !edit.text.is_empty() {
            self.insert(edit.char_range.start, &edit.text);
        }
        let new_end_byte = start_byte + edit.text.len();
        let new_end = self.byte_pos(new_end_byte);
        self.changes.push(TextChange {
            start_byte,
            old_end_byte,
            new_end_byte,
            start,
            old_end,
            new_end,
        });
    }

    pub fn find_char(&self, char_idx: usize, ch: char) -> usize {
        let len = self.len_chars();

        if char_idx >= len || len == 0 {
            return char_idx;
        }

        let slice = self.slice(char_idx..);

        if let Some(pos) = slice.chars().position(|c| c == ch) {
            pos
        } else {
            char_idx
        }
    }

    /// The pair object at `char_idx`, nvim rules: the innermost pair around
    /// the cursor, else the next one ahead. Same-char pairs (quotes,
    /// delimiters) are paired left to right along the cursor's line only.
    pub fn select_pair_object(
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

    /// Innermost `<tag>..</tag>` around `start..end`, else the next one ahead.
    /// `grow` skips a pair whose range already equals `start..end` (and never
    /// jumps ahead), so repeating it goes outward.
    pub fn select_tag_object(
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

    /// First `open` after `idx`, with its match.
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

    /// Pairs `q`s on `idx`'s line left to right (backslash-escaped ones
    /// skipped), then takes the pair around `idx` or else the next one.
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

    /// Kakoune's `m`/`<a-m>`: the enclosing range (both delimiters included)
    /// of the nearest `()`/`{}`/`[]`/`<>` pair at-or-`forward` (`m`) or
    /// at-or-backward (`<a-m>`) of `char_idx` - using the bracket under
    /// `char_idx` directly, same as vim's `%`, rather than searching past it,
    /// if it's already one.
    pub fn select_matching_pair(&self, char_idx: usize, forward: bool) -> Option<(usize, usize)> {
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

    pub fn line_is_blank(&self, line_idx: usize) -> bool {
        let line = self.rope.line(line_idx);
        line.chars().all(|c| c.is_whitespace())
    }

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

    pub fn select_whitespace_object(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
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

    pub fn select_number_object(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
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

    pub fn select_indent_object(
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

    pub fn select_paragraph_object(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
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

    pub fn select_sentence_object(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
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

    pub fn select_argument_object(&self, char_idx: usize, mode: ObjectMode) -> (usize, usize) {
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
}
