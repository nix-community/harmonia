use thiserror::Error;

use harmonia_store_path::{ParseStorePathError, StorePathNameError};

use crate::raw_output::FromRawOutputError;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("expected '{expected}' at position {pos}, found '{found}'")]
    UnexpectedChar {
        expected: char,
        found: char,
        pos: usize,
    },
    #[error("expected {expected} at position {pos}, reached end of input")]
    UnexpectedEof { expected: &'static str, pos: usize },
    #[error("unterminated string at position {pos}")]
    UnterminatedString { pos: usize },
    #[error("invalid store path at position {pos}: {source}")]
    StorePath {
        pos: usize,
        #[source]
        source: ParseStorePathError,
    },
    #[error("invalid output name: {0}")]
    OutputName(#[from] StorePathNameError),
    #[error("invalid output: {0}")]
    Output(#[from] FromRawOutputError),
    #[error("invalid UTF-8 in string at position {pos}")]
    InvalidUtf8 { pos: usize },
}
