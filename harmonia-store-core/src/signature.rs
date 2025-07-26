use base64::{Engine, engine::general_purpose};
use ed25519_dalek::Signature as Ed25519Signature;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
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

    /// Parse a signature from base64-encoded text
    pub fn from_base64(s: &str) -> Result<Self, SignatureError> {
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

impl FromStr for Signature {
    type Err = SignatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_base64(s)
    }
}

impl Hash for Signature {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.to_bytes().hash(state);
    }
}

/// A composite type containing a named signature
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NarSignature {
    /// The name/identifier of the public key used (e.g., "cache.nixos.org-1")
    pub key_name: String,
    /// The actual signature
    pub sig: Signature,
}

impl NarSignature {
    /// Create a new NAR signature
    pub fn new(key_name: String, sig: Signature) -> Self {
        Self { key_name, sig }
    }

    /// Convert to the Nix text format: "key-name:base64-signature"
    pub fn to_text(&self) -> String {
        format!("{}:{}", self.key_name, self.sig.to_base64())
    }

    /// Parse from the Nix text format: "key-name:base64-signature"
    pub fn parse(s: &str) -> Result<Self, SignatureError> {
        let (key_name, sig_str) = s
            .split_once(':')
            .ok_or_else(|| SignatureError::InvalidFormat("Missing ':' separator".to_string()))?;

        if key_name.is_empty() {
            return Err(SignatureError::InvalidFormat("Empty key name".to_string()));
        }

        let sig = Signature::from_base64(sig_str)?;
        Ok(Self::new(key_name.to_string(), sig))
    }
}

impl fmt::Display for NarSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_text())
    }
}

impl FromStr for NarSignature {
    type Err = SignatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
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
        let sig2 = Signature::from_base64(&base64).unwrap();
        assert_eq!(sig, sig2);
    }

    #[test]
    fn test_nar_signature_parse() {
        let text = "cache.example.com-1:6wzr1QlOPHG+knFuJIaw+85Z5ivwbdI512JikexG+nQ7JDSZM2hw8zzlcLrguzoLEpCA9VzaEEQflZEHVwy9AA==";
        let nar_sig = NarSignature::parse(text).unwrap();
        assert_eq!(nar_sig.key_name, "cache.example.com-1");
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
