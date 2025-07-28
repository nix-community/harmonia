use crate::signature::{NarSignature, Signature};
use base64::{Engine, engine::general_purpose};
use ed25519_dalek::{Signer, SigningKey as DalekSigningKey};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("Failed to read signing key file: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to decode base64: {0}")]
    Base64Decode(#[from] base64::DecodeError),

    #[error("Failed to parse signing key: {0}")]
    ParseKey(String),

    #[error("Invalid signing key length: expected 32 or 64 bytes, got {0}")]
    InvalidKeyLength(usize),
}

/// A signing key with a name
#[derive(Clone, Debug)]
pub struct SigningKey {
    /// The name of the key (e.g., "cache.example.com-1")
    pub name: Vec<u8>,
    /// The raw key bytes (32 or 64 bytes)
    pub key: Vec<u8>,
}

impl SigningKey {
    /// Create a new signing key
    pub fn new(name: Vec<u8>, key: Vec<u8>) -> Result<Self, SigningError> {
        if key.len() != 32 && key.len() != 64 {
            return Err(SigningError::InvalidKeyLength(key.len()));
        }
        Ok(Self { name, key })
    }

    /// Parse a secret key from a file
    ///
    /// The file should contain a key in the format: "name:base64-key"
    /// The key can be either 32 bytes (secret key only) or 64 bytes (keypair)
    pub fn from_file(path: &Path) -> Result<Self, SigningError> {
        let content = std::fs::read(path)?;
        Self::parse(&content)
    }

    /// Parse a secret key from bytes
    ///
    /// The bytes should be in the format: b"name:base64-key"
    pub fn parse(s: &[u8]) -> Result<Self, SigningError> {
        let colon_pos = s
            .iter()
            .position(|&b| b == b':')
            .ok_or_else(|| SigningError::ParseKey("Sign key does not contain a ':'".to_string()))?;

        let name = &s[..colon_pos];
        let key_base64 = &s[colon_pos + 1..];

        if name.is_empty() {
            return Err(SigningError::ParseKey("Empty key name".to_string()));
        }

        // Trim whitespace from base64 part
        let key_base64 = match key_base64.iter().rposition(|&b| !b.is_ascii_whitespace()) {
            Some(end) => &key_base64[..=end],
            None => key_base64,
        };
        let key_base64 = match key_base64.iter().position(|&b| !b.is_ascii_whitespace()) {
            Some(start) => &key_base64[start..],
            None => key_base64,
        };

        let key = general_purpose::STANDARD.decode(key_base64)?;

        // Validate the key by trying to create a DalekSigningKey
        if key.len() == 32 {
            let key_bytes: [u8; 32] = key.as_slice().try_into().map_err(|_| {
                SigningError::ParseKey("Failed to convert key bytes to [u8; 32]".to_string())
            })?;
            let _ = DalekSigningKey::from_bytes(&key_bytes);
        } else if key.len() == 64 {
            let keypair_bytes: [u8; 64] = key.as_slice().try_into().map_err(|_| {
                SigningError::ParseKey("Failed to convert key bytes to [u8; 64]".to_string())
            })?;
            let _ = DalekSigningKey::from_keypair_bytes(&keypair_bytes)
                .map_err(|e| SigningError::ParseKey(format!("Invalid Ed25519 keypair: {e}")))?;
        } else {
            return Err(SigningError::InvalidKeyLength(key.len()));
        }

        Ok(Self {
            name: name.to_vec(),
            key,
        })
    }

    /// Get the Ed25519 signing key
    fn to_dalek_key(&self) -> DalekSigningKey {
        if self.key.len() == 32 {
            let key_bytes: [u8; 32] = self
                .key
                .as_slice()
                .try_into()
                .expect("Invalid key length for Ed25519");
            DalekSigningKey::from_bytes(&key_bytes)
        } else if self.key.len() == 64 {
            let keypair_bytes: [u8; 64] = self
                .key
                .as_slice()
                .try_into()
                .expect("Invalid key length for Ed25519 keypair");
            DalekSigningKey::from_keypair_bytes(&keypair_bytes).expect("Invalid Ed25519 keypair")
        } else {
            panic!("Invalid signing key length: {}", self.key.len());
        }
    }

    /// Sign a message and return a NarSignature
    pub fn sign(&self, msg: &[u8]) -> NarSignature {
        let dalek_key = self.to_dalek_key();
        let signature = dalek_key.sign(msg);
        let sig = Signature::from_bytes(&signature.to_bytes())
            .expect("Ed25519 signature should always be valid");
        NarSignature::new(self.name.clone(), sig)
    }

    /// Sign a message and return the signature string in the format "name:base64-signature"
    pub fn sign_string(&self, msg: &[u8]) -> String {
        self.sign(msg).to_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::fingerprint_path;
    use crate::signature::NarSignature;

    #[test]
    fn test_parse_signing_key() {
        // Use a 32-byte key for simpler testing
        let key_str = "test-key:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let key = SigningKey::parse(key_str.as_bytes()).unwrap();
        assert_eq!(key.name, b"test-key");
        assert_eq!(key.key.len(), 32);
    }

    #[test]
    fn test_sign_message() {
        // Use a simple 32-byte key
        let key_str = "test-key:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let key = SigningKey::parse(key_str.as_bytes()).unwrap();

        let msg = b"Hello, world!";
        let signature = key.sign_string(msg);

        // Check format
        assert!(signature.starts_with("test-key:"));
        assert!(signature.contains(':'));

        // Verify it's a valid base64 signature
        let nar_sig = NarSignature::parse(signature.as_bytes()).unwrap();
        assert_eq!(nar_sig.key_name, b"test-key");
    }

    #[test]
    fn test_sign_fingerprint() {
        // Use a simple 32-byte key
        let key_str = "cache.example.com-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let key = SigningKey::parse(key_str.as_bytes()).unwrap();

        let mut references = std::collections::BTreeSet::new();
        references.insert(crate::StorePath::from(
            b"/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1" as &[u8],
        ));
        references.insert(crate::StorePath::from(
            b"/nix/store/sl141d1g77wvhr050ah87lcyz2czdxa3-glibc-2.40-36" as &[u8],
        ));

        let store_path = crate::StorePath::from(
            b"/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1" as &[u8],
        );
        let fingerprint = fingerprint_path(
            b"/nix/store",
            &store_path,
            b"sha256:1mkvday29m2qxg1fnbv8xh9s6151bh8a2xzhh0k86j7lqhyfwibh",
            226560,
            &references,
        )
        .unwrap();

        let signature = key.sign_string(&fingerprint);

        // The signature should be deterministic
        assert!(signature.starts_with("cache.example.com-1:"));

        // Verify it's a valid signature
        let nar_sig = NarSignature::parse(signature.as_bytes()).unwrap();
        assert_eq!(nar_sig.key_name, b"cache.example.com-1");
    }

    #[test]
    fn test_invalid_key_format() {
        assert!(SigningKey::parse(b"no-colon").is_err());
        assert!(SigningKey::parse(b":no-name").is_err());
        assert!(SigningKey::parse(b"name:invalid-base64!!!").is_err());
    }

    #[test]
    fn test_32_byte_key() {
        // 32-byte key (secret key only)
        let key_str = "test-key:zFD7RJEU40VJzJvgT7h5xQwFm8FufXKH2CJPaKvh/xo=";
        let key = SigningKey::parse(key_str.as_bytes()).unwrap();
        assert_eq!(key.key.len(), 32);

        // Should still be able to sign
        let msg = b"test message";
        let signature = key.sign_string(msg);
        assert!(signature.starts_with("test-key:"));
    }
}
