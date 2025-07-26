use base64::{Engine, engine::general_purpose};
use ed25519_dalek::Signature as Ed25519Signature;
use std::fmt;
use std::hash::{Hash, Hasher};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SignatureError {
    #[error("Failed to decode base64: {0}")]
    Base64Decode(#[from] base64::DecodeError),

    #[error("Invalid signature length: expected 64 bytes, got {0}")]
    InvalidLength(usize),

    #[error("Invalid signature format: {0}")]
    InvalidFormat(String),

    #[error("Failed to parse ed25519 signature: {0}")]
    Ed25519(#[from] ed25519_dalek::SignatureError),
}

/// A newtype wrapper around an Ed25519 signature
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature(Ed25519Signature);

impl PartialOrd for Signature {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Signature {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_bytes().cmp(&other.to_bytes())
    }
}

impl Signature {
    /// Create a new signature from raw bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SignatureError> {
        if bytes.len() != 64 {
            return Err(SignatureError::InvalidLength(bytes.len()));
        }
        let sig = Ed25519Signature::from_slice(bytes)?;
        Ok(Signature(sig))
    }

    /// Get the raw bytes of the signature
    pub fn to_bytes(&self) -> [u8; 64] {
        self.0.to_bytes()
    }

    /// Convert signature to base64-encoded text
    pub fn to_base64(&self) -> String {
        general_purpose::STANDARD.encode(self.to_bytes())
    }

    /// Parse a signature from base64-encoded bytes
    pub fn from_base64(s: &[u8]) -> Result<Self, SignatureError> {
        let bytes = general_purpose::STANDARD.decode(s)?;
        Self::from_bytes(&bytes)
    }

    /// Get the inner Ed25519 signature
    pub fn inner(&self) -> &Ed25519Signature {
        &self.0
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_base64())
    }
}

impl Hash for Signature {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.to_bytes().hash(state);
    }
}

/// A composite type containing a named signature
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NarSignature {
    /// The name/identifier of the public key used (e.g., "cache.nixos.org-1")
    pub key_name: Vec<u8>,
    /// The actual signature
    pub sig: Signature,
}

impl NarSignature {
    /// Create a new NAR signature
    pub fn new(key_name: Vec<u8>, sig: Signature) -> Self {
        Self { key_name, sig }
    }

    /// Convert to the Nix text format: "key-name:base64-signature"
    pub fn to_text(&self) -> String {
        format!(
            "{}:{}",
            String::from_utf8_lossy(&self.key_name),
            self.sig.to_base64()
        )
    }

    /// Parse from bytes format: b"key-name:base64-signature"
    pub fn parse(bytes: &[u8]) -> Result<Self, SignatureError> {
        let colon_pos = bytes
            .iter()
            .position(|&b| b == b':')
            .ok_or_else(|| SignatureError::InvalidFormat("Missing ':' separator".to_string()))?;

        let key_name = &bytes[..colon_pos];
        let sig_bytes = &bytes[colon_pos + 1..];

        if key_name.is_empty() {
            return Err(SignatureError::InvalidFormat("Empty key name".to_string()));
        }

        let sig = Signature::from_base64(sig_bytes)?;

        Ok(Self::new(key_name.to_vec(), sig))
    }
}

impl fmt::Display for NarSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_roundtrip() {
        let bytes = [42u8; 64];
        let sig = Signature::from_bytes(&bytes).unwrap();
        let base64 = sig.to_base64();
        let sig2 = Signature::from_base64(base64.as_bytes()).unwrap();
        assert_eq!(sig, sig2);
    }

    #[test]
    fn test_nar_signature_parse() {
        let text = "cache.example.com-1:6wzr1QlOPHG+knFuJIaw+85Z5ivwbdI512JikexG+nQ7JDSZM2hw8zzlcLrguzoLEpCA9VzaEEQflZEHVwy9AA==";
        let nar_sig = NarSignature::parse(text.as_bytes()).unwrap();
        assert_eq!(nar_sig.key_name, b"cache.example.com-1");
        assert_eq!(nar_sig.to_text(), text);
    }

    #[test]
    fn test_invalid_signature_length() {
        let bytes = [0u8; 32];
        let err = Signature::from_bytes(&bytes).unwrap_err();
        match err {
            SignatureError::InvalidLength(32) => {}
            _ => panic!("Expected InvalidLength error"),
        }
    }
}
