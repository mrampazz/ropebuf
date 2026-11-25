use std::ops::Range;

#[derive(Clone, Debug, Default, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
    pub pref_col: usize,
}

impl Selection {
    pub fn cursor(&self) -> usize {
        self.head
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..(self.end() + 1)
    }

    pub fn contains(&self, char_idx: usize) -> bool {
        let r: Range<usize> = self.range();
        char_idx >= r.start && char_idx < r.end
    }

    pub fn normalized(&self) -> (usize, usize) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn collapse_to_head(&mut self) {
        self.anchor = self.head;
    }

    pub fn collapse_to_anchor(&mut self) {
        self.head = self.anchor;
    }

    pub fn extend(&mut self, anchor: usize) {
        self.anchor = anchor;
    }

    pub fn is_forward(&self) -> bool {
        self.anchor <= self.head
    }

    pub fn apply_range(&mut self, start: usize, end_exclusive: usize, extend: bool) {
        let end = end_exclusive.saturating_sub(1).max(start);
        if start < end_exclusive {
            if extend {
                if self.head >= self.anchor {
                    self.head = end.max(self.head);
                } else {
                    self.head = start.min(self.head);
                }
            } else {
                self.anchor = start;
                self.head = end;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_cursor_is_a_one_char_selection() {
        let sel = Selection {
            anchor: 5,
            head: 5,
            pref_col: 0,
        };
        assert_eq!(sel.range(), 5..6);
        assert!(sel.contains(5));
        assert!(!sel.contains(6));
    }

    #[test]
    fn forward_extended_selection_includes_both_endpoints() {
        let sel = Selection {
            anchor: 3,
            head: 5,
            pref_col: 0,
        };
        assert_eq!(sel.range(), 3..6);
        assert!(sel.contains(3)); // anchor
        assert!(sel.contains(5)); // head
        assert!(!sel.contains(6));
    }

    #[test]
    fn backward_extended_selection_includes_both_endpoints() {
        let sel = Selection {
            anchor: 5,
            head: 3,
            pref_col: 0,
        };
        assert_eq!(sel.range(), 3..6);
        assert!(sel.contains(3)); // head
        assert!(sel.contains(5)); // anchor
        assert!(!sel.contains(6));
    }
}
