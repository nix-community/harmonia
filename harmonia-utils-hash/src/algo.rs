use std::str::FromStr;

use derive_more::Display;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::Hash;

const MD5_SIZE: usize = 128 / 8;
const SHA1_SIZE: usize = 160 / 8;
const SHA256_SIZE: usize = 256 / 8;
const SHA512_SIZE: usize = 512 / 8;

/// A digest algorithm.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, Display, Default)]
pub enum Algorithm {
    #[display("md5")]
    MD5,
    #[display("sha1")]
    SHA1,
    #[default]
    #[display("sha256")]
    SHA256,
    #[display("sha512")]
    SHA512,
}

impl Algorithm {
    /// The largest supported algorithm size in bytes
    pub(crate) const LARGEST: Algorithm = Algorithm::SHA512;

    /// Returns the size in bytes of this hash.
    #[inline]
    pub const fn size(&self) -> usize {
        match &self {
            Algorithm::MD5 => MD5_SIZE,
            Algorithm::SHA1 => SHA1_SIZE,
            Algorithm::SHA256 => SHA256_SIZE,
            Algorithm::SHA512 => SHA512_SIZE,
        }
    }

    /// Returns the digest of `data` using the given digest algorithm.
    ///
    /// ```
    /// # use harmonia_utils_hash::Algorithm;
    /// # use harmonia_utils_hash::fmt::CommonHash;
    /// let hash = Algorithm::SHA256.digest("abc");
    ///
    /// assert_eq!("sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s", hash.as_base32().to_string());
    /// ```
    pub fn digest<B: AsRef<[u8]>>(&self, data: B) -> Hash {
        let mut ctx = super::Context::new(*self);
        ctx.update(data);
        ctx.finish()
    }
}

#[derive(Error, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
#[error("unsupported digest algorithm '{0}'")]
pub struct UnknownAlgorithm(pub(super) String);

impl FromStr for Algorithm {
    type Err = UnknownAlgorithm;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("sha256") {
            Ok(Algorithm::SHA256)
        } else if s.eq_ignore_ascii_case("sha512") {
            Ok(Algorithm::SHA512)
        } else if s.eq_ignore_ascii_case("sha1") {
            Ok(Algorithm::SHA1)
        } else if s.eq_ignore_ascii_case("md5") {
            Ok(Algorithm::MD5)
        } else {
            Err(UnknownAlgorithm(s.to_owned()))
        }
    }
}

impl Serialize for Algorithm {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Algorithm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
