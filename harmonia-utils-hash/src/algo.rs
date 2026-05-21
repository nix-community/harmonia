use std::str::FromStr;

use derive_more::Display;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::Hash;

const MD5_SIZE: usize = 128 / 8;
const SHA1_SIZE: usize = 160 / 8;
const SHA256_SIZE: usize = 256 / 8;
const SHA512_SIZE: usize = 512 / 8;
const BLAKE3_SIZE: usize = 256 / 8;

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
    #[display("blake3")]
    BLAKE3,
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
            Algorithm::BLAKE3 => BLAKE3_SIZE,
        }
    }

    /// Returns the digest of `data` using the given digest algorithm.
    ///
    /// ```
    /// # use harmonia_utils_hash::{Algorithm, HashFormat as _};
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
        } else if s.eq_ignore_ascii_case("blake3") {
            Ok(Algorithm::BLAKE3)
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

#[cfg(test)]
mod tests {
    use hex_literal::hex;
    use rstest::rstest;

    use super::*;
    use crate::Hash;
    use crate::view::HashView as _;
    use harmonia_utils_base_encoding::Base;

    /// value taken from: https://tools.ietf.org/html/rfc1321
    const MD5_EMPTY: Hash = Hash::new(Algorithm::MD5, &hex!("d41d8cd98f00b204e9800998ecf8427e"));
    /// value taken from: https://tools.ietf.org/html/rfc1321
    const MD5_ABC: Hash = Hash::new(Algorithm::MD5, &hex!("900150983cd24fb0d6963f7d28e17f72"));

    /// value taken from: https://tools.ietf.org/html/rfc3174
    const SHA1_ABC: Hash = Hash::new(
        Algorithm::SHA1,
        &hex!("a9993e364706816aba3e25717850c26c9cd0d89d"),
    );
    /// value taken from: https://tools.ietf.org/html/rfc3174
    const SHA1_LONG: Hash = Hash::new(
        Algorithm::SHA1,
        &hex!("84983e441c3bd26ebaae4aa1f95129e5e54670f1"),
    );

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA256_ABC: Hash = Hash::new(
        Algorithm::SHA256,
        &hex!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
    );
    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA256_LONG: Hash = Hash::new(
        Algorithm::SHA256,
        &hex!("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"),
    );

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA512_ABC: Hash = Hash::new(
        Algorithm::SHA512,
        &hex!(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        ),
    );
    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA512_LONG: Hash = Hash::new(
        Algorithm::SHA512,
        &hex!(
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"
        ),
    );

    /// value cross-checked against NixOS/nix PR #12379 BLAKE3 test vectors
    const BLAKE3_ABC: Hash = Hash::new(
        Algorithm::BLAKE3,
        &hex!("6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"),
    );
    /// value cross-checked against NixOS/nix PR #12379 BLAKE3 test vectors
    const BLAKE3_LONG: Hash = Hash::new(
        Algorithm::BLAKE3,
        &hex!("c19012cc2aaf0dc3d8e5c45a1b79114d2df42abb2a410bf54be09e891af06ff8"),
    );

    #[rstest]
    #[case::md5(Algorithm::MD5, 16, 32, 26, 24, 18)]
    #[case::sha1(Algorithm::SHA1, 20, 40, 32, 28, 21)]
    #[case::sha256(Algorithm::SHA256, 32, 64, 52, 44, 33)]
    #[case::sha512(Algorithm::SHA512, 64, 128, 103, 88, 66)]
    #[case::blake3(Algorithm::BLAKE3, 32, 64, 52, 44, 33)]
    fn algorithm_size(
        #[case] algorithm: Algorithm,
        #[case] size: usize,
        #[case] base16_len: usize,
        #[case] base32_len: usize,
        #[case] base64_len: usize,
        #[case] base64_decoded: usize,
    ) {
        assert_eq!(algorithm.size(), size, "mismatched size");
        assert_eq!(
            Base::Hex.input_len(algorithm.size()),
            base16_len,
            "mismatched base16_len"
        );
        assert_eq!(
            Base::NixBase32.input_len(algorithm.size()),
            base32_len,
            "mismatched base32_len"
        );
        assert_eq!(
            Base::Base64.input_len(algorithm.size()),
            base64_len,
            "mismatched base64_len"
        );
        assert_eq!(
            Base::Base64.scratch_len(algorithm.size()),
            base64_decoded,
            "mismatched base64_decoded"
        );
    }

    #[rstest]
    #[case::md5("md5", Algorithm::MD5)]
    #[case::sha1("sha1", Algorithm::SHA1)]
    #[case::sha256("sha256", Algorithm::SHA256)]
    #[case::sha512("sha512", Algorithm::SHA512)]
    #[case::blake3("blake3", Algorithm::BLAKE3)]
    #[case::md5_upper("MD5", Algorithm::MD5)]
    #[case::sha1_upper("SHA1", Algorithm::SHA1)]
    #[case::sha256_upper("SHA256", Algorithm::SHA256)]
    #[case::sha512_upper("SHA512", Algorithm::SHA512)]
    #[case::blake3_upper("BLAKE3", Algorithm::BLAKE3)]
    #[case::md5_mixed("mD5", Algorithm::MD5)]
    #[case::sha1_mixed("ShA1", Algorithm::SHA1)]
    #[case::sha256_mixed("ShA256", Algorithm::SHA256)]
    #[case::sha512_mixed("ShA512", Algorithm::SHA512)]
    #[case::blake3_mixed("BlAkE3", Algorithm::BLAKE3)]
    fn algorithm_from_str(#[case] input: &str, #[case] expected: Algorithm) {
        let actual = input.parse().unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn unknown_algorithm() {
        assert_eq!(
            Err(UnknownAlgorithm("test".into())),
            "test".parse::<Algorithm>()
        );
    }

    #[rstest]
    #[case::md5_empty(&MD5_EMPTY, "")]
    #[case::md5_abc(&MD5_ABC, "abc")]
    #[case::sha1_abc(&SHA1_ABC, "abc")]
    #[case::sha1_long(&SHA1_LONG, "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")]
    #[case::sha256_abc(&SHA256_ABC, "abc")]
    #[case::sha256_long(&SHA256_LONG, "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")]
    #[case::sha512_abc(&SHA512_ABC, "abc")]
    #[case::sha512_long(&SHA512_LONG, "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu")]
    #[case::blake3_abc(&BLAKE3_ABC, "abc")]
    #[case::blake3_long(&BLAKE3_LONG, "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")]
    fn test_digest(#[case] expected: &Hash, #[case] input: &str) {
        let actual = expected.algorithm().digest(input);
        assert_eq!(actual, *expected);
    }
}
