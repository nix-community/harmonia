#[cfg(any(test, feature = "test"))]
use proptest_derive::Arbitrary;

use crate::fmt;
use crate::owned::Hash;
use crate::view::HashView;
use crate::{Algorithm, InvalidHashError};

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub struct Sha256(pub(crate) [u8; Algorithm::SHA256.size()]);

impl Sha256 {
    pub const fn new(digest: &[u8]) -> Self {
        let mut data = [0u8; Algorithm::SHA256.size()];
        data.copy_from_slice(digest);
        Self(data)
    }

    pub const fn from_slice(digest: &[u8]) -> Result<Self, InvalidHashError> {
        if digest.len() != Algorithm::SHA256.size() {
            return Err(InvalidHashError {
                algorithm: Algorithm::SHA256,
                length: digest.len(),
            });
        }
        Ok(Self::new(digest))
    }

    /// Returns the digest of `data` using the sha256
    ///
    /// ```
    /// # use harmonia_utils_hash::Sha256;
    /// # use harmonia_utils_hash::fmt::HashFormat;
    /// let hash = Sha256::digest("abc");
    ///
    /// assert_eq!("1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s", hash.as_base32().as_bare().to_string());
    /// ```
    pub fn digest<B: AsRef<[u8]>>(data: B) -> Self {
        Algorithm::SHA256.digest(data).try_into().unwrap()
    }

    /// Returns a reference to the inner fixed-size array.
    #[inline]
    pub fn digest_bytes(&self) -> &[u8; Algorithm::SHA256.size()] {
        &self.0
    }
}

impl HashView for Sha256 {
    #[inline]
    fn algorithm(&self) -> Algorithm {
        Algorithm::SHA256
    }

    #[inline]
    fn digest_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8; Algorithm::SHA256.size()]> for Sha256 {
    fn as_ref(&self) -> &[u8; Algorithm::SHA256.size()] {
        self.digest_bytes()
    }
}

impl From<Sha256> for Hash {
    fn from(value: Sha256) -> Self {
        Hash::SHA256(value)
    }
}

impl TryFrom<Hash> for Sha256 {
    type Error = fmt::ParseHashErrorKind;

    fn try_from(value: Hash) -> Result<Self, Self::Error> {
        match value {
            Hash::SHA256(sha) => Ok(sha),
            other => Err(fmt::ParseHashErrorKind::TypeMismatch {
                expected: Algorithm::SHA256,
                actual: other.algorithm(),
            }),
        }
    }
}
