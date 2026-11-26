use regex::Regex;

use crate::{
    buffer::Buffer,
    register::RegisterManager,
    rope::{ObjectMode, WordKind, is_word},
    selection::Selection,
    utils::clamp_range,
};

/// Counts chars in `text[..byte_idx]` to turn a regex match's byte offset
/// (regex operates on `&str`) into a char offset within that same `text`.
fn char_offset(text: &str, byte_idx: usize) -> usize {
    text[..byte_idx].chars().count()
}

impl Buffer {
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
    fn match_main_pattern(&mut self, pat: &str, forward: bool, add: bool) -> Result<bool, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(buf: &mut Buffer, pos: usize) {
        buf.selections = vec![Selection {
            anchor: pos,
            head: pos,
            pref_col: 0,
        }];
    }

    #[test]
    fn search_next_selects_the_match_and_stores_the_pattern() {
        let mut buf = Buffer::from_str("foo bar foo baz");
        at(&mut buf, 0);
        let mut registers = RegisterManager::new();

        buf.search_next(&mut registers, Some("foo".to_string()), '/', false)
            .unwrap();
        // Starting cursor is inside the first "foo"; next match after it is
        // the second one.
        assert_eq!(buf.get_selected_text(), "foo");
        assert_eq!(buf.selections[0].start(), 8);
        assert_eq!(registers.get('/'), Some("foo".to_string()));

        // Repeat with no pattern - reuses the stored one, wraps around.
        buf.search_next(&mut registers, None, '/', false).unwrap();
        assert_eq!(buf.selections[0].start(), 0);
    }

    #[test]
    fn search_previous_wraps_to_the_last_match() {
        let mut buf = Buffer::from_str("foo bar foo baz");
        at(&mut buf, 0);
        let mut registers = RegisterManager::new();

        buf.search_previous(&mut registers, Some("foo".to_string()), '/', false)
            .unwrap();
        assert_eq!(buf.selections[0].start(), 8);
    }

    #[test]
    fn search_next_extend_keeps_the_anchor() {
        let mut buf = Buffer::from_str("foo bar foo baz");
        at(&mut buf, 0);
        let mut registers = RegisterManager::new();

        buf.search_next(&mut registers, Some("foo".to_string()), '/', true)
            .unwrap();
        assert_eq!(buf.selections[0].anchor, 0);
        assert_eq!(buf.get_selected_text(), "foo bar foo");
    }

    #[test]
    fn search_next_errors_on_invalid_regex() {
        let mut buf = Buffer::from_str("foo");
        let mut registers = RegisterManager::new();
        assert!(
            buf.search_next(&mut registers, Some("(".to_string()), '/', false)
                .is_err()
        );
    }

    #[test]
    fn set_search_pattern_from_main_selection_escapes_and_bounds() {
        let mut buf = Buffer::from_str("a.b a.b");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 2,
            pref_col: 0,
        }]; // "a.b"
        let mut registers = RegisterManager::new();

        buf.set_search_pattern_from_main_selection(&mut registers, true, '/');
        assert_eq!(registers.get('/'), Some(r"\ba\.b\b".to_string()));
    }

    #[test]
    fn select_next_match_main_only_moves_the_first_selection() {
        let mut buf = Buffer::from_str("foo bar foo baz");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 0,
                pref_col: 0,
            },
            Selection {
                anchor: 12,
                head: 12,
                pref_col: 0,
            },
        ];
        let mut registers = RegisterManager::new();
        registers.set('/', "foo".to_string());

        buf.select_next_match_main(&mut registers, '/').unwrap();
        assert_eq!(buf.selections.len(), 2);
        assert_eq!(buf.selections[0].start(), 8);
    }

    #[test]
    fn add_next_match_selection_appends_instead_of_replacing() {
        let mut buf = Buffer::from_str("foo bar foo baz");
        at(&mut buf, 0);
        let mut registers = RegisterManager::new();
        registers.set('/', "foo".to_string());

        buf.add_next_match_selection(&mut registers, '/').unwrap();
        assert_eq!(buf.selections.len(), 2);
    }

    #[test]
    fn select_regex_matches_fans_out_one_selection_per_match() {
        let mut buf = Buffer::from_str("a1 b2 c3");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 7,
            pref_col: 0,
        }]; // whole buffer

        buf.select_regex_matches(r"[a-z]\d", None).unwrap();
        assert_eq!(buf.selections.len(), 3);
        assert_eq!(buf.get_selected_text(), "a1b2c3");
    }

    #[test]
    fn select_regex_matches_uses_the_given_capture_group() {
        let mut buf = Buffer::from_str("a1 b2 c3");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 7,
            pref_col: 0,
        }];

        buf.select_regex_matches(r"([a-z])(\d)", Some(2)).unwrap();
        assert_eq!(buf.get_selected_text(), "123");
    }

    #[test]
    fn split_selections_regex_selects_the_gaps() {
        let mut buf = Buffer::from_str("one,two,three");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 12,
            pref_col: 0,
        }];

        buf.split_selections_regex(",", None).unwrap();
        assert_eq!(buf.selections.len(), 3);
        assert_eq!(buf.get_selected_text(), "onetwothree");
    }

    #[test]
    fn split_selections_lines_fans_out_one_selection_per_line() {
        let mut buf = Buffer::from_str("one\ntwo\nthree");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 12,
            pref_col: 0,
        }]; // whole buffer

        buf.split_selections_lines();
        assert_eq!(buf.selections.len(), 3);
        assert_eq!(buf.selections[0].start(), 0);
        assert_eq!(buf.selections[1].start(), 4);
        assert_eq!(buf.selections[2].start(), 8);
    }

    #[test]
    fn select_endpoints_keeps_only_the_first_and_last_char_of_each_selection() {
        let mut buf = Buffer::from_str("abcdef");
        buf.selections = vec![Selection {
            anchor: 1,
            head: 4,
            pref_col: 0,
        }];

        buf.select_endpoints();
        assert_eq!(buf.selections.len(), 2);
        assert_eq!(buf.get_selected_text(), "be");
    }

    #[test]
    fn select_endpoints_keeps_a_single_char_selection_as_one_cursor() {
        let mut buf = Buffer::from_str("abc");
        at(&mut buf, 1);

        buf.select_endpoints();
        assert_eq!(buf.selections.len(), 1);
    }

    #[test]
    fn unselect_whitespace_trims_edges_and_drops_whitespace_only_selections() {
        let mut buf = Buffer::from_str("  ab  cd  ");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 3,
                pref_col: 0,
            }, // "  ab" -> "ab"
            Selection {
                anchor: 4,
                head: 5,
                pref_col: 0,
            }, // "  " -> dropped
        ];

        buf.unselect_whitespace().unwrap();
        assert_eq!(buf.selections.len(), 1);
        assert_eq!(buf.get_selected_text(), "ab");
    }

    #[test]
    fn unselect_whitespace_errors_when_every_selection_is_whitespace_only() {
        let mut buf = Buffer::from_str("   ");
        buf.selections = vec![Selection {
            anchor: 0,
            head: 2,
            pref_col: 0,
        }];

        assert!(buf.unselect_whitespace().is_err());
    }

    #[test]
    fn keep_matching_selections_drops_non_matching_ones() {
        let mut buf = Buffer::from_str("foo bar baz");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 2,
                pref_col: 0,
            }, // "foo"
            Selection {
                anchor: 4,
                head: 6,
                pref_col: 0,
            }, // "bar"
        ];

        buf.keep_matching_selections("^f").unwrap();
        assert_eq!(buf.selections.len(), 1);
        assert_eq!(buf.get_selected_text(), "foo");
    }

    #[test]
    fn clear_matching_selections_drops_matching_ones() {
        let mut buf = Buffer::from_str("foo bar baz");
        buf.selections = vec![
            Selection {
                anchor: 0,
                head: 2,
                pref_col: 0,
            }, // "foo"
            Selection {
                anchor: 4,
                head: 6,
                pref_col: 0,
            }, // "bar"
        ];

        buf.clear_matching_selections("^f").unwrap();
        assert_eq!(buf.selections.len(), 1);
        assert_eq!(buf.get_selected_text(), "bar");
    }

    #[test]
    fn select_all_occurrences_takes_whole_words_and_keeps_the_main_first() {
        let mut buf = Buffer::from_str("ab abc ab\n");
        buf.selections = vec![Selection {
            anchor: 7,
            head: 7,
            pref_col: 0,
        }];
        buf.select_all_occurrences().unwrap();
        let picked: Vec<String> = buf
            .selections
            .iter()
            .map(|s| buf.rope.slice(s.start()..s.end() + 1).to_string())
            .collect();
        // `abc` isn't a whole-word `ab`; the one under the cursor comes first.
        assert_eq!(picked, ["ab", "ab"]);
        assert_eq!(buf.selections[0].start(), 7);
    }
}

/// State of a live `/` search: where it started (restored on cancel), the
/// current pattern, and the matches picked with `a` so far.
#[derive(Debug, Clone, Default)]
pub struct SearchSession {
    origin: Vec<Selection>,
    pattern: String,
    added: Vec<Selection>,
    /// Kakoune's `?`: the origin selection grows to the match instead of becoming it.
    extend: bool,
}

impl Buffer {
    pub fn search_begin(&mut self) {
        self.search = Some(SearchSession {
            origin: self.selections.clone(),
            ..Default::default()
        });
    }

    /// While typing: jumps from the origin cursor to the first match after it
    /// (`backward`: before it) - or, `extend`, grows the origin selection to it.
    /// An empty, invalid or matchless pattern just sits at the origin.
    pub fn search_preview(&mut self, pattern: String, backward: bool, extend: bool) {
        let Some(session) = &mut self.search else {
            return;
        };
        (session.pattern, session.extend) = (pattern.clone(), extend);
        self.selections = vec![session.origin[0]];
        if !pattern.is_empty() && self.match_main_pattern(&pattern, !backward, false).is_ok() {
            self.extend_from_origin(!backward);
        }
    }

    /// In an extending session: the main selection (a match) becomes the origin selection grown
    /// to it - its head at the match's end going forward, at its start going backward.
    fn extend_from_origin(&mut self, forward: bool) {
        let (Some(session), Some(main)) = (&self.search, self.selections.first().copied()) else {
            return;
        };
        if !session.extend {
            return;
        }
        let head = if forward { main.end() } else { main.start() };
        let anchor = session.origin[0].anchor;
        self.selections[0] = Selection {
            anchor,
            head,
            pref_col: self.rope.char_to_col(head),
        };
    }

    /// `n`/`N` in search navigation: moves the current match, keeping the
    /// added ones shown. Ok(true) if it wrapped.
    pub fn search_step(&mut self, forward: bool) -> Result<bool, String> {
        let Some(session) = &self.search else {
            return Err("no active search".to_string());
        };
        let (pattern, added) = (session.pattern.clone(), session.added.clone());
        self.selections.truncate(1);
        let wrapped = self.match_main_pattern(&pattern, forward, false)?;
        self.extend_from_origin(forward);
        self.selections.extend(added);
        self.merge_selections();
        Ok(wrapped)
    }

    /// `a` in search navigation: keeps the current match.
    pub fn search_add(&mut self) {
        let (Some(session), Some(&main)) = (&mut self.search, self.selections.first()) else {
            return;
        };
        if !session.added.contains(&main) {
            session.added.push(main);
        }
    }

    /// Ends the session: `cancel` restores the selections from before `/`;
    /// otherwise the added matches (newest as main), or the current match if
    /// none were added. A no-op with no session.
    pub fn search_end(&mut self, cancel: bool) {
        let Some(mut session) = self.search.take() else {
            return;
        };
        if cancel {
            self.selections = session.origin;
        } else if let Some(newest) = session.added.pop() {
            session.added.insert(0, newest);
            self.selections = session.added;
            self.merge_selections();
        } else {
            self.selections.truncate(1);
        }
    }
}

#[cfg(test)]
mod direction_tests {
    use crate::buffer::Buffer;
    use crate::selection::Selection;

    fn sel(buf: &Buffer) -> (usize, usize) {
        (buf.selections[0].anchor, buf.selections[0].head)
    }

    #[test]
    fn previews_go_backward_and_extend_from_where_the_search_began() {
        // "ab x ab x ab": matches at 0..2, 5..7, 10..12; start on the middle `x` (8).
        let start = |buf: &mut Buffer| {
            buf.selections = vec![Selection {
                anchor: 8,
                head: 8,
                pref_col: 8,
            }];
            buf.search_begin();
        };
        let mut buf = Buffer::from_str("ab x ab x ab");
        start(&mut buf);
        buf.search_preview("ab".into(), false, false);
        assert_eq!(sel(&buf), (10, 11), "forward: the next match");
        buf.search_preview("ab".into(), true, false);
        assert_eq!(sel(&buf), (5, 6), "backward: the previous one");
        // Extending: the anchor stays where the search began, the head reaches the match.
        buf.search_preview("ab".into(), false, true);
        assert_eq!(sel(&buf), (8, 11));
        buf.search_preview("ab".into(), true, true);
        assert_eq!(sel(&buf), (8, 5));
        // Stepping keeps extending from the same anchor.
        buf.search_step(false).unwrap();
        assert_eq!(sel(&buf), (8, 0));
        buf.search_end(false);
        assert_eq!(sel(&buf), (8, 0));
        // An extending search cancelled puts the selection back.
        start(&mut buf);
        buf.search_preview("ab".into(), false, true);
        buf.search_end(true);
        assert_eq!(sel(&buf), (8, 8));
    }
}
