//! Everything regarding the word text objects

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum WordKind {
    Sub,
    Word,
    WORD,
}

pub fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub fn is_big_word(ch: char) -> bool {
    !ch.is_whitespace()
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
