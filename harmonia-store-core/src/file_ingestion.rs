use std::fmt;

/// Method for ingesting files into the Nix store
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileIngestionMethod {
    /// File contents are stored as-is (flat)
    Flat,
    /// File contents are stored as a NAR (Nix ARchive)
    Recursive,
}

impl FileIngestionMethod {
    /// Returns true if this is the recursive (NAR) method
    pub fn is_recursive(&self) -> bool {
        matches!(self, Self::Recursive)
    }

    /// Returns the string representation used in the Nix daemon protocol
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Recursive => "recursive",
        }
    }
}

impl fmt::Display for FileIngestionMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for FileIngestionMethod {
    type Err = FileIngestionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "flat" => Ok(Self::Flat),
            "recursive" => Ok(Self::Recursive),
            _ => Err(FileIngestionError::InvalidMethod(s.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FileIngestionError {
    #[error("Invalid file ingestion method: {0}")]
    InvalidMethod(String),
}
