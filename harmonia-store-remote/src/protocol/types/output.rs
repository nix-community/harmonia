use std::fmt;

/// A validated output name for Nix derivations
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputName(Vec<u8>);

impl OutputName {
    /// Create a new OutputName with validation
    pub fn new(name: Vec<u8>) -> Result<Self, OutputNameError> {
        Self::validate(&name)?;
        Ok(Self(name))
    }

    /// Create an OutputName from a byte slice
    pub fn from_bytes(name: &[u8]) -> Result<Self, OutputNameError> {
        Self::new(name.to_vec())
    }

    /// Get the output name as bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consume self and return the inner Vec<u8>
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }

    /// Validate an output name according to Nix rules
    fn validate(name: &[u8]) -> Result<(), OutputNameError> {
        if name.is_empty() {
            return Err(OutputNameError::Empty);
        }

        // First character must be ASCII letter or underscore
        if !name[0].is_ascii_alphabetic() && name[0] != b'_' {
            return Err(OutputNameError::InvalidStart(name[0] as char));
        }

        // Rest must be ASCII letters, digits, underscores, hyphens, or plus signs
        for (i, &byte) in name.iter().enumerate() {
            if !byte.is_ascii_alphanumeric() && byte != b'_' && byte != b'-' && byte != b'+' {
                return Err(OutputNameError::InvalidChar(byte as char, i));
            }
        }

        Ok(())
    }
}

impl fmt::Display for OutputName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Safe to use lossy here since we validated ASCII during construction
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OutputNameError {
    #[error("Output name cannot be empty")]
    Empty,

    #[error("Output name must start with a letter or underscore, got '{0}'")]
    InvalidStart(char),

    #[error("Invalid character '{0}' at position {1} in output name")]
    InvalidChar(char, usize),
}
