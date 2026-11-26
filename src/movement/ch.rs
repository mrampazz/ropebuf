use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

// TODO: let's add some proper grapheme tests with emojis and EOL behaviour, no need for just normal movement
