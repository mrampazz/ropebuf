use crate::Rope;

impl Rope {
    /// returns the forward char index of the nth occurrence of the target char
    fn find_char_forward(&self, char_idx: usize, target: char, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
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
    fn find_char_backward(&self, char_idx: usize, target: char, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
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
    // TODO: implement search on the rope with / without regex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_fwd() {
        let r = Rope::from_str("hello ");
        assert_eq!(r.find_char_forward(0, 'o', 0), None);
        assert_eq!(r.find_char_forward(0, 'o', 1), Some(4));
        assert_eq!(r.find_char_forward(0, 'l', 1), Some(2));
        assert_eq!(r.find_char_forward(0, 'l', 2), Some(3));
    }

    #[test]
    fn search_bwd() {
        let r = Rope::from_str("hello ");
        assert_eq!(r.find_char_backward(5, 'o', 0), None);
        assert_eq!(r.find_char_backward(5, 'o', 1), Some(4));
        assert_eq!(r.find_char_backward(5, 'l', 1), Some(3));
        assert_eq!(r.find_char_backward(5, 'l', 2), Some(2));
    }
}
