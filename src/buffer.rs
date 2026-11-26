use anyhow::{Context, Error, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufReader, BufWriter},
    ops::Range,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use crate::{
    history::{BufferEdit, History, Transaction},
    line_ending::{LineEnding, LineEndingWriter, LineNormaliser, detect_line_ending},
    register::RegisterManager,
    rope::{ObjectMode, Rope, WordKind},
    selection::Selection,
    utils::{clamp_range, is_word, sel_mode},
};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum BufferMovement {
    Home,
    End,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindKind {
    To,
    Until,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindDirection {
    Forward,
    Backward,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharSearch {
    pub kind: FindKind,
    pub direction: FindDirection,
    pub target: char,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum LineMovement {
    LineHome,
    LineEnd,
    LineHomeNoWhitespace,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum WordMovement {
    NextStart,
    NextEnd,
    PrevStart,
}

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum SelectionMode {
    Jump,        // Move cursor, reset selection width to 1
    Extend,      // Grow selection from anchor to new head
    SetExplicit, // Keep the exact anchor and head values set inside the closure
}

pub struct Buffer {
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub selections: Vec<Selection>,
    pub history: History,
    pub original_line_ending: LineEnding,
    pub dirty: bool,
    /// The file's mtime as of the last read/write - compared against the
    /// file's current mtime (lazily, on buffer entry, no filesystem watch)
    /// to detect an external edit. `None` for a path-less scratch buffer.
    pub disk_mtime: Option<SystemTime>,
    pub last_char_search: Option<CharSearch>,
    pub insert_before: bool,
    /// Synced from the `indentwidth` option before each command dispatch - see `editor`'s `Editor::dispatch_buffer_command`.
    pub indent_width: usize,
    /// Synced from the `autoindent` option the same way as `indent_width`.
    pub autoindent: bool,
    /// Synced from the `autopair` option the same way as `indent_width`.
    pub autopair: bool,
    /// Live `/` search session, see `search.rs`.
    pub search: Option<crate::search::SearchSession>,
    /// A special buffer's name (`*grep*`): one with no file, made by a script.
    pub name: Option<String>,
    /// Editing commands leave the text alone (`refused` says one tried); scripts still
    /// write with `replace_range`.
    pub read_only: bool,
    pub refused: bool,
    /// Edits are recorded for undo; off for text nobody undoes (a log, process output). Set with
    /// `set_keeps_undo`, which drops the history when it turns off.
    pub keeps_undo: bool,
    /// Edits close together merge into one undo step - set while inserting (Kakoune: one step
    /// per insert session, each normal-mode edit its own).
    pub merge_edits: bool,
    /// The chars changed since the last `take_changed`, as one range of the text now.
    changed: Option<Range<usize>>,
    /// A script's marks on the text (`overlay.rs`), kept in step with every edit.
    pub overlays: Vec<crate::overlay::Overlay>,
}

impl Buffer {
    pub fn from_str(content: &str) -> Self {
        Self {
            rope: Rope::from_str(content),
            path: None,
            selections: vec![Selection::default()],
            history: History::default(),
            original_line_ending: LineEnding::LF,
            dirty: true,
            disk_mtime: None,
            last_char_search: None,
            insert_before: false,
            indent_width: 4,
            autoindent: true,
            autopair: false,
            search: None,
            name: None,
            read_only: false,
            refused: false,
            keeps_undo: true,
            merge_edits: false,
            changed: None,
            overlays: Vec::new(),
        }
    }

    pub fn from_path(path: PathBuf) -> Result<Self> {
        let mut file = File::open(path.clone())?;
        let disk_mtime = file.metadata().and_then(|m| m.modified()).ok();
        let original_line_ending = detect_line_ending(&mut file);
        let reader = BufReader::new(file);
        let normalizer = LineNormaliser::new(reader, original_line_ending);
        let rope = Rope::from_reader(normalizer)?;
        Ok(Self {
            rope,
            path: Some(path),
            selections: vec![Selection::default()],
            history: History::default(),
            original_line_ending,
            dirty: false,
            disk_mtime,
            last_char_search: None,
            insert_before: false,
            indent_width: 4,
            autoindent: true,
            autopair: false,
            search: None,
            name: None,
            read_only: false,
            refused: false,
            keeps_undo: true,
            merge_edits: false,
            changed: None,
            overlays: Vec::new(),
        })
    }

    pub fn save_as(&mut self, new_path: PathBuf) -> Result<()> {
        self.path = Some(new_path);
        self.dirty = true;
        self.save()
    }

    pub fn save(&mut self) -> Result<()> {
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| Error::msg("No file path specified. Use save_as."))?;

        let file =
            File::create(path).with_context(|| format!("Failed to create file at {:?}", path))?;

        let buf_writer = BufWriter::new(file);
        let writer = LineEndingWriter::new(buf_writer, self.original_line_ending);
        self.rope.write(writer)?;
        self.dirty = false;
        self.disk_mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        Ok(())
    }

    /// Whether the file backing this buffer has been modified on disk since
    /// it was opened or last saved - checked lazily (see `disk_mtime`)
    pub fn changed_on_disk(&self) -> bool {
        let (Some(path), Some(recorded)) = (&self.path, self.disk_mtime) else {
            return false;
        };
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|current| current != recorded)
    }

    pub fn filename(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("scratch")
            .to_string()
    }

    pub fn extension(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|os_str| os_str.to_str())
            .unwrap_or("")
            .to_string()
    }

    pub fn path(&self) -> &Option<PathBuf> {
        &self.path
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn apply_tx(&mut self, tx: &Transaction) {
        let mut sorted_edits = tx.edits.clone();
        sorted_edits.sort_by(|a, b| b.char_range.start.cmp(&a.char_range.start));

        for edit in sorted_edits {
            self.rope.apply_edit(&edit);
            self.overlays.iter_mut().for_each(|o| o.follow(&edit));
        }
        self.note_changed(&tx.edits, false);

        self.selections = tx.selections_after.clone();
    }

    /// Remembers what `edits` changed, as a range of the text after them (`undone`: before them).
    fn note_changed(&mut self, edits: &[BufferEdit], undone: bool) {
        for edit in edits {
            let len = if undone { &edit.old_text } else { &edit.text }
                .chars()
                .count();
            let range = edit.char_range.start..edit.char_range.start + len;
            self.changed = Some(match self.changed.take() {
                Some(r) => r.start.min(range.start)..r.end.max(range.end),
                None => range,
            });
        }
    }

    /// What changed since the last call (`None`: nothing), for the `TextChanged` hook.
    pub fn take_changed(&mut self) -> Option<Range<usize>> {
        self.changed.take()
    }

    /// Replaces chars `range` with `text` as one undoable change, read-only or not - how a
    /// script writes. The selections shift with the text; one inside `range` lands at its start.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) {
        let len = self.rope.len_chars();
        let range = range.start.min(len)..range.end.clamp(range.start.min(len), len);
        let mut edits = Vec::new();
        if !range.is_empty() {
            edits.extend(self.rope.delete_range(range.clone()));
        }
        edits.extend(self.rope.insert_str(text, range.start));
        if edits.is_empty() {
            return;
        }
        let delta = text.chars().count() as isize - range.len() as isize;
        let shift = |p: usize| {
            if p < range.start {
                p
            } else if p >= range.end && !range.is_empty() || p > range.start && range.is_empty() {
                (p as isize + delta) as usize
            } else {
                range.start
            }
        };
        let selections_before = self.selections.clone();
        let selections_after = selections_before
            .iter()
            .map(|s| Selection {
                anchor: shift(s.anchor),
                head: shift(s.head),
                pref_col: s.pref_col,
            })
            .collect();
        let tx = Transaction {
            edits,
            selections_before,
            selections_after,
            time: Instant::now(),
        };
        self.apply_tx(&tx);
        if self.keeps_undo {
            self.history.push(tx);
        }
        // A special buffer is a script's view, never "unsaved".
        self.dirty |= self.name.is_none();
    }

    /// Turns undo recording on or off; off also forgets what could be undone.
    pub fn set_keeps_undo(&mut self, on: bool) {
        self.keeps_undo = on;
        if !on {
            self.history = History::default();
        }
    }

    pub fn merge_selections(&mut self) {
        self.merge_within(0);
    }

    /// Kakoune's `<a-_>`: like `merge_selections`, but also merges
    /// selections that are merely adjacent (no character shared) rather than
    /// genuinely overlapping.
    pub fn merge_contiguous_selections(&mut self) {
        self.merge_within(1);
    }

    fn merge_within(&mut self, tolerance: usize) {
        if self.selections.len() <= 1 {
            return;
        }

        let main_start = self.selections[0].start();
        self.selections.sort_by_key(|s| s.start());

        let mut merged: Vec<Selection> = Vec::with_capacity(self.selections.len());
        let mut current = self.selections[0];

        for next in self.selections.iter().skip(1) {
            // `tolerance` of 0 merges overlapping/touching-boundary selections only; 1 also merges merely-adjacent ones.
            if next.start() <= current.end() + tolerance {
                let new_start = current.start();
                let new_end = current.end().max(next.end());

                // Pick the orientation and pref_col from whichever selection extended further
                let (head, anchor, pref_col) = if next.end() >= current.end() {
                    (next.head, next.anchor, next.pref_col)
                } else {
                    (current.head, current.anchor, current.pref_col)
                };

                // Apply the new bounds and direction
                if head < anchor {
                    current.head = new_start;
                    current.anchor = new_end;
                } else {
                    current.anchor = new_start;
                    current.head = new_end;
                }

                current.pref_col = pref_col;
            } else {
                merged.push(current);
                current = *next;
            }
        }
        merged.push(current);
        // Sorting made the leftmost selection main; rotate so the old main (or what it merged into) stays first.
        if let Some(i) = merged
            .iter()
            .position(|s| s.start() <= main_start && main_start <= s.end())
        {
            merged.rotate_left(i);
        }
        self.selections = merged;
    }

    pub fn apply_edit_to_selections<F>(&mut self, mut edit_fn: F)
    where
        F: FnMut(&mut Rope, &mut Selection) -> Option<BufferEdit>,
    {
        if self.read_only {
            self.refused = true;
            return;
        }
        let selections_before = self.selections.clone();
        let mut edits = Vec::new();

        // Sort indices right-to-left to prevent index shifting
        let mut sorted_indices: Vec<usize> = (0..self.selections.len()).collect();
        sorted_indices.sort_by_key(|&i| self.selections[i].start());

        // Already-processed (rightward) selections' stored positions go stale
        // once a later (leftward) edit changes the text length before them -
        // shift them by that edit's delta to keep them pointing at the same chars.
        let mut already_edited: Vec<usize> = Vec::new();
        for &i in sorted_indices.iter().rev() {
            let sel = &mut self.selections[i];
            if let Some(edit) = edit_fn(&mut self.rope, sel) {
                let delta =
                    edit.text.chars().count() as isize - edit.old_text.chars().count() as isize;
                if delta != 0 {
                    for &j in &already_edited {
                        let s = &mut self.selections[j];
                        s.anchor = (s.anchor as isize + delta).max(0) as usize;
                        s.head = (s.head as isize + delta).max(0) as usize;
                    }
                }
                edits.push(edit);
            }
            already_edited.push(i);
        }

        self.merge_selections();

        let tx = Transaction {
            edits,
            selections_before,
            selections_after: self.selections.clone(),
            time: Instant::now(),
        };

        // apply the transaction to the rope
        self.apply_tx(&tx);

        let coalesced = self.merge_edits
            && self
                .history
                .undo_stack
                .last_mut()
                .is_some_and(|last| last.coalesce(&tx));
        if self.keeps_undo && !coalesced {
            self.history.push(tx.clone());
        }
        self.dirty = true;
    }

    pub fn reduce_to_single_primary_cursor(&mut self) {
        if let Some(primary) = self.selections.first().cloned() {
            let target = primary.head;
            let pref_col = self.rope.char_to_col(target);
            self.selections = vec![Selection {
                anchor: target,
                head: target,
                pref_col,
            }];
        }
    }

    /// Kakoune's `,`: drops every selection but the main one (the first,
    /// same convention as `reduce_to_single_primary_cursor`).
    pub fn keep_main_selection(&mut self) {
        self.selections.truncate(1);
    }

    /// Kakoune's `<a-,>`: drops just the main selection, keeping the rest -
    /// a no-op if it's the only one left.
    pub fn clear_main_selection(&mut self) {
        if self.selections.len() > 1 {
            self.selections.remove(0);
        }
    }

    /// Kakoune's `)`/`(`: the main selection (index 0, this codebase's
    /// convention) becomes the next/previous one in position order.
    pub fn rotate_main_selection(&mut self, backward: bool) {
        if self.selections.len() <= 1 {
            return;
        }
        if backward {
            self.selections.rotate_right(1);
        } else {
            self.selections.rotate_left(1);
        }
    }

    /// Kakoune's `<+>`: duplicates each selection in place, deliberately
    /// generating overlapping selections rather than auto-merging them away
    /// - that's what `<a-+>`/`merge_overlapping_selections` is for.
    pub fn duplicate_selections(&mut self) {
        let mut doubled = Vec::with_capacity(self.selections.len() * 2);
        for sel in &self.selections {
            doubled.push(*sel);
            doubled.push(*sel);
        }
        self.selections = doubled;
    }

    /// Kakoune's `C`/`<a-C>`: adds, for each selection, a duplicate on the
    /// line below/above it at the same columns (clamped to that line's
    /// length) - skipped for any selection already on the last/first line.
    pub fn duplicate_selection_lines(&mut self, below: bool) {
        let total_lines = self.rope.len_lines();
        let mut added = Vec::new();
        for sel in &self.selections {
            let line = self.rope.char_to_line(sel.head);
            if below && line + 1 >= total_lines || !below && line == 0 {
                continue;
            }
            let anchor_col = self.rope.char_to_col(sel.anchor);
            let head_col = self.rope.char_to_col(sel.head);
            let (anchor, head) = if below {
                (
                    self.rope.below_grapheme(sel.anchor, anchor_col),
                    self.rope.below_grapheme(sel.head, head_col),
                )
            } else {
                (
                    self.rope.above_grapheme(sel.anchor, anchor_col),
                    self.rope.above_grapheme(sel.head, head_col),
                )
            };
            added.push(Selection {
                anchor,
                head,
                pref_col: head_col,
            });
        }
        self.selections.extend(added);
        self.merge_selections();
    }

    pub fn flip_selections(&mut self) {
        for sel in &mut self.selections {
            let new_head = sel.anchor;
            sel.anchor = sel.head;
            sel.head = new_head;
            sel.pref_col = self.rope.char_to_col(sel.head)
        }
    }

    /// Kakoune's `<a-:>`: flips any selection whose head sits before its
    /// anchor, so the cursor is always after the anchor.
    pub fn ensure_forward_selections(&mut self) {
        for sel in &mut self.selections {
            if sel.head < sel.anchor {
                std::mem::swap(&mut sel.anchor, &mut sel.head);
            }
        }
    }

    pub fn collapse_selections(&mut self) {
        for sel in &mut self.selections {
            sel.collapse_to_head();
            sel.pref_col = self.rope.char_to_col(sel.head);
        }
    }

    /// Where insert mode types at the primary selection.
    pub fn insert_point(&self) -> usize {
        let sel = self.selections.first().copied().unwrap_or_default();
        if self.insert_before {
            sel.start()
        } else {
            (sel.end() + 1).min(self.rope.len_chars())
        }
    }

    /// One cursor at char `at` (clamped), or - `extend` - the main selection's head moved there,
    /// its anchor kept (a mouse click, a drag).
    pub fn set_point(&mut self, at: usize, extend: bool) {
        // Past the last char is a cursor too: where typing at the end lands.
        let at = at.min(self.rope.len_chars());
        let pref_col = self.rope.char_to_col(at);
        let anchor = match self.selections.first() {
            Some(main) if extend => main.anchor,
            _ => at,
        };
        self.selections = vec![Selection {
            anchor,
            head: at,
            pref_col,
        }];
    }

    pub fn enter_insert_before(&mut self) {
        self.insert_before = true;
    }

    pub fn enter_insert_after(&mut self) {
        self.insert_before = false;
    }

    /// Opens a new empty line right after each selection's line and moves
    /// that selection onto it (Kakoune's `o`/`<a-o>`).
    pub fn open_line_below(&mut self) {
        self.apply_edit_to_selections(|rope, sel| {
            let at = rope.line_end(sel.head);
            let edit = rope.insert_str("\n", at)?;
            sel.anchor = edit.char_range.end;
            sel.head = edit.char_range.end;
            sel.pref_col = 0;
            Some(edit)
        });
    }

    /// Opens a new empty line right before each selection's line and moves
    /// that selection onto it (Kakoune's `O`/`<a-O>`).
    pub fn open_line_above(&mut self) {
        self.apply_edit_to_selections(|rope, sel| {
            let at = rope.line_start(sel.head);
            let edit = rope.insert_str("\n", at)?;
            sel.anchor = at;
            sel.head = at;
            sel.pref_col = 0;
            Some(edit)
        });
    }

    /// False when there was nothing to undo.
    pub fn undo(&mut self) -> bool {
        if self.read_only {
            self.refused = true;
            return true;
        }
        let Some(tx) = self.history.undo_stack.pop() else {
            return false;
        };
        for edit in tx.edits.iter().rev() {
            let inverse = edit.inverse();
            self.rope.apply_edit(&inverse);
            self.overlays.iter_mut().for_each(|o| o.follow(&inverse));
        }
        self.note_changed(&tx.edits, true);
        self.selections = tx.selections_before.clone();
        self.history.redo_stack.push(tx);
        true
    }

    /// False when there was nothing to redo.
    pub fn redo(&mut self) -> bool {
        if self.read_only {
            self.refused = true;
            return true;
        }
        let Some(tx) = self.history.redo_stack.pop() else {
            return false;
        };
        for edit in &tx.edits {
            self.rope.apply_edit(edit);
            self.overlays.iter_mut().for_each(|o| o.follow(edit));
        }
        self.note_changed(&tx.edits, false);
        self.selections = tx.selections_after.clone();
        self.history.undo_stack.push(tx);
        true
    }

    pub fn insert_char(&mut self, ch: char) {
        let mut buf = [0u8; 4];
        self.insert_str(ch.encode_utf8(&mut buf));
    }

    /// Inserts arbitrary text (e.g. register contents) at each selection the
    /// same way a single typed char would - same before/after cursor-landing
    /// rules as `insert_char`.
    pub fn insert_str(&mut self, s: &str) {
        self.insert_with(|_, _| s.to_string());
    }

    /// Shared before/after cursor-landing logic behind `insert_str` - `text_fn`
    /// computes the text to insert from each selection's own position, so
    /// callers (like `autoindent`'s per-line leading whitespace) can insert
    /// different text at each selection instead of one fixed string.
    fn insert_with(&mut self, mut text_fn: impl FnMut(&Rope, &Selection) -> String) {
        let before = self.insert_before;
        self.apply_edit_to_selections(|rope, sel| {
            let text = text_fn(rope, sel);
            let edit = if before {
                let near = sel.start();
                let far = sel.end();
                let edit = rope.insert_str(&text, near)?;
                let shift = edit.char_range.end - edit.char_range.start;
                sel.head = near + shift;
                sel.anchor = far + shift;
                edit
            } else {
                let fixed = sel.start();
                let insert_idx = (sel.end() + 1).min(rope.len_chars());
                let edit = rope.insert_str(&text, insert_idx)?;
                sel.anchor = fixed;
                sel.head = edit.char_range.end.saturating_sub(1);
                edit
            };
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    /// Inserts a single char the way insert mode actually types one -
    /// `autoindent`-aware on `\n`, `autopair`-aware on an opening
    /// bracket/quote. Everything else that inserts text programmatically
    /// (register paste, Lua, macro replay of non-typed commands) goes
    /// through `insert_char`/`insert_str` instead, which never does either.
    pub fn insert_typed_char(&mut self, ch: char) {
        if self.autopair {
            if let Some(close) = autopair_closer(ch) {
                // shortcut: always collapses to a single cursor between the
                // new pair, even if the selection being typed over was wide -
                // covers the common single-cursor case; upgrade to wrap a
                // wide selection in the pair instead if that's ever wanted.
                let before = self.insert_before;
                let paired = format!("{ch}{close}");
                self.apply_edit_to_selections(|rope, sel| {
                    let insert_idx = if before {
                        sel.start()
                    } else {
                        (sel.end() + 1).min(rope.len_chars())
                    };
                    let edit = rope.insert_str(&paired, insert_idx)?;
                    sel.anchor = edit.char_range.start + 1;
                    sel.head = edit.char_range.start + 1;
                    sel.pref_col = rope.char_to_col(sel.head);
                    Some(edit)
                });
                return;
            }
        }
        if ch == '\n' && self.autoindent {
            self.insert_with(|rope, sel| {
                let len = rope.len_chars();
                if len == 0 {
                    return "\n".to_string();
                }
                let line_idx = rope.char_to_line(sel.head.min(len - 1));
                let indent: String = rope
                    .line(line_idx)
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                format!("\n{indent}")
            });
            return;
        }
        self.insert_char(ch);
    }

    /// Inserts the named register's contents verbatim at each selection
    /// (Kakoune's insert-mode `<c-r>`) - unlike `paste_after`/`paste_before`,
    /// never linewise, just raw text at the cursor.
    pub fn insert_register(&mut self, registers: &mut RegisterManager, reg: char) {
        if let Some(text) = registers.get(reg) {
            self.insert_str(&text);
        }
    }

    /// Breaks the 500ms auto-coalescing in `apply_edit_to_selections` so the
    /// next edit starts a new undo step instead of merging into the last one
    /// (Kakoune's insert-mode `<c-u>`).
    pub fn commit_undo_group(&mut self) {
        if let Some(last_tx) = self.history.undo_stack.last_mut() {
            last_tx.time -= Duration::from_secs(1);
        }
    }

    pub fn delete_backward(&mut self) {
        let before = self.insert_before;
        self.apply_edit_to_selections(|rope, sel| {
            if rope.len_chars() == 0 {
                return None;
            }
            if before {
                let near = sel.start();
                let far = sel.end();
                if near == 0 {
                    return None;
                }
                let edit = rope.delete_range((near - 1)..near)?;
                sel.head = near - 1;
                sel.anchor = far.saturating_sub(1).max(near - 1);
                sel.pref_col = rope.char_to_col(sel.head);
                Some(edit)
            } else {
                let fixed = sel.start();
                let growing = sel.end();
                let edit = rope.delete_range(growing..(growing + 1))?;
                sel.anchor = fixed;
                sel.head = growing.saturating_sub(1).max(fixed);
                sel.pref_col = rope.char_to_col(sel.head);
                Some(edit)
            }
        })
    }

    pub fn delete_forward(&mut self) {
        let before = self.insert_before;
        self.apply_edit_to_selections(|rope, sel| {
            let len_chars = rope.len_chars();
            if len_chars == 0 {
                return None;
            }
            if before {
                let near = sel.start();
                let far = sel.end();
                if near >= len_chars {
                    return None;
                }
                let edit = rope.delete_range(near..(near + 1))?;
                sel.head = near;
                sel.anchor = far.saturating_sub(1).max(near);
                sel.pref_col = rope.char_to_col(sel.head);
                Some(edit)
            } else {
                let pos = sel.end() + 1;
                if pos >= len_chars {
                    return None;
                }
                rope.delete_range(pos..(pos + 1))
            }
        })
    }

    /// Kakoune's `<a-)>`/`<a-(>`: shifts each selection's *text* (not its
    /// range) to the next/previous selection, cyclically within consecutive
    /// groups of `count` selections (one group of all of them, if `count`
    /// is absent) - `3<a-)>` on 6 selections rotates (1,2,3) and (4,5,6)
    /// independently.
    pub fn rotate_selection_contents(&mut self, count: Option<usize>, backward: bool) {
        let n = self.selections.len();
        if n <= 1 {
            return;
        }
        let group_size = count.unwrap_or(n).clamp(1, n);

        let starts: Vec<usize> = self.selections.iter().map(|s| s.start()).collect();
        let texts: Vec<String> = self
            .selections
            .iter()
            .map(|sel| {
                let range = clamp_range(sel.range(), self.rope.len_chars());
                self.rope.slice(range).to_string()
            })
            .collect();

        let mut new_texts = texts.clone();
        for group_start in (0..n).step_by(group_size) {
            let group_len = group_size.min(n - group_start);
            for offset in 0..group_len {
                let source_offset = if backward {
                    (offset + 1) % group_len
                } else {
                    (offset + group_len - 1) % group_len
                };
                new_texts[group_start + offset] = texts[group_start + source_offset].clone();
            }
        }

        self.apply_edit_to_selections(|rope, sel| {
            let pos = starts.iter().position(|&s| s == sel.start())?;
            let range = clamp_range(sel.range(), rope.len_chars());
            rope.replace_range(range, &new_texts[pos])
        });
    }

    pub fn get_selected_text(&self) -> String {
        let mut text = String::new();

        for sel in &self.selections {
            let range = clamp_range(sel.range(), self.rope.len_chars());
            if range.is_empty() {
                continue;
            }
            let slice = self.rope.slice(range);
            text.push_str(&slice.to_string());
        }
        text
    }

    pub fn delete_and_yank(&mut self, registers: &mut RegisterManager, reg: char) {
        let text = self.get_selected_text();
        if text.is_empty() {
            return;
        }

        registers.set(reg, text);

        self.apply_edit_to_selections(|rope, sel| {
            let range = clamp_range(sel.range(), rope.len_chars());
            if range.is_empty() {
                return None;
            }

            let edit = rope.delete_range(range)?;
            let new_pos = edit.char_range.start;
            sel.anchor = new_pos;
            sel.head = new_pos;
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    pub fn delete_no_yank(&mut self) {
        self.apply_edit_to_selections(|rope, sel| {
            let range = clamp_range(sel.range(), rope.len_chars());
            if range.is_empty() {
                return None;
            }

            let edit = rope.delete_range(range)?;
            let new_pos = edit.char_range.start;
            sel.anchor = new_pos;
            sel.head = new_pos;
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    pub fn yank(&mut self, registers: &mut RegisterManager, reg: char) {
        let text = self.get_selected_text();
        if !text.is_empty() {
            registers.set(reg, text);
        }
    }

    pub fn paste_after(&mut self, registers: &mut RegisterManager, reg: char) {
        let Some(text) = registers.get(reg) else {
            return;
        };
        if text.is_empty() {
            return;
        }

        // line wise pasting
        if text.ends_with('\n') {
            self.apply_edit_to_selections(|rope, sel| {
                let line_idx = rope.char_to_line(sel.end());
                let line = rope.line(line_idx);
                let line_has_newline =
                    line.len_chars() > 0 && line.char(line.len_chars() - 1) == '\n';

                let (insert_idx, prefix_len) = if line_has_newline {
                    (rope.line_to_char(line_idx + 1), 0)
                } else {
                    (rope.len_chars(), 1)
                };
                let insert_text = if line_has_newline {
                    text.clone()
                } else {
                    format!("\n{text}")
                };

                let edit = rope.insert_str(&insert_text, insert_idx)?;
                sel.anchor = edit.char_range.start + prefix_len;
                sel.head = edit.char_range.end.saturating_sub(1);
                sel.pref_col = rope.char_to_col(sel.head);
                Some(edit)
            });
            return;
        }

        self.apply_edit_to_selections(|rope, sel| {
            let insert_idx = (sel.end() + 1).min(rope.len_chars());
            let edit = rope.insert_str(&text, insert_idx)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    pub fn paste_before(&mut self, registers: &mut RegisterManager, reg: char) {
        let Some(text) = registers.get(reg) else {
            return;
        };
        if text.is_empty() {
            return;
        }

        // line wise pasting
        let insert_idx_fn: fn(&Rope, &Selection) -> usize = if text.ends_with('\n') {
            |rope, sel| rope.line_to_char(rope.char_to_line(sel.start()))
        } else {
            |_rope, sel| sel.start()
        };

        self.apply_edit_to_selections(|rope, sel| {
            let insert_idx = insert_idx_fn(rope, sel);
            let edit = rope.insert_str(&text, insert_idx)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    /// Replaces each selection's text with the named register's contents
    /// (Kakoune's `R`/`<a-R>` - both read the same single-value register,
    /// see the `shortcut:` note on `RegisterManager`).
    pub fn replace_with_yanked(&mut self, registers: &mut RegisterManager, reg: char) {
        let Some(text) = registers.get(reg) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.apply_edit_to_selections(|rope, sel| {
            let range = clamp_range(sel.range(), rope.len_chars());
            if range.is_empty() {
                return None;
            }
            let edit = rope.replace_range(range, &text)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    pub fn replace_char(&mut self, ch: char) {
        self.apply_edit_to_selections(|rope, sel| {
            let range = clamp_range(sel.range(), rope.len_chars());
            if range.is_empty() {
                return None;
            }
            let replacement: String = std::iter::repeat_n(ch, range.end - range.start).collect();
            let edit = rope.replace_range(range, &replacement)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    fn transform_case(&mut self, map_char: impl Fn(char) -> String) {
        self.apply_edit_to_selections(|rope, sel| {
            let range = clamp_range(sel.range(), rope.len_chars());
            if range.is_empty() {
                return None;
            }
            let original = rope.slice(range.clone()).to_string();
            let transformed: String = original.chars().map(&map_char).collect();
            if transformed == original {
                return None;
            }
            let edit = rope.replace_range(range, &transformed)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    pub fn to_lowercase(&mut self) {
        self.transform_case(|c| c.to_lowercase().collect());
    }

    pub fn to_uppercase(&mut self) {
        self.transform_case(|c| c.to_uppercase().collect());
    }

    pub fn swap_case(&mut self) {
        self.transform_case(|c| {
            if c.is_uppercase() {
                c.to_lowercase().collect()
            } else if c.is_lowercase() {
                c.to_uppercase().collect()
            } else {
                c.to_string()
            }
        });
    }

    pub fn join_lines(&mut self) {
        self.join_lines_impl(false);
    }

    pub fn join_lines_select_spaces(&mut self) {
        self.join_lines_impl(true);
    }

    fn join_lines_impl(&mut self, select_spaces: bool) {
        self.apply_edit_to_selections(|rope, sel| {
            let range = clamp_range(sel.range(), rope.len_chars());
            if range.is_empty() {
                return None;
            }
            let (edit_range, joined, space_offsets) = scan_join(rope, range.start, range.end)?;
            let edit = rope.replace_range(edit_range, &joined)?;

            if select_spaces && !space_offsets.is_empty() {
                sel.anchor = edit.char_range.start + space_offsets[0];
                sel.head = edit.char_range.start + *space_offsets.last().unwrap();
            } else {
                sel.anchor = edit.char_range.start;
                sel.head = edit.char_range.end.saturating_sub(1);
            }
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    pub fn indent_lines(&mut self, include_empty: bool) {
        let indent_unit = " ".repeat(self.indent_width);
        self.apply_edit_to_selections(|rope, sel| {
            let (range, lines) = selected_line_range(rope, sel)?;
            let mut original = String::new();
            let mut transformed = String::new();
            for line_text in lines {
                original.push_str(&line_text);
                if include_empty || !line_text.trim_end_matches('\n').is_empty() {
                    transformed.push_str(&indent_unit);
                }
                transformed.push_str(&line_text);
            }
            if transformed == original {
                return None;
            }
            let edit = rope.replace_range(range, &transformed)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    pub fn unindent_lines(&mut self, preserve_incomplete: bool) {
        let indent_width = self.indent_width;
        self.apply_edit_to_selections(|rope, sel| {
            let (range, lines) = selected_line_range(rope, sel)?;
            let mut original = String::new();
            let mut transformed = String::new();
            for line_text in lines {
                original.push_str(&line_text);
                let leading_ws = line_text
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .count();
                let to_remove = if preserve_incomplete && leading_ws < indent_width {
                    0
                } else {
                    leading_ws.min(indent_width)
                };
                transformed.extend(line_text.chars().skip(to_remove));
            }
            if transformed == original {
                return None;
            }
            let edit = rope.replace_range(range, &transformed)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    /// Inserts spaces before each selection's start so every selection's
    /// cursor lands on the same column (Kakoune's `&`).
    pub fn align_cursors(&mut self) {
        let target_col = self
            .selections
            .iter()
            .map(|sel| self.rope.char_to_col(sel.start()))
            .max()
            .unwrap_or(0);

        self.apply_edit_to_selections(|rope, sel| {
            let start = sel.start();
            let col = rope.char_to_col(start);
            if col >= target_col {
                return None;
            }
            let padding = " ".repeat(target_col - col);
            let edit = rope.insert_str(&padding, start)?;
            let shift = edit.char_range.end - edit.char_range.start;
            sel.anchor += shift;
            sel.head += shift;
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    /// Copies the leading whitespace of the main selection's line (or the
    /// `count`-th selection's line, 1-indexed, when given) onto every other
    /// selected line (Kakoune's `<a-&>`).
    pub fn copy_indentation(&mut self, count: Option<usize>) {
        if self.rope.len_chars() == 0 {
            return;
        }
        let source_idx = count
            .map(|c| c.saturating_sub(1))
            .unwrap_or(0)
            .min(self.selections.len() - 1);
        let source_line = self.rope.char_to_line(
            self.selections[source_idx]
                .start()
                .min(self.rope.len_chars() - 1),
        );
        let indent: String = self
            .rope
            .line(source_line)
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();

        self.apply_edit_to_selections(|rope, sel| {
            let (range, lines) = selected_line_range(rope, sel)?;
            let mut original = String::new();
            let mut transformed = String::new();
            for line_text in lines {
                original.push_str(&line_text);
                let existing_ws = line_text
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .count();
                transformed.push_str(&indent);
                transformed.extend(line_text.chars().skip(existing_ws));
            }
            if transformed == original {
                return None;
            }
            let edit = rope.replace_range(range, &transformed)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    /// Converts each literal tab inside each selection to enough spaces to
    /// reach the next `tabstop` column (Kakoune's `@`).
    pub fn tabs_to_spaces(&mut self, tabstop: Option<usize>) {
        let tabstop = tabstop.unwrap_or(self.indent_width).max(1);
        self.apply_edit_to_selections(|rope, sel| {
            let range = clamp_range(sel.range(), rope.len_chars());
            if range.is_empty() {
                return None;
            }
            let mut col = rope.char_to_col(range.start);
            let mut transformed = String::new();
            let mut changed = false;
            for ch in rope.slice(range.clone()).chars() {
                if ch == '\t' {
                    let spaces = tabstop - (col % tabstop);
                    transformed.push_str(&" ".repeat(spaces));
                    col += spaces;
                    changed = true;
                } else {
                    transformed.push(ch);
                    col = if ch == '\n' { 0 } else { col + 1 };
                }
            }
            if !changed {
                return None;
            }
            let edit = rope.replace_range(range, &transformed)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    /// Converts runs of spaces that reach a `tabstop` column boundary into
    /// tabs inside each selection (Kakoune's `<a-@>`, the inverse of
    /// `tabs_to_spaces`).
    pub fn spaces_to_tabs(&mut self, tabstop: Option<usize>) {
        let tabstop = tabstop.unwrap_or(self.indent_width).max(1);
        self.apply_edit_to_selections(|rope, sel| {
            let range = clamp_range(sel.range(), rope.len_chars());
            if range.is_empty() {
                return None;
            }
            let mut col = rope.char_to_col(range.start);
            let chars: Vec<char> = rope.slice(range.clone()).chars().collect();
            let mut transformed = String::new();
            let mut changed = false;
            let mut i = 0;
            while i < chars.len() {
                let ch = chars[i];
                if ch == ' ' && col % tabstop == 0 {
                    let mut run = 0;
                    while i + run < chars.len() && chars[i + run] == ' ' {
                        run += 1;
                    }
                    let tabs = run / tabstop;
                    if tabs > 0 {
                        let rest = run % tabstop;
                        transformed.extend(std::iter::repeat_n('\t', tabs));
                        transformed.extend(std::iter::repeat_n(' ', rest));
                        col += run;
                        i += run;
                        changed = true;
                        continue;
                    }
                }
                transformed.push(ch);
                col = if ch == '\n' { 0 } else { col + 1 };
                i += 1;
            }
            if !changed {
                return None;
            }
            let edit = rope.replace_range(range, &transformed)?;
            sel.anchor = edit.char_range.start;
            sel.head = edit.char_range.end.saturating_sub(1);
            sel.pref_col = rope.char_to_col(sel.head);
            Some(edit)
        });
    }

    pub fn apply_movement<F>(&mut self, mode: SelectionMode, mut move_fn: F)
    where
        F: FnMut(&mut Rope, &mut Selection) -> bool,
    {
        let buffer = &mut self.rope;
        for sel in &mut self.selections {
            let should_update_pref_col = move_fn(buffer, sel);
            if should_update_pref_col {
                sel.pref_col = buffer.char_to_col(sel.head);
            }
            if mode == SelectionMode::Jump {
                sel.anchor = sel.head;
            }
        }
        self.merge_selections()
    }

    pub fn select_pair_at(&mut self, open: char, close: char, mode: ObjectMode, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let mut found = rope.pair_object_at(sel.head, open, close, mode);
            // Already selecting exactly this pair: a repeat (or count) grows to the one around it.
            if !sel.is_empty() && found == Some((sel.start(), sel.end() + 1)) {
                let (start, _) = found.unwrap();
                let open_idx = if mode == ObjectMode::Inner {
                    start - 1
                } else {
                    start
                };
                found = open_idx
                    .checked_sub(1)
                    .and_then(|from| rope.pair_object_at(from, open, close, mode))
                    .filter(|&outer| Some(outer) != found);
            }
            if let Some((start, end)) = found {
                sel.apply_range(start, end, extend);
            }
            true
        });
    }

    pub fn select_tag_at(&mut self, mode: ObjectMode, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let grow = !sel.is_empty();
            if let Some((start, end)) = rope.tag_object_at(sel.start(), sel.end() + 1, grow, mode) {
                sel.apply_range(start, end, extend);
            }
            true
        });
    }

    pub fn select_matching_pair(&mut self, forward: bool, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            if let Some((start, end)) = rope.matching_pair_at(sel.head, forward) {
                sel.apply_range(start, end, extend);
            }
            true
        });
    }

    pub fn select_whitespace_at(&mut self, mode: ObjectMode, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.whitespace_object_at(sel.head, mode);
            sel.apply_range(start, end, extend);
            true
        });
    }

    pub fn select_number_at(&mut self, mode: ObjectMode, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.number_object_at(sel.head, mode);
            sel.apply_range(start, end, extend);
            true
        });
    }

    pub fn select_indent_at(&mut self, mode: ObjectMode, extend: bool) {
        let indent_width = self.indent_width;
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.indent_object_at(sel.head, mode, indent_width);
            sel.apply_range(start, end, extend);
            true
        });
    }

    pub fn select_paragraph_at(&mut self, mode: ObjectMode, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.paragraph_object_at(sel.head, mode);
            sel.apply_range(start, end, extend);
            true
        });
    }

    pub fn select_sentence_at(&mut self, mode: ObjectMode, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.sentence_object_at(sel.head, mode);
            sel.apply_range(start, end, extend);
            true
        });
    }

    pub fn select_argument_at(&mut self, mode: ObjectMode, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.argument_object_at(sel.head, mode);
            sel.apply_range(start, end, extend);
            true
        });
    }

    pub fn select_all(&mut self) {
        self.selections = vec![Selection::default()];
        self.selections[0].head = self.rope.buffer_range().end;
        self.selections[0].anchor = self.rope.buffer_range().start;
    }

    pub fn move_in_buffer(&mut self, movement: BufferMovement, extend: bool) {
        self.apply_movement(sel_mode(extend), |buffer, sel| {
            let Range { start, end } = buffer.buffer_range();
            sel.head = match movement {
                BufferMovement::Home => start,
                BufferMovement::End => end,
            };
            true
        });
    }

    /// goes to the first char of the last line
    pub fn goto_last_line(&mut self, extend: bool) {
        self.apply_movement(sel_mode(extend), |rope, sel| {
            let last_line = rope.len_lines().saturating_sub(1);
            sel.head = rope.line_to_char(last_line);
            true
        });
    }

    /// goes to the first char of the given 1-indexed line
    pub fn goto_line(&mut self, line: usize, extend: bool) {
        self.apply_movement(sel_mode(extend), |rope, sel| {
            let last_line = rope.len_lines().saturating_sub(1);
            let target = line.saturating_sub(1).min(last_line);
            sel.head = rope.line_to_char(target);
            true
        });
    }

    /// Jumps to the cursor position right after the last undo-stack entry
    /// (Kakoune's `g .`) - a no-op if nothing has been edited yet.
    pub fn goto_last_modification_position(&mut self, extend: bool) {
        let Some(target) = self
            .history
            .undo_stack
            .last()
            .and_then(|tx| tx.selections_after.first())
            .map(|sel| sel.head)
        else {
            return;
        };
        self.apply_movement(sel_mode(extend), |_, sel| {
            sel.head = target;
            true
        });
    }

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

    pub fn find_char(&mut self, search: CharSearch, count: Option<usize>, extend: bool) {
        self.last_char_search = Some(search);
        self.run_char_search(search, count.unwrap_or(1), extend, false);
    }

    pub fn repeat_char_search(&mut self, extend: bool) {
        if let Some(search) = self.last_char_search {
            self.run_char_search(search, 1, extend, true);
        }
    }

    fn run_char_search(&mut self, search: CharSearch, count: usize, extend: bool, is_repeat: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let from = if is_repeat && search.kind == FindKind::Until {
                match search.direction {
                    FindDirection::Forward => rope.next_grapheme(sel.head),
                    FindDirection::Backward => rope.prev_grapheme(sel.head),
                }
            } else {
                sel.head
            };

            let found = match search.direction {
                FindDirection::Forward => rope.find_char_forward(from, search.target, count),
                FindDirection::Backward => rope.find_char_backward(from, search.target, count),
            };

            if let Some(found) = found {
                let target = match (search.kind, search.direction) {
                    (FindKind::Until, FindDirection::Forward) => rope.prev_grapheme(found),
                    (FindKind::Until, FindDirection::Backward) => rope.next_grapheme(found),
                    (FindKind::To, _) => found,
                };
                if extend {
                    sel.head = target;
                } else {
                    sel.anchor = sel.head;
                    sel.head = target;
                }
            }
            true
        });
    }

    pub fn move_in_line(&mut self, movement: LineMovement, extend: bool) {
        self.apply_movement(sel_mode(extend), |buffer, sel| {
            sel.head = match movement {
                LineMovement::LineHome => buffer.line_start(sel.head),
                LineMovement::LineEnd => buffer.line_end(sel.head),
                LineMovement::LineHomeNoWhitespace => buffer.first_non_whitespace_of_line(sel.head),
            };
            true
        });
    }

    pub fn select_line(&mut self) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.extend_line_selection(sel.anchor, sel.head);
            sel.anchor = start;
            sel.head = end;
            true
        });
    }

    /// Shrinks each selection down to the full lines it fully contains,
    /// dropping any partial line at either edge and the last line's
    /// trailing end-of-line (Kakoune's `<a-x>`, the inverse of `select_line`).
    pub fn trim_line_selection(&mut self) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let len = rope.len_chars();
            if len == 0 {
                return false;
            }
            let start = sel.start().min(len - 1);
            let end_excl = (sel.end() + 1).min(len);

            let start_line = rope.char_to_line(start);
            let first_full_line = if start == rope.line_to_char(start_line) {
                start_line
            } else {
                start_line + 1
            };

            let end_line = rope.char_to_line((end_excl - 1).min(len - 1));
            let next_line_start = if end_line + 1 < rope.len_lines() {
                rope.line_to_char(end_line + 1)
            } else {
                len
            };
            let last_full_line = if end_excl >= next_line_start {
                Some(end_line)
            } else {
                end_line.checked_sub(1)
            };
            let Some(last_full_line) = last_full_line else {
                return false;
            };
            if first_full_line > last_full_line || first_full_line >= rope.len_lines() {
                return false;
            }

            let new_start = rope.line_to_char(first_full_line);
            let new_end_excl = rope.line_end(rope.line_to_char(last_full_line));
            sel.anchor = new_start;
            sel.head = new_end_excl.saturating_sub(1).max(new_start);
            true
        });
    }

    pub fn move_word(&mut self, movement: WordMovement, kind: WordKind, extend: bool) {
        let selector = match movement {
            WordMovement::NextStart => Rope::next_word_start_range,
            WordMovement::PrevStart => Rope::prev_word_start_range,
            WordMovement::NextEnd => Rope::next_word_end_range,
        };

        self.apply_movement(SelectionMode::SetExplicit, |buffer, sel| {
            let (anchor, head) = selector(buffer, sel.head, kind);
            if extend {
                sel.head = head;
            } else {
                sel.anchor = anchor;
                sel.head = head;
            }
            true
        });
    }

    pub fn select_word_at(&mut self, kind: WordKind, mode: ObjectMode, extend: bool) {
        self.apply_movement(SelectionMode::SetExplicit, |rope, sel| {
            let (start, end) = rope.word_object_at(sel.head, kind, mode);
            sel.apply_range(start, end, extend);
            true
        });
    }

    fn resolve_pattern(
        registers: &mut RegisterManager,
        pattern: Option<String>,
        register: char,
    ) -> Result<String, String> {
        match pattern {
            Some(p) => {
                registers.set(register, p.clone());
                Ok(p)
            }
            None => registers
                .get(register)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| "no previous search pattern".to_string()),
        }
    }

    /// For every selection independently, jump/extend to the next match of
    /// `pattern` after it, wrapping around to the first match in the buffer
    /// if none follows.
    pub fn search_next(
        &mut self,
        registers: &mut RegisterManager,
        pattern: Option<String>,
        register: char,
        extend: bool,
    ) -> Result<(), String> {
        self.search(registers, pattern, register, extend, true)
    }

    /// Same as `search_next`, searching backward (wrapping to the last match).
    pub fn search_previous(
        &mut self,
        registers: &mut RegisterManager,
        pattern: Option<String>,
        register: char,
        extend: bool,
    ) -> Result<(), String> {
        self.search(registers, pattern, register, extend, false)
    }

    fn search(
        &mut self,
        registers: &mut RegisterManager,
        pattern: Option<String>,
        register: char,
        extend: bool,
        forward: bool,
    ) -> Result<(), String> {
        let pat = Self::resolve_pattern(registers, pattern, register)?;
        let re = Regex::new(&pat).map_err(|e| format!("invalid pattern: {e}"))?;
        let text = self.rope.to_string();

        let matches: Vec<(usize, usize)> = re
            .find_iter(&text)
            .map(|m| {
                (
                    self.rope.byte_to_char(m.start()),
                    self.rope.byte_to_char(m.end()),
                )
            })
            .collect();
        if matches.is_empty() {
            return Err("no matches".to_string());
        }

        let rope = &self.rope;
        for sel in &mut self.selections {
            let (cur_start, cur_end) = (sel.start(), sel.end());
            let target = if forward {
                matches
                    .iter()
                    .find(|&&(s, _)| s > cur_end)
                    .or_else(|| matches.first())
            } else {
                matches
                    .iter()
                    .rev()
                    .find(|&&(_, e)| e <= cur_start)
                    .or_else(|| matches.last())
            };
            let &(start, end) = target.expect("checked non-empty above");
            let head = end.saturating_sub(1).max(start);
            if extend {
                sel.head = head;
            } else {
                sel.anchor = start;
                sel.head = head;
            }
            sel.pref_col = rope.char_to_col(sel.head);
        }
        self.merge_selections();
        Ok(())
    }

    fn match_main(
        &mut self,
        registers: &mut RegisterManager,
        register: char,
        forward: bool,
        add: bool,
    ) -> Result<(), String> {
        let pat = registers
            .get(register)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| "no previous search pattern".to_string())?;
        self.match_main_pattern(&pat, forward, add).map(|_| ())
    }

    /// `n`/`N`: adds the next/previous occurrence of the main selection's
    /// text as the new main selection. On a bare cursor over a word it
    /// selects that word first instead (Sublime's Ctrl-D).
    pub fn add_occurrence(&mut self, forward: bool) -> Result<(), String> {
        let Some(main) = self.selections.first().copied() else {
            return Ok(());
        };
        let len = self.rope.len_chars();
        if len == 0 {
            return Ok(());
        }
        if main.is_empty() && is_word(self.rope.char(main.head.min(len - 1))) {
            let (start, end) =
                self.rope
                    .word_object_at(main.head, WordKind::Word, ObjectMode::Inner);
            // A one-char word is already selected; go on to its next occurrence.
            if end - start > 1 {
                self.selections[0].apply_range(start, end, false);
                return Ok(());
            }
        }
        let pat = self.occurrence_pattern(main);
        self.match_main_pattern(&pat, forward, true).map(|_| ())
    }

    /// The regex for `sel`'s text as an occurrence: `\b` only where it sits on a word
    /// edge, so a whole word matches whole words but a partial one still matches inside words.
    fn occurrence_pattern(&self, sel: Selection) -> String {
        let len = self.rope.len_chars();
        let range = clamp_range(sel.range(), len);
        let text = self.rope.slice(range.clone()).to_string();
        let at_edge = |inside: Option<char>, outside: Option<char>| {
            inside.is_some_and(is_word) && !outside.is_some_and(is_word)
        };
        let before = range.start.checked_sub(1).map(|i| self.rope.char(i));
        let after = (range.end < len).then(|| self.rope.char(range.end));
        let mut pat = regex::escape(&text);
        if at_edge(text.chars().next(), before) {
            pat = format!(r"\b{pat}");
        }
        if at_edge(text.chars().last(), after) {
            pat = format!(r"{pat}\b");
        }
        pat
    }

    /// VS Code's Cmd+Shift+L: every occurrence of the main selection (from a bare
    /// cursor, of the word under it), the main one staying main.
    pub fn select_all_occurrences(&mut self) -> Result<(), String> {
        let Some(mut main) = self.selections.first().copied() else {
            return Ok(());
        };
        let len = self.rope.len_chars();
        if len == 0 {
            return Ok(());
        }
        if main.is_empty() && is_word(self.rope.char(main.head.min(len - 1))) {
            let (start, end) =
                self.rope
                    .word_object_at(main.head, WordKind::Word, ObjectMode::Inner);
            main.apply_range(start, end, false);
        }
        let re = Regex::new(&self.occurrence_pattern(main))
            .map_err(|e| format!("invalid pattern: {e}"))?;
        let text = self.rope.to_string();
        let mut found: Vec<Selection> = re
            .find_iter(&text)
            .filter(|m| !m.is_empty())
            .map(|m| {
                let mut sel = Selection::default();
                sel.apply_range(
                    char_offset(&text, m.start()),
                    char_offset(&text, m.end()),
                    false,
                );
                sel
            })
            .collect();
        if let Some(i) = found.iter().position(|s| s.start() == main.start()) {
            found.swap(0, i);
        }
        if !found.is_empty() {
            self.selections = found;
        }
        Ok(())
    }

    /// Ok(true) when the search wrapped around the buffer's end (or start).
    pub fn match_main_pattern(
        &mut self,
        pat: &str,
        forward: bool,
        add: bool,
    ) -> Result<bool, String> {
        let re = Regex::new(pat).map_err(|e| format!("invalid pattern: {e}"))?;
        let text = self.rope.to_string();

        let matches: Vec<(usize, usize)> = re
            .find_iter(&text)
            .map(|m| {
                (
                    self.rope.byte_to_char(m.start()),
                    self.rope.byte_to_char(m.end()),
                )
            })
            .collect();
        if matches.is_empty() {
            return Err("no matches".to_string());
        }

        let Some(main) = self.selections.first().copied() else {
            return Ok(false);
        };
        let (cur_start, cur_end) = (main.start(), main.end());
        let ahead = if forward {
            matches.iter().find(|&&(s, _)| s > cur_end)
        } else {
            matches.iter().rev().find(|&&(_, e)| e <= cur_start)
        };
        let wrapped = ahead.is_none();
        let target = ahead.or(if forward {
            matches.first()
        } else {
            matches.last()
        });
        let &(start, end) = target.expect("checked non-empty above");
        let head = end.saturating_sub(1).max(start);
        let new_sel = Selection {
            anchor: start,
            head,
            pref_col: self.rope.char_to_col(head),
        };

        if add {
            // New match becomes main, so the next `n` continues from it and `<a-,>` drops it.
            self.selections.insert(0, new_sel);
        } else {
            self.selections[0] = new_sel;
        }
        self.merge_selections();
        Ok(wrapped)
    }

    pub fn select_next_match_main(
        &mut self,
        registers: &mut RegisterManager,
        register: char,
    ) -> Result<(), String> {
        self.match_main(registers, register, true, false)
    }

    pub fn select_previous_match_main(
        &mut self,
        registers: &mut RegisterManager,
        register: char,
    ) -> Result<(), String> {
        self.match_main(registers, register, false, false)
    }

    pub fn add_next_match_selection(
        &mut self,
        registers: &mut RegisterManager,
        register: char,
    ) -> Result<(), String> {
        self.match_main(registers, register, true, true)
    }

    pub fn add_previous_match_selection(
        &mut self,
        registers: &mut RegisterManager,
        register: char,
    ) -> Result<(), String> {
        self.match_main(registers, register, false, true)
    }

    /// Sets the search register to the main selection's text, escaped so
    /// it's matched literally. `smart_boundaries` wraps it in `\b` anchors
    /// so e.g. selecting "foo" doesn't also match inside "foobar".
    pub fn set_search_pattern_from_main_selection(
        &mut self,
        registers: &mut RegisterManager,
        smart_boundaries: bool,
        register: char,
    ) {
        let Some(sel) = self.selections.first() else {
            return;
        };
        let range = clamp_range(sel.range(), self.rope.len_chars());
        if range.is_empty() {
            return;
        }
        let text = self.rope.slice(range).to_string();
        let escaped = regex::escape(&text);
        let pattern = if smart_boundaries {
            format!(r"\b{escaped}\b")
        } else {
            escaped
        };
        registers.set(register, pattern);
    }

    /// Replaces every selection with one new selection per match of `regex`
    /// found inside it (or per `capture` group, if given).
    pub fn select_regex_matches(
        &mut self,
        regex: &str,
        capture: Option<usize>,
    ) -> Result<(), String> {
        let re = Regex::new(regex).map_err(|e| format!("invalid pattern: {e}"))?;
        let len_chars = self.rope.len_chars();
        let mut new_selections = Vec::new();

        for sel in &self.selections {
            let range = clamp_range(sel.range(), len_chars);
            if range.is_empty() {
                continue;
            }
            let text = self.rope.slice(range.clone()).to_string();
            let base = range.start;

            for caps in re.captures_iter(&text) {
                let Some(m) = (match capture {
                    Some(idx) => caps.get(idx),
                    None => caps.get(0),
                }) else {
                    continue;
                };
                let start = base + char_offset(&text, m.start());
                let end = base + char_offset(&text, m.end());
                if end > start {
                    new_selections.push(Selection {
                        anchor: start,
                        head: end - 1,
                        pref_col: 0,
                    });
                }
            }
        }

        if new_selections.is_empty() {
            return Err("no matches".to_string());
        }
        self.replace_selections(new_selections);
        Ok(())
    }

    /// Replaces every selection with the gaps between matches of `regex`
    /// inside it (or between `capture` groups, if given) - the inverse of
    /// `select_regex_matches`.
    pub fn split_selections_regex(
        &mut self,
        regex: &str,
        capture: Option<usize>,
    ) -> Result<(), String> {
        let re = Regex::new(regex).map_err(|e| format!("invalid pattern: {e}"))?;
        let len_chars = self.rope.len_chars();
        let mut new_selections = Vec::new();

        for sel in &self.selections {
            let range = clamp_range(sel.range(), len_chars);
            if range.is_empty() {
                continue;
            }
            let text = self.rope.slice(range.clone()).to_string();
            let base = range.start;

            let delimiters: Vec<(usize, usize)> = re
                .captures_iter(&text)
                .filter_map(|caps| match capture {
                    Some(idx) => caps.get(idx),
                    None => caps.get(0),
                })
                .filter(|m| !m.as_str().is_empty())
                .map(|m| (m.start(), m.end()))
                .collect();

            if delimiters.is_empty() {
                new_selections.push(*sel);
                continue;
            }

            let mut cursor = 0usize;
            for &(d_start, d_end) in &delimiters {
                if d_start > cursor {
                    let start = base + char_offset(&text, cursor);
                    let end = base + char_offset(&text, d_start);
                    if end > start {
                        new_selections.push(Selection {
                            anchor: start,
                            head: end - 1,
                            pref_col: 0,
                        });
                    }
                }
                cursor = d_end;
            }
            if cursor < text.len() {
                let start = base + char_offset(&text, cursor);
                let end = base + char_offset(&text, text.len());
                if end > start {
                    new_selections.push(Selection {
                        anchor: start,
                        head: end - 1,
                        pref_col: 0,
                    });
                }
            }
        }

        if new_selections.is_empty() {
            return Err("split produced no selections".to_string());
        }
        self.replace_selections(new_selections);
        Ok(())
    }

    /// Kakoune's `<a-s>`: replaces each selection with one selection per
    /// line it spans, clipped to the original selection's bounds.
    pub fn split_selections_lines(&mut self) {
        let rope = &self.rope;
        let len_chars = rope.len_chars();
        let mut new_selections = Vec::new();

        for sel in &self.selections {
            let range = clamp_range(sel.range(), len_chars);
            if range.is_empty() {
                new_selections.push(*sel);
                continue;
            }
            let start_line = rope.char_to_line(range.start);
            let end_line = rope.char_to_line(range.end - 1);
            for line in start_line..=end_line {
                let line_start = rope.line_to_char(line).max(range.start);
                let line_end = if line + 1 < rope.len_lines() {
                    rope.line_to_char(line + 1)
                } else {
                    len_chars
                }
                .min(range.end);
                if line_end > line_start {
                    new_selections.push(Selection {
                        anchor: line_start,
                        head: line_end - 1,
                        pref_col: 0,
                    });
                }
            }
        }

        self.replace_selections(new_selections);
    }

    /// Kakoune's `<a-S>`: replaces each selection with cursors on its first
    /// and last characters (just the first, if it's already a single char).
    pub fn select_endpoints(&mut self) {
        let mut new_selections = Vec::new();
        for sel in &self.selections {
            let (start, end) = (sel.start(), sel.end());
            new_selections.push(Selection {
                anchor: start,
                head: start,
                pref_col: 0,
            });
            if end != start {
                new_selections.push(Selection {
                    anchor: end,
                    head: end,
                    pref_col: 0,
                });
            }
        }
        self.replace_selections(new_selections);
    }

    fn replace_selections(&mut self, mut new_selections: Vec<Selection>) {
        new_selections.sort_by_key(|s| s.start());
        let rope = &self.rope;
        for s in &mut new_selections {
            s.pref_col = rope.char_to_col(s.head);
        }
        self.selections = new_selections;
        self.merge_selections();
    }

    /// Keeps only the selections whose text matches `regex`.
    pub fn keep_matching_selections(&mut self, regex: &str) -> Result<(), String> {
        self.filter_matching_selections(regex, true)
    }

    /// Drops the selections whose text matches `regex`.
    pub fn clear_matching_selections(&mut self, regex: &str) -> Result<(), String> {
        self.filter_matching_selections(regex, false)
    }

    fn filter_matching_selections(
        &mut self,
        regex: &str,
        keep_if_match: bool,
    ) -> Result<(), String> {
        let re = Regex::new(regex).map_err(|e| format!("invalid pattern: {e}"))?;
        let len_chars = self.rope.len_chars();
        let rope = &self.rope;

        let kept: Vec<Selection> = self
            .selections
            .iter()
            .filter(|sel| {
                let range = clamp_range(sel.range(), len_chars);
                let text = rope.slice(range).to_string();
                re.is_match(&text) == keep_if_match
            })
            .copied()
            .collect();

        if kept.is_empty() {
            return Err("no selections left".to_string());
        }
        self.selections = kept;
        Ok(())
    }

    /// Kakoune's `_`: trims whitespace off each selection's edges, dropping
    /// any selection that turns out to be whitespace-only.
    pub fn unselect_whitespace(&mut self) -> Result<(), String> {
        let len_chars = self.rope.len_chars();
        let rope = &self.rope;

        let mut kept: Vec<Selection> = self
            .selections
            .iter()
            .filter_map(|sel| {
                let range = clamp_range(sel.range(), len_chars);
                let mut start = range.start;
                let mut end = range.end;
                while start < end && rope.char(start).is_whitespace() {
                    start += 1;
                }
                while start < end && rope.char(end - 1).is_whitespace() {
                    end -= 1;
                }
                (start < end).then_some(Selection {
                    anchor: start,
                    head: end - 1,
                    pref_col: 0,
                })
            })
            .collect();

        if kept.is_empty() {
            return Err("no selections left".to_string());
        }
        for s in &mut kept {
            s.pref_col = rope.char_to_col(s.head);
        }
        self.selections = kept;
        Ok(())
    }
}

/// The closing char `autopair` inserts right after a typed opener, or `None`
/// if `ch` isn't one of the pairable brackets/quotes.
fn autopair_closer(ch: char) -> Option<char> {
    match ch {
        '(' => Some(')'),
        '{' => Some('}'),
        '[' => Some(']'),
        '"' => Some('"'),
        '\'' => Some('\''),
        '`' => Some('`'),
        _ => None,
    }
}

/// The whole lines (start char idx of the first to end char idx of the last)
/// touched by `sel`, as a replaceable range plus each line's own text.
fn selected_line_range(rope: &Rope, sel: &Selection) -> Option<(Range<usize>, Vec<String>)> {
    let len_chars = rope.len_chars();
    if len_chars == 0 {
        return None;
    }
    let start_line = rope.char_to_line(sel.start().min(len_chars - 1));
    let end_line = rope.char_to_line(sel.end().min(len_chars - 1));

    let start = rope.line_to_char(start_line);
    let lines: Vec<String> = (start_line..=end_line)
        .map(|idx| rope.line(idx).to_string())
        .collect();
    let total_len: usize = lines.iter().map(|l| l.chars().count()).sum();

    Some((start..(start + total_len), lines))
}

fn scan_join(rope: &Rope, start: usize, end: usize) -> Option<(Range<usize>, String, Vec<usize>)> {
    let len_chars = rope.len_chars();
    let mut result = String::new();
    let mut space_offsets = Vec::new();
    let mut i = start;
    let mut changed = false;

    while i < end.min(len_chars) {
        let c = rope.char(i);
        if c == '\n' {
            space_offsets.push(result.chars().count());
            result.push(' ');
            i += 1;
            while i < len_chars {
                let next = rope.char(i);
                if next == ' ' || next == '\t' {
                    i += 1;
                } else {
                    break;
                }
            }
            changed = true;
        } else {
            result.push(c);
            i += 1;
        }
    }

    if !changed {
        return None;
    }
    Some((start..i, result, space_offsets))
}

/// Counts chars in `text[..byte_idx]` to turn a regex match's byte offset
/// (regex operates on `&str`) into a char offset within that same `text`.
fn char_offset(text: &str, byte_idx: usize) -> usize {
    text[..byte_idx].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_point_puts_one_cursor_or_extends_the_main_selection() {
        let mut buf = Buffer::from_str("hello world");
        buf.set_point(4, false);
        assert_eq!((buf.selections[0].anchor, buf.selections[0].head), (4, 4));
        buf.set_point(8, true);
        assert_eq!(
            (
                buf.selections.len(),
                buf.selections[0].anchor,
                buf.selections[0].head
            ),
            (1, 4, 8)
        );
        // Past the end is where typing at the end lands; further is clamped to it.
        buf.set_point(99, false);
        assert_eq!(buf.selections[0].head, 11);
    }

    fn at(buf: &mut Buffer, pos: usize) {
        buf.selections = vec![Selection {
            anchor: pos,
            head: pos,
            pref_col: 0,
        }];
    }

    #[test]
    fn replace_range_is_one_undoable_change_that_shifts_selections() {
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 8);
        buf.replace_range(0..5, "goodbye");
        assert_eq!(buf.rope.to_string(), "goodbye world");
        assert_eq!(buf.selections[0].head, 10); // still on the `r`
        assert_eq!(buf.take_changed(), Some(0..7));
        assert_eq!(buf.take_changed(), None);
        buf.replace_range(13..13, "!");
        assert_eq!(buf.rope.to_string(), "goodbye world!");
        assert_eq!(buf.take_changed(), Some(13..14));
        assert!(buf.undo() && buf.undo());
        assert_eq!(buf.rope.to_string(), "hello world");
        assert_eq!(buf.take_changed(), Some(0..13));
    }

    #[test]
    fn a_read_only_buffer_refuses_edits_but_takes_a_scripts_writes() {
        let mut buf = Buffer::from_str("");
        buf.dirty = false;
        buf.read_only = true;
        buf.name = Some("*results*".into());
        buf.replace_range(0..0, "line\n");
        assert_eq!(buf.rope.to_string(), "line\n");
        assert!(!buf.dirty, "a special buffer is never unsaved");
        at(&mut buf, 0);
        buf.enter_insert_before();
        buf.insert_char('x');
        buf.delete_forward();
        assert!(buf.refused);
        assert!(buf.undo());
        assert_eq!(buf.rope.to_string(), "line\n");
    }

    fn pair(buf: &mut Buffer, pos: usize, open: char, close: char, mode: ObjectMode) -> String {
        at(buf, pos);
        buf.select_pair_at(open, close, mode, false);
        let r = buf.selections[0].range();
        buf.rope.slice(r).to_string()
    }

    #[test]
    fn pair_objects_follow_nvim_surrounding_else_next() {
        let src = "struct A {\n    m: M,\n}\n\nfn p() {\n    let x = S { a: f(1) };\n}\n";
        let mut buf = Buffer::from_str(src);
        let blank = src.find("\n\nfn").unwrap() + 1;
        let let_ = src.find("let").unwrap();
        // Blank line between items: nothing surrounds, so the next `{}` ahead (fn body).
        assert_eq!(
            pair(&mut buf, blank, '{', '}', ObjectMode::Inner),
            "\n    let x = S { a: f(1) };\n"
        );
        // Inside the fn body: the surrounding pair wins over the `{ a: .. }` on the line.
        assert_eq!(
            pair(&mut buf, let_, '{', '}', ObjectMode::Around),
            "{\n    let x = S { a: f(1) };\n}"
        );
        // No `()` around `let`: the next one ahead.
        assert_eq!(pair(&mut buf, let_, '(', ')', ObjectMode::Inner), "1");
        // Cursor on a bracket counts as inside it.
        let paren = src.find("(1)").unwrap();
        assert_eq!(
            pair(&mut buf, paren + 2, '(', ')', ObjectMode::Around),
            "(1)"
        );
    }

    #[test]
    fn quote_objects_pair_along_the_line() {
        let src = r#"x "a" y "b\"c" z"#;
        let mut buf = Buffer::from_str(src);
        assert_eq!(pair(&mut buf, 0, '"', '"', ObjectMode::Inner), "a");
        // Between two strings: the next one, not `" y "`; escaped quote skipped.
        assert_eq!(
            pair(
                &mut buf,
                src.find('y').unwrap(),
                '"',
                '"',
                ObjectMode::Inner
            ),
            r#"b\"c"#
        );
        // Nothing ahead on the line.
        at(&mut buf, src.len() - 1);
        buf.select_pair_at('"', '"', ObjectMode::Inner, false);
        assert_eq!(buf.selections[0].range(), src.len() - 1..src.len());
    }

    #[test]
    fn n_selects_the_word_then_adds_whole_word_occurrences_as_main() {
        let mut buf = Buffer::from_str("foo bar foobar foo foo");
        let ranges = |b: &Buffer| b.selections.iter().map(|s| s.range()).collect::<Vec<_>>();
        at(&mut buf, 1);
        buf.add_occurrence(true).unwrap(); // bare cursor: just selects the word
        assert_eq!(ranges(&buf), vec![0..3]);
        buf.add_occurrence(true).unwrap(); // skips `foobar`, new match is main
        assert_eq!(ranges(&buf), vec![15..18, 0..3]);
        buf.add_occurrence(true).unwrap(); // continues from the newest, not the leftmost
        assert_eq!(ranges(&buf), vec![19..22, 0..3, 15..18]);
        buf.clear_main_selection(); // `<a-,>` undoes the last add
        assert_eq!(ranges(&buf), vec![0..3, 15..18]);
        // `N` adds the previous one, wrapping past the start.
        buf.add_occurrence(false).unwrap();
        assert_eq!(buf.selections[0].range(), 19..22);
        // A partial selection matches inside words too.
        buf.selections = vec![Selection {
            anchor: 9,
            head: 10,
            pref_col: 0,
        }]; // "oo" in foobar
        buf.add_occurrence(true).unwrap();
        assert_eq!(buf.selections[0].range(), 16..18);
    }

    #[test]
    fn search_session_previews_steps_adds_and_ends() {
        let mut buf = Buffer::from_str("a foo b foo c foo");
        let ranges = |b: &Buffer| b.selections.iter().map(|s| s.range()).collect::<Vec<_>>();
        at(&mut buf, 0);
        buf.search_begin();
        buf.search_preview("fo".into(), false, false);
        assert_eq!(ranges(&buf), vec![2..4]);
        buf.search_preview("foo".into(), false, false); // still measured from the origin
        assert_eq!(ranges(&buf), vec![2..5]);
        buf.search_preview("fo(".into(), false, false); // invalid while typing: sits at the origin
        assert_eq!(ranges(&buf), vec![0..1]);
        buf.search_preview("foo".into(), false, false);
        assert_eq!(buf.search_step(true), Ok(false));
        buf.search_add();
        assert_eq!(buf.search_step(true), Ok(false));
        assert_eq!(ranges(&buf), vec![14..17, 8..11]); // added one stays visible
        buf.search_add();
        assert_eq!(buf.search_step(true), Ok(true)); // wrapped to the first
        buf.search_end(false);
        assert_eq!(ranges(&buf), vec![14..17, 8..11]); // the added ones, newest is main
        assert!(buf.search.is_none());

        // Cancel restores the selections from before `/`, all of them.
        buf.search_begin();
        buf.search_preview("foo".into(), false, false);
        buf.search_end(true);
        assert_eq!(ranges(&buf), vec![14..17, 8..11]);

        // Nothing added: ends on the current match.
        at(&mut buf, 0);
        buf.search_begin();
        buf.search_preview("b".into(), false, false);
        buf.search_end(false);
        assert_eq!(ranges(&buf), vec![6..7]);
    }

    #[test]
    fn tag_object_selects_around_else_ahead_and_grows() {
        let src = "<p>a <b class=\"x\">bo<br/>ld</b> c</p> <i>z</i>";
        let mut buf = Buffer::from_str(src);
        let text = |b: &Buffer| b.rope.slice(b.selections[0].range()).to_string();
        at(&mut buf, src.find("bo").unwrap());
        buf.select_tag_at(ObjectMode::Inner, false);
        assert_eq!(text(&buf), "bo<br/>ld"); // self-closing `<br/>` ignored
        buf.select_tag_at(ObjectMode::Inner, false);
        assert_eq!(text(&buf), "a <b class=\"x\">bo<br/>ld</b> c"); // grows to <p>
        buf.select_tag_at(ObjectMode::Inner, false);
        assert_eq!(text(&buf), "a <b class=\"x\">bo<br/>ld</b> c"); // outermost: stays
        // Between tags: the next one ahead.
        at(&mut buf, src.find("</p> ").unwrap() + 4);
        buf.select_tag_at(ObjectMode::Around, false);
        assert_eq!(text(&buf), "<i>z</i>");
    }

    #[test]
    fn repeating_a_pair_object_grows_outward() {
        let mut buf = Buffer::from_str("((ab) c)");
        at(&mut buf, 2);
        buf.select_pair_at('(', ')', ObjectMode::Inner, false);
        assert_eq!(buf.selections[0].range(), 2..4);
        buf.select_pair_at('(', ')', ObjectMode::Inner, false);
        assert_eq!(buf.selections[0].range(), 1..7);
        // Outermost already selected: stays put.
        buf.select_pair_at('(', ')', ObjectMode::Inner, false);
        assert_eq!(buf.selections[0].range(), 1..7);
    }

    #[test]
    fn select_matching_pair_jumps_to_the_nearest_bracket_and_selects_through_its_match() {
        let mut buf = Buffer::from_str("a (b [c] d) e");
        // Cursor not on a bracket - `m` searches forward for the next one.
        at(&mut buf, 0);
        buf.select_matching_pair(true, false);
        assert_eq!((buf.selections[0].anchor, buf.selections[0].head), (2, 10));

        // Cursor already on a bracket - jumps straight to its match (vim's `%`), nesting respected.
        at(&mut buf, 5);
        buf.select_matching_pair(true, false);
        assert_eq!((buf.selections[0].anchor, buf.selections[0].head), (5, 7));

        // `<a-m>` searches backward.
        at(&mut buf, 9);
        buf.select_matching_pair(false, false);
        assert_eq!((buf.selections[0].anchor, buf.selections[0].head), (5, 7));
    }

    #[test]
    fn merge_overlapping_selections_merges_shared_chars_but_not_mere_touching() {
        let mut buf = Buffer::from_str("abcdef");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 2,
                pref_col: 0,
            }, // "abc"
            Selection {
                anchor: 2,
                head: 4,
                pref_col: 0,
            }, // "cde" - shares index 2 with the first
            Selection {
                anchor: 5,
                head: 5,
                pref_col: 0,
            }, // "f" - immediately after, but shares nothing
        ];

        buf.merge_selections();
        assert_eq!(buf.selections.len(), 2);
        assert_eq!(buf.selections[0].range(), 0..5);
    }

    #[test]
    fn merge_contiguous_selections_also_merges_merely_adjacent_ones() {
        let mut buf = Buffer::from_str("abcdef");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 2,
                pref_col: 0,
            },
            Selection {
                anchor: 3,
                head: 5,
                pref_col: 0,
            }, // starts right where the first ends, no shared char
        ];

        buf.merge_contiguous_selections();
        assert_eq!(buf.selections.len(), 1);
        assert_eq!(buf.selections[0].range(), 0..6);
    }

    #[test]
    fn duplicate_selections_doubles_without_merging() {
        let mut buf = Buffer::from_str("abcdef");
        at(&mut buf, 1);

        buf.duplicate_selections();
        assert_eq!(buf.selections.len(), 2);
        assert_eq!(buf.selections[0], buf.selections[1]);
    }

    #[test]
    fn duplicate_selection_lines_below_matches_columns_and_skips_the_last_line() {
        let mut buf = Buffer::from_str("ab\ncd\nef");
        at(&mut buf, 1); // 'b', line 0

        buf.duplicate_selection_lines(true);
        assert_eq!(buf.selections.len(), 2);
        assert_eq!(buf.rope.char_to_line(buf.selections[1].head), 1);
        assert_eq!(buf.rope.char_to_col(buf.selections[1].head), 1); // same column 'd'

        at(&mut buf, 7); // 'f', last line
        buf.duplicate_selection_lines(true);
        assert_eq!(buf.selections.len(), 1); // nothing below the last line
    }

    fn two_cursors() -> Vec<Selection> {
        vec![
            Selection {
                anchor: 0,
                head: 0,
                pref_col: 0,
            },
            Selection {
                anchor: 2,
                head: 2,
                pref_col: 0,
            },
        ]
    }

    #[test]
    fn clear_main_selection_drops_only_the_first() {
        let mut buf = Buffer::from_str("abc");
        buf.selections = two_cursors();

        buf.clear_main_selection();
        assert_eq!(buf.selections.len(), 1);
        assert_eq!(buf.selections[0].head, 2);
    }

    #[test]
    fn keep_main_selection_drops_everything_else() {
        let mut buf = Buffer::from_str("abc");
        buf.selections = two_cursors();

        buf.keep_main_selection();
        assert_eq!(buf.selections.len(), 1);
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn rotate_main_selection_moves_which_one_is_first() {
        let mut buf = Buffer::from_str("abc");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 0,
                pref_col: 0,
            },
            Selection {
                anchor: 1,
                head: 1,
                pref_col: 0,
            },
            Selection {
                anchor: 2,
                head: 2,
                pref_col: 0,
            },
        ];

        buf.rotate_main_selection(false);
        assert_eq!(buf.selections[0].head, 1);

        buf.rotate_main_selection(true);
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn ensure_forward_selections_flips_only_backward_ones() {
        let mut buf = Buffer::from_str("abcdef");
        buf.selections = vec![
            Selection {
                anchor: 3,
                head: 1,
                pref_col: 0,
            }, // backward
            Selection {
                anchor: 1,
                head: 3,
                pref_col: 0,
            }, // already forward
        ];

        buf.ensure_forward_selections();
        assert!(buf.selections.iter().all(|s| s.head >= s.anchor));
        assert_eq!((buf.selections[0].anchor, buf.selections[0].head), (1, 3));
        assert_eq!((buf.selections[1].anchor, buf.selections[1].head), (1, 3));
    }

    #[test]
    fn rotate_selection_contents_shifts_text_within_independent_groups() {
        let mut buf = Buffer::from_str("1 2 3 4 5 6");
        buf.selections = (0..6)
            .map(|i| Selection {
                anchor: i * 2,
                head: i * 2,
                pref_col: 0,
            })
            .collect();

        buf.rotate_selection_contents(Some(3), false);
        // Group (1,2,3) -> each gets the previous one's text: (3,1,2).
        // Group (4,5,6) -> (6,4,5).
        assert_eq!(buf.rope.to_string(), "3 1 2 6 4 5");
    }

    #[test]
    fn extending_selection_to_end_of_buffer_does_not_panic_on_delete() {
        // `head` can legitimately reach `len_chars()` (the append-at-end
        // caret position) — `Selection::range()`'s `end() + 1` would overshoot
        // the rope's real bounds there if the buffer didn't clamp first.
        let mut buf = Buffer::from_str("abc");
        at(&mut buf, 0);
        buf.move_char(Direction::Right, true);
        buf.move_char(Direction::Right, true);
        buf.move_char(Direction::Right, true); // head now at len_chars() == 3

        let mut registers = RegisterManager::new();
        buf.delete_and_yank(&mut registers, '"');

        assert_eq!(buf.rope.to_string(), "");
    }

    #[test]
    fn cursor_column_tracks_the_last_typed_char_not_the_one_before_it() {
        let mut buf = Buffer::from_str("");
        buf.enter_insert_before();
        buf.insert_char('a');
        buf.insert_char('b');

        let head = buf.selections[0].head;
        assert_eq!(head, buf.rope.len_chars());
        assert_eq!(buf.rope.char_to_col(head), 2);
    }

    #[test]
    fn cursor_column_resets_to_zero_on_the_new_line_after_enter() {
        let mut buf = Buffer::from_str("");
        buf.enter_insert_before();
        buf.insert_char('a');
        buf.insert_char('\n');

        let head = buf.selections[0].head;
        assert_eq!(buf.rope.char_to_line(head), 1);
        assert_eq!(buf.rope.char_to_col(head), 0);
    }

    #[test]
    fn paste_after_inserts_past_selection_end_even_at_buffer_end() {
        // Same end-of-buffer edge as above: `sel.end() + 1` must clamp to
        // `len_chars()` or the insert index overshoots the rope's bounds.
        let mut buf = Buffer::from_str("ab");
        at(&mut buf, 1); // cursor on the last char

        let mut registers = RegisterManager::new();
        registers.set('"', "X".to_string());
        buf.paste_after(&mut registers, '"');

        assert_eq!(buf.rope.to_string(), "abX");
    }

    #[test]
    fn paste_after_linewise_yank_goes_on_its_own_line() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 0); // cursor inside "one"

        let mut registers = RegisterManager::new();
        registers.set('"', "foo\n".to_string());
        buf.paste_after(&mut registers, '"');

        assert_eq!(buf.rope.to_string(), "one\nfoo\ntwo\nthree");
        assert_eq!(buf.get_selected_text(), "foo\n");
    }

    #[test]
    fn paste_after_linewise_yank_on_last_line_without_trailing_newline() {
        let mut buf = Buffer::from_str("one\ntwo");
        at(&mut buf, 5); // cursor inside "two", which has no trailing \n

        let mut registers = RegisterManager::new();
        registers.set('"', "foo\n".to_string());
        buf.paste_after(&mut registers, '"');

        assert_eq!(buf.rope.to_string(), "one\ntwo\nfoo\n");
        assert_eq!(buf.get_selected_text(), "foo\n");
    }

    #[test]
    fn paste_before_inserts_at_selection_start() {
        let mut buf = Buffer::from_str("ab");
        at(&mut buf, 1); // cursor on the second char

        let mut registers = RegisterManager::new();
        registers.set('"', "X".to_string());
        buf.paste_before(&mut registers, '"');

        assert_eq!(buf.rope.to_string(), "aXb");
    }

    #[test]
    fn paste_before_linewise_yank_goes_on_its_own_line_above() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 5); // cursor inside "two"

        let mut registers = RegisterManager::new();
        registers.set('"', "foo\n".to_string());
        buf.paste_before(&mut registers, '"');

        assert_eq!(buf.rope.to_string(), "one\nfoo\ntwo\nthree");
        assert_eq!(buf.get_selected_text(), "foo\n");
    }

    fn select_hello(buf: &mut Buffer) {
        buf.selections = vec![Selection {
            anchor: 0,
            head: 4,
            pref_col: 0,
        }]; // "hello" in "hello world"
    }

    #[test]
    fn append_grows_the_selection_to_cover_the_typed_text() {
        let mut buf = Buffer::from_str("hello world");
        select_hello(&mut buf);
        buf.enter_insert_after(); // `a` - leaves the selection untouched
        assert_eq!(
            buf.selections[0],
            Selection {
                anchor: 0,
                head: 4,
                pref_col: 0
            }
        );

        for ch in "XXX".chars() {
            buf.insert_char(ch);
        }

        assert_eq!(buf.rope.to_string(), "helloXXX world");
        assert_eq!(buf.selections[0].anchor, 0);
        assert_eq!(buf.selections[0].head, 7);
        assert_eq!(buf.get_selected_text(), "helloXXX");
    }

    #[test]
    fn insert_before_shifts_the_selection_without_growing_it() {
        let mut buf = Buffer::from_str("hello world");
        select_hello(&mut buf);
        buf.enter_insert_before(); // `i` - leaves the selection untouched

        for ch in "XXX".chars() {
            buf.insert_char(ch);
        }

        assert_eq!(buf.rope.to_string(), "XXXhello world");
        // `head` is the render-critical field - it must track the advancing
        // insertion point (right where the next char would go), not the far
        // edge, even though the far edge (anchor) is numerically larger here.
        assert_eq!(buf.selections[0].head, 3);
        assert_eq!(buf.selections[0].anchor, 7);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn backspace_during_append_shrinks_the_grown_edge() {
        let mut buf = Buffer::from_str("hello world");
        select_hello(&mut buf);
        buf.enter_insert_after();
        for ch in "XXX".chars() {
            buf.insert_char(ch);
        }

        buf.delete_backward(); // undo one typed char
        assert_eq!(buf.rope.to_string(), "helloXX world");
        assert_eq!(buf.get_selected_text(), "helloXX");

        buf.delete_backward();
        buf.delete_backward();
        // Keeps going past what was typed, into the original selection too.
        buf.delete_backward();
        assert_eq!(buf.rope.to_string(), "hell world");
        assert_eq!(buf.get_selected_text(), "hell");
    }

    #[test]
    fn backspace_during_insert_before_undoes_one_prepended_char() {
        let mut buf = Buffer::from_str("hello world");
        select_hello(&mut buf);
        buf.enter_insert_before();
        for ch in "XXX".chars() {
            buf.insert_char(ch);
        }

        buf.delete_backward();
        assert_eq!(buf.rope.to_string(), "XXhello world");
        assert_eq!(buf.get_selected_text(), "hello");
        assert_eq!(buf.selections[0].head, 2); // cursor sits right after "XX"
    }

    #[test]
    fn insert_before_head_tracks_the_insertion_point_while_typing() {
        // The rendered cursor is always `head` - it must advance through the
        // newly typed text as you type it, not jump to the far edge.
        let mut buf = Buffer::from_str("hello world");
        select_hello(&mut buf);
        buf.enter_insert_before();

        buf.insert_char('X');
        assert_eq!(buf.rope.to_string(), "Xhello world");
        assert_eq!(buf.selections[0].head, 1);

        buf.insert_char('Y');
        assert_eq!(buf.rope.to_string(), "XYhello world");
        assert_eq!(buf.selections[0].head, 2);
    }

    #[test]
    fn delete_no_yank_does_not_touch_registers() {
        let mut buf = Buffer::from_str("abc");
        at(&mut buf, 0);
        buf.move_char(Direction::Right, true); // selects "ab"

        let mut registers = RegisterManager::new();
        registers.set('"', "preexisting".to_string());
        buf.delete_no_yank();

        assert_eq!(buf.rope.to_string(), "c");
        assert_eq!(registers.get('"'), Some("preexisting".to_string()));
    }

    #[test]
    fn replace_char_overwrites_every_selected_char() {
        let mut buf = Buffer::from_str("abc");
        at(&mut buf, 0);
        buf.move_char(Direction::Right, true);
        buf.move_char(Direction::Right, true); // selects "abc"

        buf.replace_char('x');
        assert_eq!(buf.rope.to_string(), "xxx");
    }

    #[test]
    fn case_conversion_transforms_selected_text() {
        let mut buf = Buffer::from_str("Hello");
        at(&mut buf, 0);
        for _ in 0..4 {
            buf.move_char(Direction::Right, true);
        } // selects "Hello"

        buf.to_uppercase();
        assert_eq!(buf.rope.to_string(), "HELLO");

        buf.to_lowercase();
        assert_eq!(buf.rope.to_string(), "hello");

        buf.swap_case();
        assert_eq!(buf.rope.to_string(), "HELLO");
    }

    #[test]
    fn join_lines_replaces_newline_and_indentation_with_a_space() {
        let mut buf = Buffer::from_str("one\n    two\nthree");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 7, // spans "one\n   " into "two"
            pref_col: 0,
        }];

        buf.join_lines();
        assert_eq!(buf.rope.to_string(), "one two\nthree");
    }

    #[test]
    fn join_lines_select_spaces_selects_only_the_inserted_spaces() {
        let mut buf = Buffer::from_str("one\ntwo");
        at(&mut buf, 0);
        buf.move_char(Direction::Right, true);
        buf.move_char(Direction::Right, true); // cursor/selection on "one"
        buf.select_line(); // "one\n" - trailing newline is the join point

        buf.join_lines_select_spaces();
        assert_eq!(buf.rope.to_string(), "one two");
        assert_eq!(buf.get_selected_text(), " ");
    }

    #[test]
    fn indent_lines_skips_empty_lines_unless_including_empty() {
        let mut buf = Buffer::from_str("one\n\ntwo");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 7,
            pref_col: 0,
        }]; // spans all three lines

        buf.indent_lines(false);
        assert_eq!(buf.rope.to_string(), "    one\n\n    two");

        buf.undo();
        buf.indent_lines(true);
        assert_eq!(buf.rope.to_string(), "    one\n    \n    two");
    }

    #[test]
    fn unindent_lines_removes_up_to_one_indent_unit() {
        let mut buf = Buffer::from_str("      two"); // 6 leading spaces
        at(&mut buf, 0);

        buf.unindent_lines(false);
        assert_eq!(buf.rope.to_string(), "  two"); // removed a full 4-space unit

        buf.unindent_lines(true); // only 2 spaces left, less than one unit
        assert_eq!(buf.rope.to_string(), "  two"); // preserve_incomplete leaves it
    }

    #[test]
    fn replace_with_yanked_overwrites_every_selection_with_the_register() {
        let mut buf = Buffer::from_str("abc def");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 2,
                pref_col: 0,
            },
            Selection {
                anchor: 4,
                head: 6,
                pref_col: 0,
            },
        ];

        let mut registers = RegisterManager::new();
        registers.set('"', "X".to_string());
        buf.replace_with_yanked(&mut registers, '"');

        assert_eq!(buf.rope.to_string(), "X X");
    }

    #[test]
    fn change_and_yank_dispatch_deletes_selection_and_enters_insert() {
        let mut buf = Buffer::from_str("abc");
        at(&mut buf, 0);
        buf.move_char(Direction::Right, true);
        buf.move_char(Direction::Right, true); // selects "abc"

        let mut registers = RegisterManager::new();
        buf.delete_and_yank(&mut registers, '"');
        buf.enter_insert_before();

        assert_eq!(buf.rope.to_string(), "");
        assert_eq!(registers.get('"'), Some("abc".to_string()));
        buf.insert_char('x');
        assert_eq!(buf.rope.to_string(), "x");
    }

    #[test]
    fn align_cursors_pads_shorter_selections_to_the_widest_column() {
        let mut buf = Buffer::from_str("a\nbb\nccc");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 0,
                pref_col: 0,
            }, // col 0 on "a"
            Selection {
                anchor: 2,
                head: 2,
                pref_col: 0,
            }, // col 0 on "bb" (line 2 starts at char 2)
            Selection {
                anchor: 5,
                head: 5,
                pref_col: 0,
            }, // col 0 on "ccc"
        ];
        // Move each selection's cursor onto its line to vary the starting column.
        buf.selections[1].anchor = 3; // col 1 of "bb"
        buf.selections[1].head = 3;
        buf.selections[2].anchor = 7; // col 2 of "ccc"
        buf.selections[2].head = 7;

        buf.align_cursors();
        assert_eq!(buf.rope.to_string(), "  a\nb b\nccc");
        // every cursor should now sit on column 2, the widest starting column
        for sel in &buf.selections {
            assert_eq!(buf.rope.char_to_col(sel.head), 2);
        }
    }

    #[test]
    fn copy_indentation_applies_main_selections_indent_to_others() {
        let mut buf = Buffer::from_str("    one\ntwo\n  three");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 0,
                pref_col: 0,
            }, // main: "    one" (4 spaces)
            Selection {
                anchor: 8,
                head: 8,
                pref_col: 0,
            }, // "two"
            Selection {
                anchor: 12,
                head: 12,
                pref_col: 0,
            }, // "  three"
        ];

        buf.copy_indentation(None);
        assert_eq!(buf.rope.to_string(), "    one\n    two\n    three");
    }

    #[test]
    fn tabs_to_spaces_expands_to_the_next_tabstop_column() {
        let mut buf = Buffer::from_str("a\tb");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 2,
            pref_col: 0,
        }];

        buf.tabs_to_spaces(Some(4));
        assert_eq!(buf.rope.to_string(), "a   b");
    }

    #[test]
    fn spaces_to_tabs_collapses_aligned_runs_into_tabs() {
        let mut buf = Buffer::from_str("        two"); // 8 leading spaces
        at(&mut buf, 0);
        buf.move_char(Direction::Right, true);
        for _ in 0..7 {
            buf.move_char(Direction::Right, true);
        } // selects all 8 leading spaces

        buf.spaces_to_tabs(Some(4));
        assert_eq!(buf.rope.to_string(), "\t\ttwo");
    }

    #[test]
    fn insert_register_inserts_raw_text_not_linewise() {
        // Unlike `paste_after`, a trailing `\n` in the register must NOT
        // trigger the own-line linewise behavior - it's a literal <c-r>
        // insert of whatever text is in the register.
        let mut buf = Buffer::from_str("ab");
        at(&mut buf, 0);
        buf.enter_insert_before();

        let mut registers = RegisterManager::new();
        registers.set('"', "X\n".to_string());
        buf.insert_register(&mut registers, '"');

        assert_eq!(buf.rope.to_string(), "X\nab");
    }

    #[test]
    fn insert_typed_char_autoindents_a_new_line_to_match_the_previous_one() {
        let mut buf = Buffer::from_str("    one");
        at(&mut buf, 7); // end of "one"
        buf.enter_insert_before();
        buf.autoindent = true;

        buf.insert_typed_char('\n');
        assert_eq!(buf.rope.to_string(), "    one\n    ");

        buf.insert_typed_char('x');
        assert_eq!(buf.rope.to_string(), "    one\n    x");
    }

    #[test]
    fn insert_typed_char_does_not_autoindent_when_disabled() {
        let mut buf = Buffer::from_str("    one");
        at(&mut buf, 7);
        buf.enter_insert_before();
        buf.autoindent = false;

        buf.insert_typed_char('\n');
        assert_eq!(buf.rope.to_string(), "    one\n");
    }

    #[test]
    fn insert_typed_char_autopairs_brackets_and_quotes_with_cursor_between() {
        let mut buf = Buffer::from_str("");
        at(&mut buf, 0);
        buf.enter_insert_before();
        buf.autopair = true;

        buf.insert_typed_char('(');
        assert_eq!(buf.rope.to_string(), "()");
        assert_eq!(buf.selections[0].head, 1);
        assert_eq!(buf.selections[0].anchor, 1);

        buf.insert_typed_char('x');
        assert_eq!(buf.rope.to_string(), "(x)");
    }

    #[test]
    fn insert_typed_char_does_not_autopair_when_disabled() {
        let mut buf = Buffer::from_str("");
        at(&mut buf, 0);
        buf.enter_insert_before();
        buf.autopair = false;

        buf.insert_typed_char('(');
        assert_eq!(buf.rope.to_string(), "(");
    }

    #[test]
    fn commit_undo_group_stops_the_next_edit_coalescing_into_the_last() {
        let mut buf = Buffer::from_str("");
        at(&mut buf, 0);
        buf.enter_insert_before();

        buf.insert_char('X');
        buf.commit_undo_group();
        buf.insert_char('Y');

        assert_eq!(buf.rope.to_string(), "XY");
        assert_eq!(buf.history.undo_stack.len(), 2);

        buf.undo();
        assert_eq!(buf.rope.to_string(), "X"); // only "Y" was undone
    }

    #[test]
    fn open_line_below_inserts_a_blank_line_and_lands_the_cursor_on_it() {
        let mut buf = Buffer::from_str("one\ntwo");
        at(&mut buf, 1); // middle of "one"

        buf.open_line_below();

        assert_eq!(buf.rope.to_string(), "one\n\ntwo");
        assert_eq!(buf.selections[0].head, 4); // on the new, empty line
        assert_eq!(buf.selections[0].anchor, 4);

        buf.enter_insert_before();
        buf.insert_char('X');
        assert_eq!(buf.rope.to_string(), "one\nX\ntwo"); // typed onto that line
    }

    #[test]
    fn open_line_above_inserts_a_blank_line_before_the_current_one() {
        let mut buf = Buffer::from_str("one\ntwo");
        at(&mut buf, 5); // middle of "two"

        buf.open_line_above();

        assert_eq!(buf.rope.to_string(), "one\n\ntwo");
        assert_eq!(buf.selections[0].head, 4); // on the new, empty line
        assert_eq!(buf.selections[0].anchor, 4);

        buf.enter_insert_before();
        buf.insert_char('X');
        assert_eq!(buf.rope.to_string(), "one\nX\ntwo"); // typed onto that line, "two" pushed down
    }

    #[test]
    fn open_line_above_on_the_first_line_pushes_everything_down() {
        let mut buf = Buffer::from_str("one");
        at(&mut buf, 0);

        buf.open_line_above();

        assert_eq!(buf.rope.to_string(), "\none");
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn enter_insert_line_end_appends_past_the_last_char_not_onto_the_next_line() {
        // Regression guard for the landing-mode pitfall called out next to
        // the `EnterInsertLineEnd` dispatch arm - using `enter_insert_after`
        // here would land one column into "two" instead of extending "one".
        let mut buf = Buffer::from_str("one\ntwo");
        at(&mut buf, 0);

        buf.move_in_line(LineMovement::LineEnd, false);
        buf.enter_insert_before();
        buf.insert_char('X');

        assert_eq!(buf.rope.to_string(), "oneX\ntwo");
    }

    #[test]
    fn home_and_end_move_to_buffer_boundaries() {
        let mut buf = Buffer::from_str("hello\nworld");
        at(&mut buf, 3);

        buf.move_in_buffer(BufferMovement::End, false);
        assert_eq!(buf.selections[0].head, buf.rope.buffer_range().end);

        buf.move_in_buffer(BufferMovement::Home, false);
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn goto_last_line_lands_on_its_first_char_not_the_buffer_end() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 0);

        buf.goto_last_line(false);
        assert_eq!(buf.selections[0].head, 8); // 't' of "three"
    }

    #[test]
    fn goto_line_is_one_indexed_and_clamps_past_the_end() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 0);

        buf.goto_line(2, false);
        assert_eq!(buf.selections[0].head, 4); // 't' of "two"

        buf.goto_line(99, false);
        assert_eq!(buf.selections[0].head, 8); // clamped to the last line
    }

    #[test]
    fn goto_last_modification_position_returns_to_the_last_edit() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 5); // inside "two"
        buf.enter_insert_before();
        buf.insert_char('X');
        let edit_head = buf.selections[0].head;

        buf.goto_last_line(false);
        assert_ne!(buf.selections[0].head, edit_head);

        buf.goto_last_modification_position(false);
        assert_eq!(buf.selections[0].head, edit_head);
    }

    #[test]
    fn goto_last_modification_position_is_a_no_op_with_no_history() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 5);

        buf.goto_last_modification_position(false);
        assert_eq!(buf.selections[0].head, 5);
    }

    fn search(kind: FindKind, direction: FindDirection, target: char) -> CharSearch {
        CharSearch {
            kind,
            direction,
            target,
        }
    }

    #[test]
    fn f_lands_on_the_target_char() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 5); // 't' of "the"
    }

    #[test]
    fn t_lands_just_short_of_the_target_char() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::Until, FindDirection::Forward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 4); // the space right before "the"
    }

    #[test]
    fn capital_f_searches_backward_landing_on_target() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 13); // the 'e' of "target", one before its trailing 't'
        buf.find_char(
            search(FindKind::To, FindDirection::Backward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 9); // the 't' that starts "target"
    }

    #[test]
    fn capital_t_searches_backward_landing_just_short_of_target() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 13); // the 'e' of "target", one before its trailing 't'
        buf.find_char(
            search(FindKind::Until, FindDirection::Backward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 10); // right after that 't'
    }

    #[test]
    fn count_skips_to_the_nth_occurrence() {
        let mut buf = Buffer::from_str("a-b-c-d");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, '-'),
            Some(3),
            false,
        );
        assert_eq!(buf.selections[0].head, 5); // the 3rd '-'
    }

    #[test]
    fn missing_target_leaves_selection_untouched() {
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, 'z'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn non_extending_search_selects_from_the_old_head_to_the_target() {
        // Like `w`/`b`/`e`, a non-extending search still selects the text
        // it swept over - it doesn't collapse to a bare cursor on the
        // target the way plain h/j/k/l movement would.
        let mut buf = Buffer::from_str("find the target");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 2,
            pref_col: 0,
        }];
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, 't'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].anchor, 2); // the old head
        assert_eq!(buf.selections[0].head, 5);
    }

    #[test]
    fn extending_search_keeps_the_anchor() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, 't'),
            None,
            true,
        );
        assert_eq!(buf.selections[0].anchor, 0);
        assert_eq!(buf.selections[0].head, 5);
    }

    #[test]
    fn repeat_reuses_the_last_search() {
        let mut buf = Buffer::from_str("a-b-c-d");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::To, FindDirection::Forward, '-'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 1);
        buf.repeat_char_search(false);
        assert_eq!(buf.selections[0].head, 3);
        buf.repeat_char_search(false);
        assert_eq!(buf.selections[0].head, 5);
    }

    #[test]
    fn repeat_with_no_prior_search_is_a_no_op() {
        let mut buf = Buffer::from_str("find the target");
        at(&mut buf, 0);
        buf.repeat_char_search(false);
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn repeating_until_skips_past_the_char_it_already_stopped_before() {
        // `t-` from the start stops right before the first '-'; repeating
        // it naively would re-find that same '-' and not move at all, so
        // the repeat has to step past it first (matching vim/kakoune).
        let mut buf = Buffer::from_str("a-b-c-d");
        at(&mut buf, 0);
        buf.find_char(
            search(FindKind::Until, FindDirection::Forward, '-'),
            None,
            false,
        );
        assert_eq!(buf.selections[0].head, 0); // already right before the 1st '-'
        buf.repeat_char_search(false);
        assert_eq!(buf.selections[0].head, 2); // right before the 2nd '-'
    }

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
    fn w_from_whitespace_selects_the_whole_next_word() {
        // Whitespace isn't a word: starting on the separator skips it
        // entirely and selects the full next word, nothing before it.
        let mut buf = Buffer::from_str("ciao come");
        at(&mut buf, 4); // the space between the two words
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "come");
    }

    #[test]
    fn w_from_word_start_selects_word_plus_trailing_separator() {
        // Starting inside a word keeps the selection from the cursor's
        // position through the trailing separator, stopping just before the
        // next word starts (not entering it).
        let mut buf = Buffer::from_str("hello world hi");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello ");
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "world ");
    }

    #[test]
    fn b_from_next_word_start_selects_prev_word_plus_leading_separator() {
        // Starting right on the next word's first char jumps fully back:
        // the gap between the two words is reclaimed as "preceding
        // whitespace", but the char the cursor started on is not pulled in.
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 6); // 'w' of "world"
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello ");
    }

    #[test]
    fn b_mid_token_finishes_the_token_only() {
        // Still mid-token: finish backward to this word's own start, no
        // separator involved since no gap was crossed.
        let mut buf = Buffer::from_str(" hello world hi");
        at(&mut buf, 9); // 'r', middle of "world"
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "wor");
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello ");
    }

    #[test]
    fn b_three_presses_walks_use_rpc_colon_colon_client_backward() {
        let mut buf = Buffer::from_str("use rpc::client");
        at(&mut buf, 14); // 't', last char of "client"
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "client");
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "::");
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "rpc");
    }

    #[test]
    fn w_does_not_absorb_the_trailing_newline() {
        let mut buf = Buffer::from_str("hello\nworld");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn e_does_not_absorb_a_newline_crossed_while_jumping() {
        let mut buf = Buffer::from_str("hi\nworld");
        at(&mut buf, 1); // last char of "hi"
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "world");
    }

    #[test]
    fn e_keeps_leading_indentation_on_the_destination_line() {
        let mut buf = Buffer::from_str("hi\n  world");
        at(&mut buf, 1); // last char of "hi"
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "  world");
    }

    #[test]
    fn b_does_not_absorb_a_newline_crossed_while_jumping() {
        let mut buf = Buffer::from_str("hello\nworld");
        at(&mut buf, 6); // 'w' of "world"
        buf.move_word(WordMovement::PrevStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn e_from_word_start_finishes_current_word_only() {
        // Mid/at the start of a word: `e` just lands on this word's own end,
        // no separator is touched since nothing was jumped over.
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "hello");
    }

    #[test]
    fn e_from_word_end_jumps_to_next_word_end_with_leading_separator() {
        // Already at the current word's last char: jump the gap (captured
        // as "preceding whitespace") and land on the next word's last char.
        let mut buf = Buffer::from_str("hello world hi");
        at(&mut buf, 4); // last 'o' of "hello"
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), " world");
    }

    #[test]
    fn e_from_whitespace_selects_through_next_words_end() {
        let mut buf = Buffer::from_str("ciao come");
        at(&mut buf, 4); // the space between the two words
        buf.move_word(WordMovement::NextEnd, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "come");
    }

    #[test]
    fn w_from_last_char_of_token_adjacent_to_punctuation_enters_next_cleanly() {
        // "rpc" is directly followed by "::" with no whitespace in between.
        // At the LAST char of "rpc" there's nothing left of it to extend
        // through, so `w` jumps fully into "::" (anchor included) rather
        // than dragging the already-finished 'c' along as "c::".
        let mut buf = Buffer::from_str("rpc::client");
        at(&mut buf, 2); // last 'c' of "rpc"
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "::");
    }

    #[test]
    fn w_mid_token_adjacent_to_punctuation_finishes_the_token_only() {
        // Still mid-token (not yet at its last char): finish "rpc" itself,
        // but don't cross into "::" in the same press.
        let mut buf = Buffer::from_str("rpc::client");
        at(&mut buf, 1); // 'p', middle of "rpc"
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "pc");
    }

    #[test]
    fn w_four_presses_walks_use_rpc_colon_colon_client() {
        let mut buf = Buffer::from_str("use rpc::client");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "use ");
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "rpc");
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "::");
        buf.move_word(WordMovement::NextStart, WordKind::Word, false);
        assert_eq!(buf.get_selected_text(), "client");
    }

    #[test]
    fn shift_w_extends_head_without_moving_anchor() {
        let mut buf = Buffer::from_str("hello world next");
        at(&mut buf, 0);
        buf.move_word(WordMovement::NextStart, WordKind::Word, false); // "hello "
        buf.move_word(WordMovement::NextStart, WordKind::Word, true); // extend
        assert_eq!(buf.get_selected_text(), "hello world "); // anchor stayed at 0
    }

    #[test]
    fn object_selection_selects_the_whole_word() {
        // Regression test: `apply_range` stores `head` as the last included
        // character, which silently under-selected by one char before
        // `Selection::range()` became an inclusive, closed interval.
        let mut buf = Buffer::from_str("hello world");
        at(&mut buf, 0);
        buf.select_word_at(WordKind::Word, ObjectMode::Inner, false);
        assert_eq!(buf.get_selected_text(), "hello");
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

    #[test]
    fn line_end_then_line_home_moves_to_line_boundaries() {
        let mut buf = Buffer::from_str("hello world\nnext line");
        at(&mut buf, 3); // middle of "hello", on the first line
        buf.move_in_line(LineMovement::LineEnd, false);
        assert_eq!(buf.selections[0].head, 11); // end of "hello world"

        buf.move_in_line(LineMovement::LineHome, false);
        assert_eq!(buf.selections[0].head, 0);
    }

    #[test]
    fn line_home_no_whitespace_skips_leading_indent() {
        let mut buf = Buffer::from_str("    indented");
        at(&mut buf, 10);
        buf.move_in_line(LineMovement::LineHomeNoWhitespace, false);
        assert_eq!(buf.selections[0].head, 4);
    }

    #[test]
    fn select_line_first_press_selects_enclosing_line() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 5); // 'w' of "two"
        buf.select_line();
        assert_eq!(buf.get_selected_text(), "two\n");
    }

    #[test]
    fn select_line_consecutive_presses_select_lines_below() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        at(&mut buf, 5); // 'w' of "two"
        buf.select_line(); // "two\n"
        buf.select_line(); // extends down to "three"
        assert_eq!(buf.get_selected_text(), "two\nthree");
    }

    #[test]
    fn select_line_expands_partial_multi_line_selection() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        buf.selections = vec![Selection {
            anchor: 1, // inside "one"
            head: 5,   // inside "two"
            pref_col: 0,
        }];
        buf.select_line();
        assert_eq!(buf.get_selected_text(), "one\ntwo\n");
    }

    #[test]
    fn select_line_on_last_line_has_no_trailing_newline_and_stops_extending() {
        let mut buf = Buffer::from_str("one\ntwo");
        at(&mut buf, 5); // 'w' of "two"
        buf.select_line();
        assert_eq!(buf.get_selected_text(), "two");
        buf.select_line(); // nothing below to extend to
        assert_eq!(buf.get_selected_text(), "two");
    }
}
