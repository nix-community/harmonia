use crate::owned::Hash;
use crate::view::HashView;
use crate::{Algorithm, Sha256};

/// A borrowed view of a hash digest.
///
/// Each variant borrows a fixed-size array, so the slice is
/// statically guaranteed to be the right length for its algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowedHash<'a> {
    MD5(&'a [u8; Algorithm::MD5.size()]),
    SHA1(&'a [u8; Algorithm::SHA1.size()]),
    SHA256(&'a Sha256),
    SHA512(&'a [u8; Algorithm::SHA512.size()]),
    BLAKE3(&'a [u8; Algorithm::BLAKE3.size()]),
}

impl<'a> BorrowedHash<'a> {
    /// Convert to an owned [`Hash`].
    pub fn to_owned(self) -> Hash {
        match self {
            BorrowedHash::MD5(d) => Hash::MD5(*d),
            BorrowedHash::SHA1(d) => Hash::SHA1(*d),
            BorrowedHash::SHA256(d) => Hash::SHA256(*d),
            BorrowedHash::SHA512(d) => Hash::SHA512(*d),
            BorrowedHash::BLAKE3(d) => Hash::BLAKE3(*d),
        }
    }

    #[inline]
    fn algorithm(self) -> Algorithm {
        match self {
            BorrowedHash::MD5(_) => Algorithm::MD5,
            BorrowedHash::SHA1(_) => Algorithm::SHA1,
            BorrowedHash::SHA256(_) => Algorithm::SHA256,
            BorrowedHash::SHA512(_) => Algorithm::SHA512,
            BorrowedHash::BLAKE3(_) => Algorithm::BLAKE3,
        }
    }

    /// Returns the raw digest bytes with the original data lifetime,
    /// not tied to `&self`.
    #[inline]
    pub fn digest_bytes(self) -> &'a [u8] {
        match self {
            BorrowedHash::MD5(d) => d,
            BorrowedHash::SHA1(d) => d,
            BorrowedHash::SHA256(d) => d.digest_bytes(),
            BorrowedHash::SHA512(d) => d,
            BorrowedHash::BLAKE3(d) => d,
        }
    }
}

impl HashView for BorrowedHash<'_> {
    #[inline]
    fn algorithm(&self) -> Algorithm {
        (*self).algorithm()
    }

    #[inline]
    fn digest_bytes(&self) -> &[u8] {
        (*self).digest_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn borrow_roundtrip(hash in any::<Hash>()) {
            let borrowed = hash.borrow();
            prop_assert_eq!(borrowed.algorithm(), hash.algorithm());
            prop_assert_eq!(borrowed.digest_bytes(), hash.digest_bytes());
            prop_assert_eq!(borrowed.to_owned(), hash);
        }
    }
}
