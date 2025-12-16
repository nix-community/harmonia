use std::convert::TryInto;
use std::fmt as sfmt;

#[cfg(any(test, feature = "test"))]
use proptest_derive::Arbitrary;
use ring::digest;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

mod algo;
pub mod fmt;

pub use algo::{Algorithm, UnknownAlgorithm};

const LARGEST_ALGORITHM: Algorithm = Algorithm::LARGEST;

#[derive(Error, Debug, PartialEq, Eq, Clone, Copy)]
#[error("hash has wrong length {length} != {} for hash type '{algorithm}'", algorithm.size())]
pub struct InvalidHashError {
    algorithm: Algorithm,
    length: usize,
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct Hash {
    algorithm: Algorithm,
    data: [u8; LARGEST_ALGORITHM.size()],
}

impl Hash {
    pub const fn new(algorithm: Algorithm, hash: &[u8]) -> Hash {
        let mut data = [0u8; LARGEST_ALGORITHM.size()];
        let (hash_data, _postfix) = data.split_at_mut(algorithm.size());
        hash_data.copy_from_slice(hash);
        Hash { algorithm, data }
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

    #[inline]
    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }

    #[inline]
    pub fn digest_bytes(&self) -> &[u8] {
        &self.data[0..(self.algorithm.size())]
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

impl TryFrom<digest::Digest> for Hash {
    type Error = UnknownAlgorithm;
    fn try_from(digest: digest::Digest) -> Result<Self, Self::Error> {
        Ok(Hash::new(digest.algorithm().try_into()?, digest.as_ref()))
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

impl From<NarHash> for Hash {
    fn from(value: NarHash) -> Self {
        value.0.into()
    }
}

impl TryFrom<Hash> for NarHash {
    type Error = fmt::ParseHashErrorKind;

    fn try_from(value: Hash) -> Result<Self, Self::Error> {
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

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub struct Sha256([u8; Algorithm::SHA256.size()]);
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
    /// let hash = Sha256::digest("abc");
    ///
    /// assert_eq!("1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s", hash.as_base32().as_bare().to_string());
    /// ```
    pub fn digest<B: AsRef<[u8]>>(data: B) -> Self {
        Algorithm::SHA256.digest(data).try_into().unwrap()
    }

    #[inline]
    pub fn digest_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for Sha256 {
    fn as_ref(&self) -> &[u8] {
        self.digest_bytes()
    }
}

impl From<Sha256> for Hash {
    fn from(value: Sha256) -> Self {
        Hash::new(Algorithm::SHA256, value.as_ref())
    }
}

impl TryFrom<Hash> for Sha256 {
    type Error = fmt::ParseHashErrorKind;

    fn try_from(value: Hash) -> Result<Self, Self::Error> {
        if value.algorithm() != Algorithm::SHA256 {
            return Err(fmt::ParseHashErrorKind::TypeMismatch {
                expected: Algorithm::SHA256,
                actual: value.algorithm(),
            });
        }
        Ok(Self::new(value.as_ref()))
    }
}

#[derive(Clone)]
enum InnerContext {
    MD5(md5::Context),
    Ring(digest::Context),
}

/// A context for multi-step (Init-Update-Finish) digest calculation.
///
/// # Examples
///
/// ```
/// use harmonia_utils_hash as hash;
///
/// let one_shot = hash::Algorithm::SHA256.digest("hello, world");
///
/// let mut ctx = hash::Context::new(hash::Algorithm::SHA256);
/// ctx.update("hello");
/// ctx.update(", ");
/// ctx.update("world");
/// let multi_path = ctx.finish();
///
/// assert_eq!(one_shot, multi_path);
/// ```
#[derive(Clone)]
pub struct Context(Algorithm, InnerContext);

impl Context {
    /// Constructs a new context with `algorithm`.
    pub fn new(algorithm: Algorithm) -> Self {
        match algorithm {
            Algorithm::MD5 => Context(algorithm, InnerContext::MD5(md5::Context::new())),
            _ => Context(
                algorithm,
                InnerContext::Ring(digest::Context::new(algorithm.digest_algorithm())),
            ),
        }
    }

    /// Update the digest with all the data in `data`.
    /// `update` may be called zero or more times before `finish` is called.
    pub fn update<D: AsRef<[u8]>>(&mut self, data: D) {
        let data = data.as_ref();
        match &mut self.1 {
            InnerContext::MD5(ctx) => ctx.consume(data),
            InnerContext::Ring(ctx) => ctx.update(data),
        }
    }

    /// Finalizes the digest calculation and returns the [`Hash`] value.
    /// This consumes the context to prevent misuse.
    ///
    /// [`Hash`]: struct@Hash
    pub fn finish(self) -> Hash {
        match self.1 {
            InnerContext::MD5(ctx) => Hash::new(self.0, ctx.finalize().as_ref()),
            InnerContext::Ring(ctx) => ctx.finish().try_into().unwrap(),
        }
    }

    /// The algorithm that this context is using.
    pub fn algorithm(&self) -> Algorithm {
        self.0
    }
}

impl sfmt::Debug for Context {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        f.debug_tuple("Context").field(&self.0).finish()
    }
}

/// A hash sink that implements [`AsyncWrite`].
///
/// # Examples
///
/// ```
/// use tokio::io;
/// use harmonia_utils_hash as hash;
///
/// # #[tokio::main]
/// # async fn main() -> std::io::Result<()> {
/// let mut reader: &[u8] = b"hello, world";
/// let mut sink = hash::HashSink::new(hash::Algorithm::SHA256);
///
/// io::copy(&mut reader, &mut sink).await?;
/// let (size, hash) = sink.finish();
///
/// let one_shot = hash::Algorithm::SHA256.digest("hello, world");
/// assert_eq!(one_shot, hash);
/// assert_eq!(12, size);
/// # Ok(())
/// # }
/// ```
///
/// [`AsyncWrite`]: tokio::io::AsyncWrite
#[derive(Debug)]
pub struct HashSink(Option<(u64, Context)>);
impl HashSink {
    /// Constructs a new sink with `algorithm`.
    pub fn new(algorithm: Algorithm) -> HashSink {
        HashSink(Some((0, Context::new(algorithm))))
    }

    /// Finalizes this sink and returns the hash and number of bytes written to the sink.
    pub fn finish(self) -> (u64, Hash) {
        let (read, ctx) = self.0.unwrap();
        (read, ctx.finish())
    }
}

impl tokio::io::AsyncWrite for HashSink {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        match self.0.as_mut() {
            None => {
                return std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "cannot write to HashSink after calling finish()",
                )));
            }
            Some((read, ctx)) => {
                *read += buf.len() as u64;
                ctx.update(buf)
            }
        }
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[cfg(any(test, feature = "test"))]
mod proptests {
    use super::*;
    use ::proptest::prelude::*;

    impl Arbitrary for Algorithm {
        type Parameters = ();
        type Strategy = BoxedStrategy<Algorithm>;
        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            prop_oneof![
                1 => Just(Algorithm::MD5),
                2 => Just(Algorithm::SHA1),
                5 => Just(Algorithm::SHA256),
                2 => Just(Algorithm::SHA512)
            ]
            .boxed()
        }
    }

    impl Arbitrary for Hash {
        type Parameters = Algorithm;
        type Strategy = BoxedStrategy<Hash>;

        fn arbitrary_with(algorithm: Self::Parameters) -> Self::Strategy {
            any_hash(algorithm).boxed()
        }
    }

    prop_compose! {
        fn any_hash(algorithm: Algorithm)
                   (data in any::<Vec<u8>>()) -> Hash
        {
            algorithm.digest(data)
        }
    }
}

#[cfg(test)]
mod unittests {
    use hex_literal::hex;
    use rstest::rstest;

    use super::*;
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

    #[rstest]
    #[case::md5(Algorithm::MD5, 16, 32, 26, 24, 18)]
    #[case::sha1(Algorithm::SHA1, 20, 40, 32, 28, 21)]
    #[case::sha256(Algorithm::SHA256, 32, 64, 52, 44, 33)]
    #[case::sha512(Algorithm::SHA512, 64, 128, 103, 88, 66)]
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
    #[case::md5_upper("MD5", Algorithm::MD5)]
    #[case::sha1_upper("SHA1", Algorithm::SHA1)]
    #[case::sha256_upper("SHA256", Algorithm::SHA256)]
    #[case::sha512_upper("SHA512", Algorithm::SHA512)]
    #[case::md5_mixed("mD5", Algorithm::MD5)]
    #[case::sha1_mixed("ShA1", Algorithm::SHA1)]
    #[case::sha256_mixed("ShA256", Algorithm::SHA256)]
    #[case::sha512_mixed("ShA512", Algorithm::SHA512)]
    fn algorithm_from_str(#[case] input: &str, #[case] expected: Algorithm) {
        let actual = input.parse().unwrap();
        assert_eq!(expected, actual);
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
    fn test_digest(#[case] expected: &Hash, #[case] input: &str) {
        let actual = expected.algorithm().digest(input);
        assert_eq!(actual, *expected);
    }

    #[test]
    fn unknown_algorithm() {
        assert_eq!(
            Err(UnknownAlgorithm("test".into())),
            "test".parse::<Algorithm>()
        );
    }

    #[test]
    fn unknown_digest() {
        assert_eq!(
            Err(UnknownAlgorithm("SHA384".into())),
            Algorithm::try_from(&digest::SHA384)
        );
    }

    #[rstest]
    #[case::sha256(&SHA256_ABC, "sha256-ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=")]
    #[case::sha1(&SHA1_ABC, "sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2J0=")]
    #[case::md5(&MD5_ABC, "md5-kAFQmDzST7DWlj99KOF/cg==")]
    #[case::sha512(&SHA512_ABC, "sha512-3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw==")]
    fn test_serde_hash_sri(#[case] hash: &Hash, #[case] sri_str: &str) {
        // Test serialization - should produce SRI string
        let serialized = serde_json::to_value(hash).unwrap();
        assert_eq!(serialized.as_str().unwrap(), sri_str);

        // Test deserialization from SRI string
        let deserialized: Hash = serde_json::from_value(serialized).unwrap();
        assert_eq!(&deserialized, hash);
    }

    #[test]
    fn test_serde_hash_invalid() {
        let json = serde_json::json!("invalid-hash-string");
        let result: Result<Hash, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }
}
