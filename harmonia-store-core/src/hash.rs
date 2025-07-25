use crate::base32;
use base64::{engine::general_purpose, Engine as _};
use std::fmt;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgo {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl HashAlgo {
    pub fn name(&self) -> &'static str {
        match self {
            HashAlgo::Md5 => "md5",
            HashAlgo::Sha1 => "sha1",
            HashAlgo::Sha256 => "sha256",
            HashAlgo::Sha512 => "sha512",
        }
    }

    pub fn digest_size(&self) -> usize {
        match self {
            HashAlgo::Md5 => 16,
            HashAlgo::Sha1 => 20,
            HashAlgo::Sha256 => 32,
            HashAlgo::Sha512 => 64,
        }
    }

    pub fn base16_len(&self) -> usize {
        self.digest_size() * 2
    }

    pub fn base32_len(&self) -> usize {
        ((self.digest_size() * 8 - 1) / 5) + 1
    }

    pub fn base64_len(&self) -> usize {
        (4 * self.digest_size() / 3).div_ceil(4) * 4
    }
}

impl HashAlgo {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ParseHashError> {
        match bytes {
            b"md5" => Ok(HashAlgo::Md5),
            b"sha1" => Ok(HashAlgo::Sha1),
            b"sha256" => Ok(HashAlgo::Sha256),
            b"sha512" => Ok(HashAlgo::Sha512),
            _ => Err(ParseHashError::UnknownAlgorithm(
                String::from_utf8_lossy(bytes).to_string(),
            )),
        }
    }
}

impl fmt::Display for HashAlgo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hash {
    pub algo: HashAlgo,
    pub digest: Vec<u8>,
}

impl Hash {
    pub fn new(algo: HashAlgo, digest: Vec<u8>) -> Result<Self, ParseHashError> {
        if digest.len() != algo.digest_size() {
            return Err(ParseHashError::InvalidDigestSize {
                expected: algo.digest_size(),
                actual: digest.len(),
            });
        }
        Ok(Hash { algo, digest })
    }

    /// Create a hash from hex-encoded bytes and algorithm
    pub fn from_hex_bytes(algo: HashAlgo, hex_bytes: &[u8]) -> Result<Self, ParseHashError> {
        let digest =
            hex::decode(hex_bytes).map_err(|e| ParseHashError::HexDecodeError(e.to_string()))?;
        Self::new(algo, digest)
    }

    /// Parse a hash from bytes in the format "algo:hex_digest"
    /// e.g., b"sha256:1b4sb93wp679q4zx9k1ignby1yna3z7c4c2ri3wphylbc2dwsys0"
    pub fn parse(bytes: &[u8]) -> Result<Self, ParseHashError> {
        // Find the colon separator
        let colon_pos = bytes.iter().position(|&b| b == b':').ok_or_else(|| {
            ParseHashError::InvalidFormat(String::from_utf8_lossy(bytes).to_string())
        })?;

        let (algo_bytes, digest_bytes) = bytes.split_at(colon_pos);
        let digest_bytes = &digest_bytes[1..]; // Skip the colon

        let algo = HashAlgo::from_bytes(algo_bytes)?;

        // Try to decode the digest from various encodings
        let digest = if digest_bytes.len() == algo.base16_len() {
            // Hex encoding - hex::decode accepts &[u8]
            hex::decode(digest_bytes).map_err(|e| ParseHashError::HexDecodeError(e.to_string()))?
        } else if digest_bytes.len() == algo.base32_len() {
            // Nix base32 decoding - works with bytes directly
            base32::from_nix_base32(digest_bytes).map_err(ParseHashError::Base32DecodeError)?
        } else if digest_bytes.len() == algo.base64_len() {
            // Base64 decoding - accepts &[u8]
            general_purpose::STANDARD
                .decode(digest_bytes)
                .map_err(|e| ParseHashError::Base64DecodeError(e.to_string()))?
        } else {
            return Err(ParseHashError::InvalidDigestLength {
                algo: algo.name().to_string(),
                expected_lengths: vec![algo.base16_len(), algo.base32_len(), algo.base64_len()],
                actual: digest_bytes.len(),
            });
        };

        Hash::new(algo, digest)
    }

    /// Parse a hash from database format where it's stored as "algo:hex"
    /// Returns just the raw bytes of the hash digest
    pub fn parse_db_hash(bytes: &[u8]) -> Result<Vec<u8>, ParseHashError> {
        let hash = Self::parse(bytes)?;
        Ok(hash.digest)
    }

    /// Format hash as "algo:hex_digest" bytes
    pub fn to_sri(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(self.algo.name().len() + 1 + self.algo.base16_len());
        result.extend_from_slice(self.algo.name().as_bytes());
        result.push(b':');
        result.extend_from_slice(&self.to_hex());
        result
    }

    /// Get hex encoding of the digest as bytes
    pub fn to_hex(&self) -> Vec<u8> {
        hex::encode(&self.digest).into_bytes()
    }

    /// Get the raw digest bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.digest
    }

    /// Get base32 (nix-style) encoding of the digest as bytes
    pub fn to_nix_base32(&self) -> Vec<u8> {
        base32::to_nix_base32(&self.digest)
    }

    /// Get base64 encoding of the digest as bytes
    pub fn to_base64(&self) -> Vec<u8> {
        general_purpose::STANDARD.encode(&self.digest).into_bytes()
    }
}

#[derive(Error, Debug)]
pub enum ParseHashError {
    #[error("Unknown hash algorithm: {0}")]
    UnknownAlgorithm(String),

    #[error("Invalid hash format: {0}")]
    InvalidFormat(String),

    #[error("Invalid digest size: expected {expected}, got {actual}")]
    InvalidDigestSize { expected: usize, actual: usize },

    #[error(
        "Invalid digest length for {algo}: expected one of {expected_lengths:?}, got {actual}"
    )]
    InvalidDigestLength {
        algo: String,
        expected_lengths: Vec<usize>,
        actual: usize,
    },

    #[error("Hex decode error: {0}")]
    HexDecodeError(String),

    #[error("Base32 decode error: {0}")]
    Base32DecodeError(String),

    #[error("Base64 decode error: {0}")]
    Base64DecodeError(String),

    #[error("Unsupported encoding: {0}")]
    UnsupportedEncoding(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sha256_hex() {
        let hash_bytes = b"sha256:1b4a5c9d8e7f2a3b6c9d8e7f2a3b6c9d8e7f2a3b6c9d8e7f2a3b6c9d8e7f2a3b";
        let hash = Hash::parse(hash_bytes).unwrap();
        assert_eq!(hash.algo, HashAlgo::Sha256);
        assert_eq!(hash.digest.len(), 32);
    }

    #[test]
    fn test_parse_invalid_format() {
        let result = Hash::parse(b"sha256-invalid");
        assert!(matches!(result, Err(ParseHashError::InvalidFormat(_))));
    }

    #[test]
    fn test_parse_db_hash() {
        let db_hash = b"sha256:1b4a5c9d8e7f2a3b6c9d8e7f2a3b6c9d8e7f2a3b6c9d8e7f2a3b6c9d8e7f2a3b";
        let digest = Hash::parse_db_hash(db_hash).unwrap();
        assert_eq!(digest.len(), 32);
    }

    #[test]
    fn test_hello_world_encodings() {
        // Test against known values from nix CLI
        let hello_world_digest =
            hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
                .unwrap();
        let hash = Hash::new(HashAlgo::Sha256, hello_world_digest).unwrap();

        assert_eq!(
            hash.to_hex(),
            b"b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(
            hash.to_nix_base32(),
            b"1sfdxziarxw8j3p80lvswgpq9i7smdyxmmsj5sjhhgjdjfwjfkdr"
        );
        assert_eq!(
            hash.to_base64(),
            b"uU0nuZNNPgilLlLX2n2r+sSE7+N6U4DukIj3rOLvzek="
        );
    }

    #[test]
    fn test_real_nix_hash_conversion() {
        // Test the actual hash from the bug report
        let hex = "ab00922634303a8b47680f96752c3ff1017a21cf84e6b0b4f28fc3f2346da666";
        let expected_base32 = "0rm6dlsg5hwgyasb1rl4rwhpl0gi7wn7b5hgd13qnfih6hk9405b";

        let digest = hex::decode(hex).unwrap();
        let hash = Hash::new(HashAlgo::Sha256, digest).unwrap();

        assert_eq!(hash.to_nix_base32(), expected_base32.as_bytes());

        // Test parsing from database format
        let db_hash = format!("sha256:{hex}");
        let parsed = Hash::parse(db_hash.as_bytes()).unwrap();
        assert_eq!(parsed.to_hex(), hex.as_bytes());
        assert_eq!(parsed.to_nix_base32(), expected_base32.as_bytes());
    }

    #[test]
    fn test_parse_all_encodings() {
        let hex = "ab00922634303a8b47680f96752c3ff1017a21cf84e6b0b4f28fc3f2346da666";
        let base32 = "0rm6dlsg5hwgyasb1rl4rwhpl0gi7wn7b5hgd13qnfih6hk9405b";
        let digest = hex::decode(hex).unwrap();
        let hash = Hash::new(HashAlgo::Sha256, digest).unwrap();
        let base64 = String::from_utf8(hash.to_base64()).unwrap();

        // Test parsing hex
        let parsed_hex = Hash::parse(format!("sha256:{hex}").as_bytes()).unwrap();
        assert_eq!(parsed_hex.to_hex(), hex.as_bytes());

        // Test parsing base32
        let parsed_base32 = Hash::parse(format!("sha256:{base32}").as_bytes()).unwrap();
        assert_eq!(parsed_base32.to_hex(), hex.as_bytes());

        // Test parsing base64
        let parsed_base64 = Hash::parse(format!("sha256:{base64}").as_bytes()).unwrap();
        assert_eq!(parsed_base64.to_hex(), hex.as_bytes());
    }
}
