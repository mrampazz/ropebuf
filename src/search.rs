use crate::{buffer::Buffer, selection::Selection};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::register::RegisterManager;

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
