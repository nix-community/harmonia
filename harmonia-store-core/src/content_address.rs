use crate::{FileIngestionMethod, Hash};
use std::fmt;

/// Content-addressed store path information
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentAddress {
    /// Text file with the given hash of its contents
    Text { hash: Hash },
    /// Fixed-output derivation with specified ingestion method and hash
    Fixed {
        method: FileIngestionMethod,
        hash: Hash,
    },
}

impl ContentAddress {
    /// Get the hash component of this content address
    pub fn hash(&self) -> &Hash {
        match self {
            Self::Text { hash } | Self::Fixed { hash, .. } => hash,
        }
    }

    /// Returns true if this is a text content address
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    /// Returns true if this is a fixed content address
    pub fn is_fixed(&self) -> bool {
        matches!(self, Self::Fixed { .. })
    }

    /// Parse a content address from its byte representation
    /// Format: b"text:sha256:..." or b"fixed:[r:]sha256:..."
    pub fn parse(s: &[u8]) -> Result<Self, ContentAddressError> {
        // Find positions of colons
        let mut colon_positions = Vec::new();
        for (i, &b) in s.iter().enumerate() {
            if b == b':' {
                colon_positions.push(i);
                if colon_positions.len() == 3 {
                    break;
                }
            }
        }

        if colon_positions.is_empty() {
            return Err(ContentAddressError::InvalidFormat(
                String::from_utf8_lossy(s).into_owned(),
            ));
        }

        let prefix = &s[..colon_positions[0]];

        match prefix {
            b"text" => {
                if colon_positions.len() < 2 {
                    return Err(ContentAddressError::InvalidFormat(
                        String::from_utf8_lossy(s).into_owned(),
                    ));
                }
                let hash_bytes = &s[colon_positions[0] + 1..];
                let hash = Hash::parse(hash_bytes)
                    .map_err(|e| ContentAddressError::InvalidHash(e.to_string()))?;
                Ok(Self::Text { hash })
            }
            b"fixed" => {
                if colon_positions.len() < 2 {
                    return Err(ContentAddressError::InvalidFormat(
                        String::from_utf8_lossy(s).into_owned(),
                    ));
                }

                // Check if there's an 'r' after the first colon
                if colon_positions.len() >= 2 && s.get(colon_positions[0] + 1) == Some(&b'r') {
                    // fixed:r:hash format
                    if colon_positions.len() < 3 {
                        return Err(ContentAddressError::InvalidFormat(
                            String::from_utf8_lossy(s).into_owned(),
                        ));
                    }
                    let hash_bytes = &s[colon_positions[1] + 1..];
                    let hash = Hash::parse(hash_bytes)
                        .map_err(|e| ContentAddressError::InvalidHash(e.to_string()))?;
                    Ok(Self::Fixed {
                        method: FileIngestionMethod::Recursive,
                        hash,
                    })
                } else {
                    // fixed:hash format
                    let hash_bytes = &s[colon_positions[0] + 1..];
                    let hash = Hash::parse(hash_bytes)
                        .map_err(|e| ContentAddressError::InvalidHash(e.to_string()))?;
                    Ok(Self::Fixed {
                        method: FileIngestionMethod::Flat,
                        hash,
                    })
                }
            }
            _ => Err(ContentAddressError::InvalidFormat(
                String::from_utf8_lossy(s).into_owned(),
            )),
        }
    }
}

impl fmt::Display for ContentAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { hash } => {
                write!(
                    f,
                    "text:{}:{}",
                    hash.algo,
                    String::from_utf8_lossy(&hash.to_hex())
                )
            }
            Self::Fixed {
                method: FileIngestionMethod::Flat,
                hash,
            } => {
                write!(
                    f,
                    "fixed:{}:{}",
                    hash.algo,
                    String::from_utf8_lossy(&hash.to_hex())
                )
            }
            Self::Fixed {
                method: FileIngestionMethod::Recursive,
                hash,
            } => {
                write!(
                    f,
                    "fixed:r:{}:{}",
                    hash.algo,
                    String::from_utf8_lossy(&hash.to_hex())
                )
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContentAddressError {
    #[error("Invalid content address format: {0}")]
    InvalidFormat(String),

    #[error("Invalid hash in content address: {0}")]
    InvalidHash(String),
}
