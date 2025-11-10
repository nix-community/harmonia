#![allow(unsafe_code)]

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use data_encoding::BASE64;

use ring::error::{KeyRejected, Unspecified};
use ring::rand;
use ring::signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use thiserror::Error;
use tracing::error;

use crate::wire::base64_len;

pub const SIGNATURE_BYTES: usize = 64;
const SIGNATURE_BASE64_LEN: usize = base64_len(SIGNATURE_BYTES);
const SIGNATURE_BASE64_DECODED_LEN: usize = 66;
pub const SEED_BYTES: usize = 32;
pub const PUBLIC_KEY_BYTES: usize = 32;
const PUBLIC_KEY_BASE64_LEN: usize = base64_len(PUBLIC_KEY_BYTES);
const PUBLIC_KEY_BASE64_DECODED_LEN: usize = 33;
pub const SECRET_KEY_BYTES: usize = SEED_BYTES + PUBLIC_KEY_BYTES;
const SECRET_KEY_BASE64_LEN: usize = base64_len(SECRET_KEY_BYTES);
const SECRET_KEY_BASE64_DECODED_LEN: usize = 66;

#[derive(Error, Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum ParseSignatureError {
    #[error("signature is corrupt")]
    CorruptSignature,
    #[error("signature is not valid")]
    InvalidSignature,
}

pub type SignatureSet = BTreeSet<Signature>;

#[derive(
    Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, DeserializeFromStr, SerializeDisplay,
)]
pub struct Signature(Arc<String>, [u8; SIGNATURE_BYTES]);

impl Signature {
    pub fn name(&self) -> &str {
        &self.0
    }

    pub fn signature_bytes(&self) -> &[u8] {
        &self.1[..]
    }

    pub fn signature(&self) -> impl fmt::Display + '_ {
        Base64Display::<SIGNATURE_BASE64_LEN>(self.signature_bytes())
    }

    pub fn from_parts(name: &str, signature: &[u8]) -> Result<Signature, ParseSignatureError> {
        if signature.len() != SIGNATURE_BYTES {
            error!(
                "Signature wrong length {}!={}",
                signature.len(),
                SIGNATURE_BYTES
            );
            return Err(ParseSignatureError::InvalidSignature);
        }
        let mut data = [0u8; SIGNATURE_BYTES];
        data.copy_from_slice(signature);

        Ok(Self(Arc::new(name.to_string()), data))
    }
}

struct Base64Display<'d, const N: usize>(&'d [u8]);
impl<const N: usize> fmt::Display for Base64Display<'_, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; N];
        BASE64.encode_mut(self.0, &mut buf);

        // SAFETY: Base64 is a subset of ASCII, which guarantees valid UTF-8.
        let s = unsafe { std::str::from_utf8_unchecked(&buf) };
        write!(f, "{s}")
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.name(), self.signature())
    }
}

impl FromStr for Signature {
    type Err = ParseSignatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut sp = s.splitn(2, ':');
        let name = Arc::new(
            sp.next()
                .ok_or(ParseSignatureError::CorruptSignature)?
                .to_string(),
        );
        let sig_s = sp.next().ok_or(ParseSignatureError::CorruptSignature)?;
        if sig_s.len() != SIGNATURE_BASE64_LEN {
            return Err(ParseSignatureError::InvalidSignature);
        }
        let mut sig_b = [0u8; SIGNATURE_BASE64_DECODED_LEN];
        let len = BASE64
            .decode_mut(sig_s.as_bytes(), &mut sig_b)
            .map_err(|_| ParseSignatureError::InvalidSignature)?;
        if len != SIGNATURE_BYTES {
            error!("Signature wrong length {}!={}", len, SIGNATURE_BYTES);
            return Err(ParseSignatureError::InvalidSignature);
        }
        let mut sig_buf = [0u8; SIGNATURE_BYTES];
        sig_buf.copy_from_slice(&sig_b[..SIGNATURE_BYTES]);
        Ok(Signature(name, sig_buf))
    }
}

#[derive(Error, Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum ParseKeyError {
    #[error("key is corrupt")]
    CorruptKey,
    #[error("secret key is not valid")]
    InvalidSecretKey,
    #[error("public key is not valid")]
    InvalidPublicKey,
}

#[derive(Clone)]
pub struct PublicKey {
    name: Arc<String>,
    key_data: [u8; PUBLIC_KEY_BYTES],
    key: UnparsedPublicKey<[u8; PUBLIC_KEY_BYTES]>,
}

impl PublicKey {
    pub fn verify<M: AsRef<[u8]>>(&self, data: M, signature: &Signature) -> bool {
        let message = data.as_ref();
        self.key
            .verify(message, signature.signature_bytes())
            .is_ok()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> impl fmt::Display + '_ {
        Base64Display::<PUBLIC_KEY_BASE64_LEN>(&self.key_data)
    }
}

impl PartialEq for PublicKey {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.key_data == other.key_data
    }
}

impl Eq for PublicKey {}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicKey")
            .field("name", &self.name)
            .field("key", &format_args!("{}", self.key()))
            .finish()
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.name(), self.key())
    }
}

impl FromStr for PublicKey {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut sp = s.splitn(2, ':');
        let name = Arc::new(sp.next().ok_or(ParseKeyError::CorruptKey)?.to_string());
        let key_s = sp.next().ok_or(ParseKeyError::CorruptKey)?;
        if key_s.len() != PUBLIC_KEY_BASE64_LEN {
            return Err(ParseKeyError::InvalidPublicKey);
        }
        let mut key_buf = [0u8; PUBLIC_KEY_BASE64_DECODED_LEN];
        let len = BASE64
            .decode_mut(key_s.as_bytes(), &mut key_buf)
            .map_err(|_| ParseKeyError::InvalidPublicKey)?;
        if len != PUBLIC_KEY_BYTES {
            return Err(ParseKeyError::InvalidPublicKey);
        }
        let mut key_data = [0u8; PUBLIC_KEY_BYTES];
        key_data.copy_from_slice(&key_buf[..PUBLIC_KEY_BYTES]);
        let key = UnparsedPublicKey::new(&signature::ED25519, key_data);
        Ok(PublicKey {
            name,
            key,
            key_data,
        })
    }
}

#[derive(Error, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[error("error generating key")]
pub struct GenerateKeyError;

impl From<Unspecified> for GenerateKeyError {
    fn from(_: Unspecified) -> Self {
        GenerateKeyError
    }
}
impl From<KeyRejected> for GenerateKeyError {
    fn from(_: KeyRejected) -> Self {
        GenerateKeyError
    }
}

pub struct SecretKey {
    name: Arc<String>,
    key_data: [u8; SECRET_KEY_BYTES],
    key: Ed25519KeyPair,
}

impl SecretKey {
    pub fn generate(
        name: String,
        rng: &dyn rand::SecureRandom,
    ) -> Result<SecretKey, GenerateKeyError> {
        let name = Arc::new(name);
        let seed: [u8; SEED_BYTES] = rand::generate(rng)?.expose();
        let key = Ed25519KeyPair::from_seed_unchecked(&seed)?;
        let pk = key.public_key();
        let mut key_data = [0u8; SECRET_KEY_BYTES];
        key_data[0..SEED_BYTES].copy_from_slice(&seed);
        key_data[SEED_BYTES..SECRET_KEY_BYTES].copy_from_slice(pk.as_ref());
        Ok(SecretKey {
            name,
            key,
            key_data,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> impl fmt::Display + '_ {
        Base64Display::<SECRET_KEY_BASE64_LEN>(&self.key_data)
    }

    pub fn sign<M: AsRef<[u8]>>(&self, data: M) -> Signature {
        let msg = data.as_ref();
        let sig = self.key.sign(msg);
        let mut sig_buf = [0u8; SIGNATURE_BYTES];
        sig_buf.copy_from_slice(sig.as_ref());
        Signature(self.name.clone(), sig_buf)
    }

    pub fn to_public_key(&self) -> PublicKey {
        let name = self.name.clone();
        let peer_public_key_bytes = self.key.public_key();
        let mut key_buf = [0u8; PUBLIC_KEY_BYTES];
        key_buf.copy_from_slice(peer_public_key_bytes.as_ref());
        let key = UnparsedPublicKey::new(&signature::ED25519, key_buf);
        let mut key_data = [0u8; PUBLIC_KEY_BYTES];
        key_data.copy_from_slice(peer_public_key_bytes.as_ref());
        PublicKey {
            name,
            key,
            key_data,
        }
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretKey")
            .field("name", &self.name)
            .field("key", &format_args!("{}", self.key()))
            .finish()
    }
}
impl fmt::Display for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.name(), self.key())
    }
}

impl PartialEq for SecretKey {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.key_data == other.key_data
    }
}

impl Eq for SecretKey {}

impl From<SecretKey> for PublicKey {
    fn from(v: SecretKey) -> Self {
        v.to_public_key()
    }
}

impl<'a> From<&'a SecretKey> for PublicKey {
    fn from(v: &'a SecretKey) -> Self {
        v.to_public_key()
    }
}

impl FromStr for SecretKey {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut sp = s.splitn(2, ':');
        let name = Arc::new(sp.next().ok_or(ParseKeyError::CorruptKey)?.to_string());
        let key_s = sp.next().ok_or(ParseKeyError::CorruptKey)?;
        if key_s.len() != SECRET_KEY_BASE64_LEN {
            return Err(ParseKeyError::InvalidSecretKey);
        }
        let mut key_b = [0u8; SECRET_KEY_BASE64_DECODED_LEN];
        let len = BASE64
            .decode_mut(key_s.as_bytes(), &mut key_b)
            .map_err(|_| ParseKeyError::InvalidSecretKey)?;
        if len != SECRET_KEY_BYTES {
            return Err(ParseKeyError::InvalidSecretKey);
        }
        let mut key_data = [0u8; SECRET_KEY_BYTES];
        key_data.copy_from_slice(&key_b[..SECRET_KEY_BYTES]);
        let seed = &key_data[0..SEED_BYTES];
        let public_key = &key_data[SEED_BYTES..SECRET_KEY_BYTES];
        let key = Ed25519KeyPair::from_seed_and_public_key(seed, public_key)
            .map_err(|_| ParseKeyError::InvalidSecretKey)?;
        Ok(SecretKey {
            name,
            key,
            key_data,
        })
    }
}

#[cfg(any(test, feature = "test"))]
pub mod proptests {
    use super::*;
    use ::proptest::{arbitrary::Arbitrary, prelude::*};

    pub fn arb_key_name(max: u8) -> impl Strategy<Value = String> {
        "[a-zA-Z0-9+\\-_?=][a-zA-Z0-9+\\-_?=.]{0,210}".prop_map(move |mut s| {
            if s.len() > max as usize {
                s.truncate(max as usize);
            }
            s
        })
    }

    pub fn arb_signature(max: u8) -> impl Strategy<Value = Signature> {
        (arb_key_name(max), any::<[u8; SIGNATURE_BYTES]>())
            .prop_map(|(name, signature)| Signature(Arc::new(name), signature))
    }

    pub fn arb_signatures() -> impl Strategy<Value = BTreeSet<Signature>> {
        prop::collection::btree_set(any::<Signature>(), 0..5)
    }

    impl Arbitrary for Signature {
        type Parameters = ();
        type Strategy = BoxedStrategy<Signature>;
        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_signature(211).boxed()
        }
    }
}

#[cfg(test)]
mod unittests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_public_key() {
        let sk_s = "cache.example.org-1:ZJui+kG6vPCSRD4+p1P4DyUVlASmp/zsaeN84PTFW28tj2/PtQWvFWK6Mw+ay8kGif8AZkR5KosHLvuwlzDlgg==";
        let sk: SecretKey = sk_s.parse().unwrap();
        assert_eq!("cache.example.org-1", sk.name());
        let pk_s = "cache.example.org-1:LY9vz7UFrxViujMPmsvJBon/AGZEeSqLBy77sJcw5YI=";
        let pk: PublicKey = pk_s.parse().unwrap();
        assert_eq!("cache.example.org-1", pk.name());
        assert_eq!(sk.to_public_key(), pk);
        assert_eq!(sk.to_string(), sk_s);
        assert_eq!(pk.to_string(), pk_s);
    }

    #[test]
    fn test_generate() {
        let rng = rand::SystemRandom::new();
        let sk_gen = SecretKey::generate("cache.example.org-1".into(), &rng).unwrap();
        let sk_s = sk_gen.to_string();
        let sk: SecretKey = sk_s.parse().unwrap();
        assert_eq!(sk_gen, sk);
        assert_eq!(sk.to_string(), sk_s);
        let pk_s = sk_gen.to_public_key().to_string();
        let pk: PublicKey = pk_s.parse().unwrap();
        assert_eq!(sk.to_public_key(), pk);
        assert_eq!(pk.to_string(), pk_s);
    }

    #[test]
    fn test_verify() {
        let data = "1;/nix/store/02bfycjg1607gpcnsg8l13lc45qa8qj3-libssh2-1.10.0;sha256:1l29f8r5q2739wnq4i7m2v545qx77b3wrdsw9xz2ajiy3hv1al8b;294664;/nix/store/02bfycjg1607gpcnsg8l13lc45qa8qj3-libssh2-1.10.0,/nix/store/1l4r0r4ab3v3a3ppir4jwiah3icalk9d-zlib-1.2.11,/nix/store/gf6j3k1flnhayvpnwnhikkg0s5dxrn1i-openssl-1.1.1l,/nix/store/z56jcx3j1gfyk4sv7g8iaan0ssbdkhz1-glibc-2.33-56";
        let s : Signature = "cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==".parse().unwrap();
        assert_eq!("cache.nixos.org-1", s.name());
        assert_eq!(
            s.to_string(),
            "cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="
        );
        let pk: PublicKey = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
            .parse()
            .unwrap();
        assert!(pk.verify(data, &s));
    }

    #[test]
    fn test_sign() {
        let data = "1;/nix/store/02bfycjg1607gpcnsg8l13lc45qa8qj3-libssh2-1.10.0;sha256:1l29f8r5q2739wnq4i7m2v545qx77b3wrdsw9xz2ajiy3hv1al8b;294664;/nix/store/02bfycjg1607gpcnsg8l13lc45qa8qj3-libssh2-1.10.0,/nix/store/1l4r0r4ab3v3a3ppir4jwiah3icalk9d-zlib-1.2.11,/nix/store/gf6j3k1flnhayvpnwnhikkg0s5dxrn1i-openssl-1.1.1l,/nix/store/z56jcx3j1gfyk4sv7g8iaan0ssbdkhz1-glibc-2.33-56";
        let sk_s = "cache.example.org-1:ZJui+kG6vPCSRD4+p1P4DyUVlASmp/zsaeN84PTFW28tj2/PtQWvFWK6Mw+ay8kGif8AZkR5KosHLvuwlzDlgg==";
        let sk: SecretKey = sk_s.parse().unwrap();
        let pk_s = "cache.example.org-1:LY9vz7UFrxViujMPmsvJBon/AGZEeSqLBy77sJcw5YI=";
        let pk: PublicKey = pk_s.parse().unwrap();

        let s = sk.sign(data);
        assert!(pk.verify(data, &s));
    }
}
