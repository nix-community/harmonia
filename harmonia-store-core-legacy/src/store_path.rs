use std::fmt;
use std::ops::Deref;

/// A Nix store path
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorePath {
    path: Vec<u8>,
}

impl StorePath {
    /// Create a new store path
    pub fn new(path: Vec<u8>) -> Self {
        Self { path }
    }

    /// Get the path as bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.path
    }

    /// Get the path as a vector of bytes
    pub fn to_vec(&self) -> Vec<u8> {
        self.path.clone()
    }

    /// Create a StorePath from a byte slice
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            path: bytes.to_vec(),
        }
    }
}

impl Deref for StorePath {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl From<Vec<u8>> for StorePath {
    fn from(path: Vec<u8>) -> Self {
        Self::new(path)
    }
}

impl From<&[u8]> for StorePath {
    fn from(path: &[u8]) -> Self {
        Self::new(path.to_vec())
    }
}

impl fmt::Display for StorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.path))
    }
}
