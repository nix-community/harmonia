#![allow(unsafe_code)]
// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-FileCopyrightText: 2026 John Ericson
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate as originally derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Cryptographic signing of Nix store paths (Ed25519 NAR signatures).

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use data_encoding::BASE64;

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

use harmonia_utils_base_encoding::{Base, base64_len};

pub const SIGNATURE_BYTES: usize = 64;
const SIGNATURE_BASE64_LEN: usize = base64_len(SIGNATURE_BYTES);
const SIGNATURE_BASE64_DECODED_LEN: usize = Base::Base64.scratch_len(SIGNATURE_BYTES);
pub const SEED_BYTES: usize = 32;
pub const PUBLIC_KEY_BYTES: usize = 32;
const PUBLIC_KEY_BASE64_LEN: usize = base64_len(PUBLIC_KEY_BYTES);
const PUBLIC_KEY_BASE64_DECODED_LEN: usize = Base::Base64.scratch_len(PUBLIC_KEY_BYTES);
pub const SECRET_KEY_BYTES: usize = SEED_BYTES + PUBLIC_KEY_BYTES;
const SECRET_KEY_BASE64_LEN: usize = base64_len(SECRET_KEY_BYTES);
const SECRET_KEY_BASE64_DECODED_LEN: usize = Base::Base64.scratch_len(SECRET_KEY_BYTES);

#[derive(Error, Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum ParseSignatureError {
    #[error("signature is corrupt")]
    CorruptSignature,
    #[error("signature is not valid")]
    InvalidSignature,
}

pub type SignatureSet = BTreeSet<Signature>;

/// Raw Ed25519 signature bytes with base64 Display/FromStr/serde.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct RawSignature(pub [u8; SIGNATURE_BYTES]);

#[inline]
fn encode_base64_str<'a, const N: usize>(bytes: &[u8], buf: &'a mut [u8; N]) -> &'a str {
    BASE64.encode_mut(bytes, buf);
    // SAFETY: Base64 is a subset of ASCII, which guarantees valid UTF-8.
    unsafe { std::str::from_utf8_unchecked(buf) }
}

#[inline]
fn decode_base64_fixed<const N: usize, const SCRATCH: usize>(
    s: &str,
    expected_len: usize,
    scratch: &mut [u8; SCRATCH],
) -> Option<[u8; N]> {
    if s.len() != expected_len {
        return None;
    }
    let len = BASE64.decode_mut(s.as_bytes(), scratch).ok()?;
    if len != N {
        return None;
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&scratch[..N]);
    Some(out)
}

impl fmt::Display for RawSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; SIGNATURE_BASE64_LEN];
        f.write_str(encode_base64_str(&self.0, &mut buf))
    }
}

impl FromStr for RawSignature {
    type Err = ParseSignatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut scratch = [0u8; SIGNATURE_BASE64_DECODED_LEN];
        let buf = decode_base64_fixed::<SIGNATURE_BYTES, _>(s, SIGNATURE_BASE64_LEN, &mut scratch)
            .ok_or(ParseSignatureError::InvalidSignature)?;
        Ok(RawSignature(buf))
    }
}

impl Serialize for RawSignature {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut buf = [0u8; SIGNATURE_BASE64_LEN];
        serializer.serialize_str(encode_base64_str(&self.0, &mut buf))
    }
}

impl<'de> Deserialize<'de> for RawSignature {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RawSigVisitor;

        impl Visitor<'_> for RawSigVisitor {
            type Value = RawSignature;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a base64-encoded Ed25519 signature")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<RawSignature, E> {
                v.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(RawSigVisitor)
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Signature {
    pub key_name: String,
    pub sig: RawSignature,
}

impl Signature {
    pub fn name(&self) -> &str {
        &self.key_name
    }

    pub fn signature(&self) -> &RawSignature {
        &self.sig
    }

    pub fn signature_bytes(&self) -> &[u8] {
        &self.sig.0
    }

    pub fn from_parts(name: &str, bytes: &[u8]) -> Result<Self, ParseSignatureError> {
        let buf: [u8; SIGNATURE_BYTES] = bytes
            .try_into()
            .map_err(|_| ParseSignatureError::InvalidSignature)?;
        Ok(Signature {
            key_name: name.to_string(),
            sig: RawSignature(buf),
        })
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.key_name, self.sig)
    }
}

/// JSON serialization matches upstream Nix `adl_serializer<Signature>`:
/// always writes the structured `{"keyName": ..., "sig": <base64>}` form,
/// accepts either that form or the legacy `"name:base64"` string when reading.
impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("Signature", 2)?;
        s.serialize_field("keyName", self.name())?;
        s.serialize_field("sig", self.signature())?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "camelCase")]
        enum Field {
            KeyName,
            Sig,
            #[serde(other)]
            Ignore,
        }

        struct SigVisitor;
        impl<'de> Visitor<'de> for SigVisitor {
            type Value = Signature;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a signature string or {keyName, sig} object")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Signature, E> {
                v.parse().map_err(E::custom)
            }
            fn visit_map<A>(self, mut map: A) -> Result<Signature, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut key_name: Option<String> = None;
                let mut sig: Option<RawSignature> = None;
                while let Some(field) = map.next_key::<Field>()? {
                    match field {
                        Field::KeyName => key_name = Some(map.next_value()?),
                        Field::Sig => sig = Some(map.next_value()?),
                        Field::Ignore => {
                            let _ignored: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let key_name = key_name.ok_or_else(|| de::Error::missing_field("keyName"))?;
                let sig = sig.ok_or_else(|| de::Error::missing_field("sig"))?;
                Ok(Signature { key_name, sig })
            }
        }
        deserializer.deserialize_any(SigVisitor)
    }
}

impl FromStr for Signature {
    type Err = ParseSignatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, sig_s) = s
            .split_once(':')
            .ok_or(ParseSignatureError::CorruptSignature)?;
        Ok(Signature {
            key_name: name.to_string(),
            sig: sig_s.parse()?,
        })
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
    name: Arc<str>,
    key_data: [u8; PUBLIC_KEY_BYTES],
    key: VerifyingKey,
}

impl PublicKey {
    pub fn verify<M: AsRef<[u8]>>(&self, data: M, signature: &Signature) -> bool {
        let message = data.as_ref();
        let sig = ed25519_dalek::Signature::from_bytes(&signature.sig.0);
        self.key.verify(message, &sig).is_ok()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> String {
        BASE64.encode(&self.key_data)
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
        let mut buf = [0u8; PUBLIC_KEY_BASE64_LEN];
        let key_str = encode_base64_str(&self.key_data, &mut buf);
        f.debug_struct("PublicKey")
            .field("name", &self.name)
            .field("key", &key_str)
            .finish()
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; PUBLIC_KEY_BASE64_LEN];
        write!(
            f,
            "{}:{}",
            self.name(),
            encode_base64_str(&self.key_data, &mut buf)
        )
    }
}

impl FromStr for PublicKey {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name_s, key_s) = s.split_once(':').ok_or(ParseKeyError::CorruptKey)?;
        let mut scratch = [0u8; PUBLIC_KEY_BASE64_DECODED_LEN];
        let key_data =
            decode_base64_fixed::<PUBLIC_KEY_BYTES, _>(key_s, PUBLIC_KEY_BASE64_LEN, &mut scratch)
                .ok_or(ParseKeyError::InvalidPublicKey)?;
        let key =
            VerifyingKey::from_bytes(&key_data).map_err(|_| ParseKeyError::InvalidPublicKey)?;
        Ok(PublicKey {
            name: Arc::from(name_s),
            key,
            key_data,
        })
    }
}

#[derive(Error, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[error("error generating key")]
pub struct GenerateKeyError;

pub struct SecretKey {
    name: Arc<str>,
    key_data: [u8; SECRET_KEY_BYTES],
    key: SigningKey,
}

impl SecretKey {
    pub fn generate(name: String) -> Result<SecretKey, GenerateKeyError> {
        let name = Arc::from(name);
        let mut seed = Zeroizing::new([0u8; SEED_BYTES]);
        getrandom::fill(&mut *seed).map_err(|_| GenerateKeyError)?;
        let key = SigningKey::from_bytes(&seed);
        let pk = key.verifying_key();
        let mut key_data = [0u8; SECRET_KEY_BYTES];
        key_data[0..SEED_BYTES].copy_from_slice(&*seed);
        key_data[SEED_BYTES..SECRET_KEY_BYTES].copy_from_slice(pk.as_bytes());
        Ok(SecretKey {
            name,
            key,
            key_data,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> String {
        BASE64.encode(&self.key_data)
    }

    pub fn sign<M: AsRef<[u8]>>(&self, data: M) -> Signature {
        let msg = data.as_ref();
        let sig = self.key.sign(msg);
        Signature {
            key_name: self.name.to_string(),
            sig: RawSignature(sig.to_bytes()),
        }
    }

    pub fn to_public_key(&self) -> PublicKey {
        let name = self.name.clone();
        let key = self.key.verifying_key();
        let key_data = key.to_bytes();
        PublicKey {
            name,
            key,
            key_data,
        }
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        // Wipe our copy of seed||pubkey; SigningKey zeroizes itself.
        self.key_data.zeroize();
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redacted: Debug output reaches logs/panics.
        f.debug_struct("SecretKey")
            .field("name", &self.name)
            .field("key", &"<redacted>")
            .finish()
    }
}
impl fmt::Display for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = Zeroizing::new([0u8; SECRET_KEY_BASE64_LEN]);
        write!(
            f,
            "{}:{}",
            self.name(),
            encode_base64_str(&self.key_data, &mut buf)
        )
    }
}

impl PartialEq for SecretKey {
    fn eq(&self, other: &Self) -> bool {
        // Constant-time on the secret bytes to avoid a timing oracle.
        self.name == other.name && bool::from(self.key_data.ct_eq(&other.key_data))
    }
}

impl Eq for SecretKey {}

impl<'a> From<&'a SecretKey> for PublicKey {
    fn from(v: &'a SecretKey) -> Self {
        v.to_public_key()
    }
}

impl FromStr for SecretKey {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name_s, key_s) = s.split_once(':').ok_or(ParseKeyError::CorruptKey)?;
        let mut scratch = Zeroizing::new([0u8; SECRET_KEY_BASE64_DECODED_LEN]);
        let key_data =
            decode_base64_fixed::<SECRET_KEY_BYTES, _>(key_s, SECRET_KEY_BASE64_LEN, &mut scratch)
                .ok_or(ParseKeyError::InvalidSecretKey)?;
        let mut seed = Zeroizing::new([0u8; SEED_BYTES]);
        seed.copy_from_slice(&key_data[0..SEED_BYTES]);
        let public_key = &key_data[SEED_BYTES..SECRET_KEY_BYTES];
        let key = SigningKey::from_bytes(&seed);
        if key.verifying_key().as_bytes() != public_key {
            return Err(ParseKeyError::InvalidSecretKey);
        }
        Ok(SecretKey {
            name: Arc::from(name_s),
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
        (arb_key_name(max), any::<[u8; SIGNATURE_BYTES]>()).prop_map(|(name, sig)| Signature {
            key_name: name,
            sig: RawSignature(sig),
        })
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

    /// `Debug` must redact key material so it can't leak via logs/panics.
    #[test]
    fn test_secret_key_debug_redacts_key() {
        let sk = SecretKey::generate("k".into()).unwrap();
        let dbg = format!("{sk:?}");
        assert!(!dbg.contains(&sk.key().to_string()), "leaked: {dbg}");
    }

    #[test]
    fn test_generate() {
        let sk_gen = SecretKey::generate("cache.example.org-1".into()).unwrap();
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
    fn test_serde_json_errors() {
        assert!(serde_json::from_value::<Signature>(serde_json::json!({"keyName": "x"})).is_err());
        assert!(serde_json::from_value::<Signature>(serde_json::json!({"sig": "x"})).is_err());
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

    proptest::proptest! {
        #[test]
        fn proptest_signature_display_parse(sig in proptest::prelude::any::<Signature>()) {
            let s = sig.to_string();
            proptest::prop_assert_eq!(s.parse::<Signature>().unwrap(), sig);
        }

        /// `Serialize` always emits the `{keyName, sig}` object; `Deserialize`
        /// must accept that, the legacy `"name:b64"` string, and ignore unknown
        /// fields.
        #[test]
        fn proptest_signature_json(sig in proptest::prelude::any::<Signature>()) {
            let json = serde_json::to_value(&sig).unwrap();
            proptest::prop_assert_eq!(json["keyName"].as_str().unwrap(), sig.name());
            proptest::prop_assert_eq!(serde_json::from_value::<Signature>(json).unwrap(), sig.clone());

            let legacy = serde_json::Value::String(sig.to_string());
            proptest::prop_assert_eq!(serde_json::from_value::<Signature>(legacy).unwrap(), sig.clone());

            let with_extra = serde_json::json!({
                "keyName": sig.name(),
                "sig": sig.signature().to_string(),
                "extra": 1,
            });
            proptest::prop_assert_eq!(serde_json::from_value::<Signature>(with_extra).unwrap(), sig);
        }
    }
}
