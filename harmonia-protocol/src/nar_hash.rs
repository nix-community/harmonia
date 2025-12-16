//! NAR hash type - a SHA256 hash specifically for NAR archives.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(any(test, feature = "test"))]
use proptest_derive::Arbitrary;

use harmonia_utils_hash::fmt::{self, CommonHash};
use harmonia_utils_hash::{Algorithm, InvalidHashError, Sha256};

/// A NAR hash - always SHA256.
#[derive(Clone, Copy, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
#[repr(transparent)]
pub struct NarHash(Sha256);

impl NarHash {
    pub const fn new(digest: &[u8]) -> NarHash {
        NarHash(Sha256::new(digest))
    }

    pub fn from_slice(digest: &[u8]) -> Result<NarHash, InvalidHashError> {
        Sha256::from_slice(digest).map(NarHash)
    }

    pub fn digest<D: AsRef<[u8]>>(data: D) -> Self {
        Self::new(&Algorithm::SHA256.digest(data))
    }

    #[inline]
    pub fn digest_bytes(&self) -> &[u8] {
        self.0.digest_bytes()
    }
}

impl From<NarHash> for harmonia_utils_hash::Hash {
    fn from(value: NarHash) -> Self {
        value.0.into()
    }
}

impl TryFrom<harmonia_utils_hash::Hash> for NarHash {
    type Error = fmt::ParseHashErrorKind;

    fn try_from(value: harmonia_utils_hash::Hash) -> Result<Self, Self::Error> {
        Ok(NarHash(value.try_into()?))
    }
}

impl Serialize for NarHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_sri().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for NarHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        fmt::SRI::<NarHash>::deserialize(deserializer).map(|sri| sri.into_hash())
    }
}

impl CommonHash for NarHash {
    #[inline]
    fn from_slice(algorithm: Algorithm, hash: &[u8]) -> Result<Self, fmt::ParseHashErrorKind> {
        if algorithm != Algorithm::SHA256 {
            return Err(fmt::ParseHashErrorKind::TypeMismatch {
                expected: Algorithm::SHA256,
                actual: algorithm,
            });
        }
        NarHash::from_slice(hash).map_err(From::from)
    }

    #[inline]
    fn implied_algorithm() -> Option<Algorithm> {
        Some(Algorithm::SHA256)
    }

    #[inline]
    fn algorithm(&self) -> Algorithm {
        Algorithm::SHA256
    }

    #[inline]
    fn digest_bytes(&self) -> &[u8] {
        self.0.digest_bytes()
    }
}

impl std::fmt::Debug for NarHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("NarHash")
            .field(&format_args!("{}", self.as_base16().as_bare()))
            .finish()
    }
}

impl std::fmt::LowerHex for NarHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for val in self.0.as_ref() {
            write!(f, "{val:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::UpperHex for NarHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for val in self.0.as_ref() {
            write!(f, "{val:02X}")?;
        }
        Ok(())
    }
}

// From implementations for format wrappers - these allow conversion between
// NarHash and its various encoding formats.
harmonia_utils_hash::impl_hash_format_from!(SRI<NarHash>);
harmonia_utils_hash::impl_hash_format_from!(Base16<NarHash>);
harmonia_utils_hash::impl_hash_format_from!(Base32<NarHash>);
harmonia_utils_hash::impl_hash_format_from!(Base64<NarHash>);
harmonia_utils_hash::impl_hash_format_from!(Any<NarHash>);
harmonia_utils_hash::impl_hash_format_from!(NonSRI<NarHash>);

impl AsRef<[u8]> for NarHash {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.digest_bytes()
    }
}
