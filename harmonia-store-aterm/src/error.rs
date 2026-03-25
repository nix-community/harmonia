use thiserror::Error;

use harmonia_store_core::store_path::{
    ParseContentAddressError, ParseStorePathError, StorePathError, StorePathNameError,
};

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
    #[error("invalid content address: {0}")]
    ContentAddress(#[from] ParseContentAddressError),
    #[error("invalid hash: {0}")]
    Hash(String),
    #[error("invalid UTF-8 in string at position {pos}")]
    InvalidUtf8 { pos: usize },
}

impl ParseError {
    /// Wrap a [`StorePathError`] into a [`ParseError::StorePath`].
    pub(crate) fn store_path_error(pos: usize, path: &str, error: StorePathError) -> Self {
        Self::StorePath {
            pos,
            source: ParseStorePathError {
                path: path.to_owned(),
                error,
            },
        }
    }
}
