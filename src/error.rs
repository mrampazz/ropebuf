use std::fmt;
use std::io::{self, Read, Write};

pub type Result<T> = std::result::Result<T, RopeError>;

#[derive(Debug)]
pub enum RopeError {
    Io(io::Error),
    WriteFailed {
        operation: &'static str,
        source: io::Error,
    },
    OutOfBounds {
        index: usize,
        len: usize,
    },
    InvalidRange {
        start: usize,
        end: usize,
    },
}

impl fmt::Display for RopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::WriteFailed { operation, source } => {
                write!(f, "failed to {operation} buffer content: {source}")
            }
            Self::OutOfBounds { index, len } => {
                write!(f, "index {index} out of bounds for length {len}")
            }
            Self::InvalidRange { start, end } => {
                write!(f, "invalid range {start}..{end}")
            }
        }
    }
}

impl std::error::Error for RopeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::WriteFailed { source, .. } => Some(source),
            Self::OutOfBounds { .. } | Self::InvalidRange { .. } => None,
        }
    }
}

impl From<io::Error> for RopeError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
