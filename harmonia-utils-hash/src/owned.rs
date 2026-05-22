use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::borrowed::BorrowedHash;
use crate::fmt::HashFormat as _;
use crate::view::HashView;
use crate::{Algorithm, InvalidHashError, Sha256, fmt};

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub enum Hash {
    MD5([u8; Algorithm::MD5.size()]),
    SHA1([u8; Algorithm::SHA1.size()]),
    SHA256(Sha256),
    SHA512([u8; Algorithm::SHA512.size()]),
    BLAKE3([u8; Algorithm::BLAKE3.size()]),
}

impl Hash {
    /// Create a hash from an algorithm and digest bytes.
    ///
    /// The input slice must be at least `algorithm.size()` bytes;
    /// trailing bytes beyond the algorithm's digest size are ignored.
    pub const fn new(algorithm: Algorithm, hash: &[u8]) -> Hash {
        /// Copy the first `N` bytes from `src` into a fixed-size array.
        const fn copy_into<const N: usize>(src: &[u8]) -> [u8; N] {
            let mut data = [0u8; N];
            let (src, _) = src.split_at(N);
            data.copy_from_slice(src);
            data
        }

        match algorithm {
            Algorithm::MD5 => Hash::MD5(copy_into(hash)),
            Algorithm::SHA1 => Hash::SHA1(copy_into(hash)),
            Algorithm::SHA256 => Hash::SHA256(Sha256(copy_into(hash))),
            Algorithm::SHA512 => Hash::SHA512(copy_into(hash)),
            Algorithm::BLAKE3 => Hash::BLAKE3(copy_into(hash)),
        }
    }

    pub fn from_slice(algorithm: Algorithm, hash: &[u8]) -> Result<Hash, InvalidHashError> {
        if hash.len() != algorithm.size() {
            return Err(InvalidHashError {
                algorithm,
                length: hash.len(),
            });
        }
        Ok(Hash::new(algorithm, hash))
    }

    /// Borrow this hash as a [`BorrowedHash`].
    pub fn borrow(&self) -> BorrowedHash<'_> {
        match self {
            Hash::MD5(d) => BorrowedHash::MD5(d),
            Hash::SHA1(d) => BorrowedHash::SHA1(d),
            Hash::SHA256(d) => BorrowedHash::SHA256(d),
            Hash::SHA512(d) => BorrowedHash::SHA512(d),
            Hash::BLAKE3(d) => BorrowedHash::BLAKE3(d),
        }
    }
}

impl HashView for Hash {
    #[inline]
    fn algorithm(&self) -> Algorithm {
        match self {
            Hash::MD5(_) => Algorithm::MD5,
            Hash::SHA1(_) => Algorithm::SHA1,
            Hash::SHA256(_) => Algorithm::SHA256,
            Hash::SHA512(_) => Algorithm::SHA512,
            Hash::BLAKE3(_) => Algorithm::BLAKE3,
        }
    }

    #[inline]
    fn digest_bytes(&self) -> &[u8] {
        match self {
            Hash::MD5(d) => d,
            Hash::SHA1(d) => d,
            Hash::SHA256(d) => &d.0,
            Hash::SHA512(d) => d,
            Hash::BLAKE3(d) => d,
        }
    }
}

impl std::ops::Deref for Hash {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.digest_bytes()
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        self.digest_bytes()
    }
}

impl Serialize for Hash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize as SRI string: "sha256-base64hash="
        serializer.serialize_str(&self.as_sri().to_string())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de;

        let s = String::deserialize(deserializer)?;
        // Try SRI format first, then fall back to Any format for flexibility
        s.parse::<fmt::SRI<Hash>>()
            .map(|sri| sri.into_hash())
            .or_else(|_| s.parse::<fmt::Any<Hash>>().map(|any| any.into_hash()))
            .map_err(de::Error::custom)
    }
}
