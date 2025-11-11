#![allow(unsafe_code)]

use std::{fmt as sfmt, str::FromStr};

use data_encoding::{BASE64, DecodeError, DecodeKind, HEXLOWER_PERMISSIVE};

use thiserror::Error;

use crate::base32;
use crate::hash;
use crate::hash::InvalidHashError;
use crate::hash::UnknownAlgorithm;

mod private {
    pub trait Sealed {}
}

#[derive(derive_more::Display, Debug, PartialEq, Clone)]
pub enum Base {
    #[display("hex")]
    Hex,
    #[display("nixbase32")]
    NixBase32,
    #[display("base64")]
    Base64,
}

#[derive(derive_more::Display, Debug, PartialEq, Clone)]
pub enum Encoding {
    #[display("sri")]
    Sri,
    #[display("{_0}")]
    NoAlgo(Base),
}

#[derive(derive_more::Display, Debug, PartialEq, Clone)]
pub enum ParseHashErrorKind {
    #[display("has {_0}")]
    Algorithm(hash::UnknownAlgorithm),
    #[display("is not SRI")]
    NotSRI,
    #[display("should have type '{expected}' but got '{actual}'")]
    TypeMismatch {
        expected: hash::Algorithm,
        actual: hash::Algorithm,
    },
    #[display("does not include a type, nor is the type otherwise known from context")]
    MissingType,
    #[display("has {_1} when decoding as {_0}")]
    BadEncoding(Encoding, data_encoding::DecodeError),
    #[display("has wrong length for hash type '{_0}'")]
    WrongHashLength(hash::Algorithm),
    #[display("has wrong length {length} != {} for hash type '{algorithm}'", algorithm.size())]
    WrongHashLength2 {
        algorithm: hash::Algorithm,
        length: usize,
    },
}

impl ParseHashErrorKind {
    fn adjust_position(&mut self, amt: usize) {
        match self {
            ParseHashErrorKind::BadEncoding(_, decode_error) => {
                decode_error.position += amt;
            }
            ParseHashErrorKind::WrongHashLength2 {
                algorithm: _,
                length,
            } => {
                *length += amt;
            }
            _ => {}
        }
    }

    fn adjust_encoding(&mut self, encoding: Encoding) {
        if let ParseHashErrorKind::BadEncoding(old_encoding, _) = self {
            *old_encoding = encoding;
        }
    }
}

#[derive(Error, Debug, PartialEq, Clone)]
#[error("hash '{hash}' {kind}")]
pub struct ParseHashError {
    hash: String,
    kind: ParseHashErrorKind,
}

impl ParseHashError {
    pub(crate) fn new<S: Into<String>>(hash: S, kind: ParseHashErrorKind) -> Self {
        ParseHashError {
            kind,
            hash: hash.into(),
        }
    }

    pub fn kind(&self) -> &ParseHashErrorKind {
        &self.kind
    }
}

impl From<InvalidHashError> for ParseHashErrorKind {
    fn from(value: InvalidHashError) -> Self {
        ParseHashErrorKind::WrongHashLength2 {
            algorithm: value.algorithm,
            length: value.length,
        }
    }
}

impl From<UnknownAlgorithm> for ParseHashErrorKind {
    fn from(value: UnknownAlgorithm) -> Self {
        Self::Algorithm(value)
    }
}

pub trait CommonHash: private::Sealed + Sized {
    fn from_slice(algorithm: hash::Algorithm, hash: &[u8]) -> Result<Self, ParseHashErrorKind>;
    fn implied_algorithm() -> Option<hash::Algorithm>;
    fn algorithm(&self) -> hash::Algorithm;
    fn digest_bytes(&self) -> &[u8];

    fn as_base16(&self) -> &Base16<Self>;
    fn as_base32(&self) -> &Base32<Self>;
    fn as_base64(&self) -> &Base64<Self>;
    fn as_sri(&self) -> &SRI<Self>;

    #[inline]
    fn base16(self) -> Base16<Self> {
        Base16(self)
    }

    #[inline]
    fn base32(self) -> Base32<Self> {
        Base32(self)
    }

    #[inline]
    fn base64(self) -> Base64<Self> {
        Base64(self)
    }

    #[inline]
    fn sri(self) -> SRI<Self> {
        SRI(self)
    }
}

impl hash::Hash {
    #[inline]
    pub fn as_base16(&self) -> &Base16<Self> {
        // SAFETY: `Hash` and `Base16<hash::Hash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base16<Self>) }
    }

    #[inline]
    pub const fn base16(self) -> Base16<Self> {
        Base16(self)
    }

    #[inline]
    pub fn as_base32(&self) -> &Base32<Self> {
        // SAFETY: `Hash` and `Base32<hash::Hash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base32<Self>) }
    }

    #[inline]
    pub const fn base32(self) -> Base32<Self> {
        Base32(self)
    }

    #[inline]
    pub fn as_base64(&self) -> &Base64<Self> {
        // SAFETY: `Hash` and `Base64` have the same ABI
        unsafe { &*(self as *const Self as *const Base64<Self>) }
    }

    #[inline]
    pub const fn base64(self) -> Base64<Self> {
        Base64(self)
    }

    #[inline]
    pub fn as_sri(&self) -> &SRI<Self> {
        // SAFETY: `Hash` and `SRIHash` have the same ABI
        unsafe { &*(self as *const hash::Hash as *const SRI<Self>) }
    }

    #[inline]
    pub const fn sri(self) -> SRI<Self> {
        SRI(self)
    }
}

impl private::Sealed for hash::Hash {}
impl CommonHash for hash::Hash {
    #[inline]
    fn from_slice(algorithm: hash::Algorithm, hash: &[u8]) -> Result<Self, ParseHashErrorKind> {
        Ok(hash::Hash::from_slice(algorithm, hash)?)
    }

    #[inline]
    fn implied_algorithm() -> Option<hash::Algorithm> {
        None
    }

    #[inline]
    fn algorithm(&self) -> hash::Algorithm {
        self.algorithm
    }

    #[inline]
    fn digest_bytes(&self) -> &[u8] {
        self.as_ref()
    }

    #[inline]
    fn as_base16(&self) -> &Base16<Self> {
        // SAFETY: `Hash` and `Base16<Hash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base16<Self>) }
    }

    #[inline]
    fn as_base32(&self) -> &Base32<Self> {
        // SAFETY: `Hash` and `Base32<Hash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base32<Self>) }
    }

    #[inline]
    fn as_base64(&self) -> &Base64<Self> {
        // SAFETY: `Hash` and `Base64<Hash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base64<Self>) }
    }

    #[inline]
    fn as_sri(&self) -> &SRI<Self> {
        // SAFETY: `Hash` and `SRI<Hash>` have the same ABI
        unsafe { &*(self as *const Self as *const SRI<Self>) }
    }
}

/*
impl FromStr for hash::Hash {
    type Err = hash::ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<Any<Self>>().map(From::from)
    }
}

impl sfmt::Display for hash::Hash {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        self.as_base32().fmt(f)
    }
}
*/

impl sfmt::Debug for hash::Hash {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        f.debug_struct("Hash")
            .field("algorithm", &self.algorithm)
            .field("data", &format_args!("{}", self.as_base32()))
            .finish()
    }
}

impl sfmt::LowerHex for hash::Hash {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        if !f.alternate() {
            write!(f, "{}:", self.algorithm())?;
        }
        for val in self.digest_bytes() {
            write!(f, "{val:02x}")?;
        }
        Ok(())
    }
}

impl sfmt::UpperHex for hash::Hash {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        if !f.alternate() {
            write!(f, "{}:", self.algorithm())?;
        }
        for val in self.digest_bytes() {
            write!(f, "{val:02X}")?;
        }
        Ok(())
    }
}

impl hash::Sha256 {
    #[inline]
    pub fn as_base16(&self) -> &Base16<Self> {
        // SAFETY: `Sha256` and `Base16<Sha256>` have the same ABI
        unsafe { &*(self as *const Self as *const Base16<Self>) }
    }

    #[inline]
    pub const fn base16(self) -> Base16<Self> {
        Base16(self)
    }

    #[inline]
    pub fn as_base32(&self) -> &Base32<Self> {
        // SAFETY: `Sha256` and `Base32<Sha256>` have the same ABI
        unsafe { &*(self as *const Self as *const Base32<Self>) }
    }

    #[inline]
    pub const fn base32(self) -> Base32<Self> {
        Base32(self)
    }

    #[inline]
    pub fn as_base64(&self) -> &Base64<Self> {
        // SAFETY: `Sha256` and `Base64<Sha256>` have the same ABI
        unsafe { &*(self as *const Self as *const Base64<Self>) }
    }

    #[inline]
    pub const fn base64(self) -> Base64<Self> {
        Base64(self)
    }
}

impl private::Sealed for hash::Sha256 {}
impl CommonHash for hash::Sha256 {
    #[inline]
    fn from_slice(algorithm: hash::Algorithm, hash: &[u8]) -> Result<Self, ParseHashErrorKind> {
        if algorithm != hash::Algorithm::SHA256 {
            return Err(ParseHashErrorKind::TypeMismatch {
                expected: hash::Algorithm::SHA256,
                actual: algorithm,
            });
        }
        hash::Sha256::from_slice(hash).map_err(From::from)
    }

    #[inline]
    fn implied_algorithm() -> Option<hash::Algorithm> {
        Some(hash::Algorithm::SHA256)
    }

    #[inline]
    fn algorithm(&self) -> hash::Algorithm {
        hash::Algorithm::SHA256
    }

    #[inline]
    fn digest_bytes(&self) -> &[u8] {
        self.as_ref()
    }

    #[inline]
    fn as_base16(&self) -> &Base16<Self> {
        // SAFETY: `Sha256` and `Base16<Sha256>` have the same ABI
        unsafe { &*(self as *const Self as *const Base16<Self>) }
    }

    #[inline]
    fn as_base32(&self) -> &Base32<Self> {
        // SAFETY: `Sha256` and `Base32<Sha256>` have the same ABI
        unsafe { &*(self as *const Self as *const Base32<Self>) }
    }

    #[inline]
    fn as_base64(&self) -> &Base64<Self> {
        // SAFETY: `Sha256` and `Base64<Sha256>` have the same ABI
        unsafe { &*(self as *const Self as *const Base64<Self>) }
    }

    #[inline]
    fn as_sri(&self) -> &SRI<Self> {
        // SAFETY: `Sha256` and `SRI<Sha256>` have the same ABI
        unsafe { &*(self as *const Self as *const SRI<Self>) }
    }
}

/*
impl FromStr for hash::Sha256 {
    type Err = hash::ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<Bare<NonSRI<Self>>>().map(From::from)
    }
}

impl sfmt::Display for hash::Sha256 {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        self.as_base32().as_bare().fmt(f)
    }
}
 */

impl sfmt::Debug for hash::Sha256 {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        f.debug_tuple("Sha256")
            .field(&format_args!("{}", self.as_base32().as_bare()))
            .finish()
    }
}

impl sfmt::LowerHex for hash::Sha256 {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        for val in self.digest_bytes() {
            write!(f, "{val:02x}")?;
        }
        Ok(())
    }
}

impl sfmt::UpperHex for hash::Sha256 {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        for val in self.digest_bytes() {
            write!(f, "{val:02X}")?;
        }
        Ok(())
    }
}

impl hash::NarHash {
    #[inline]
    pub fn as_base16(&self) -> &Base16<Self> {
        // SAFETY: `NarHash` and `Base16<NarHash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base16<Self>) }
    }

    #[inline]
    pub const fn base16(self) -> Base16<Self> {
        Base16(self)
    }

    #[inline]
    pub fn as_base32(&self) -> &Base32<Self> {
        // SAFETY: `NarHash` and `Base32<NarHash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base32<Self>) }
    }

    #[inline]
    pub const fn base32(self) -> Base32<Self> {
        Base32(self)
    }

    #[inline]
    pub fn as_base64(&self) -> &Base64<Self> {
        // SAFETY: `NarHash` and `Base64<NarHash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base64<Self>) }
    }

    #[inline]
    pub const fn base64(self) -> Base64<Self> {
        Base64(self)
    }
}

impl private::Sealed for hash::NarHash {}
impl CommonHash for hash::NarHash {
    #[inline]
    fn from_slice(algorithm: hash::Algorithm, hash: &[u8]) -> Result<Self, ParseHashErrorKind> {
        if algorithm != hash::Algorithm::SHA256 {
            return Err(ParseHashErrorKind::TypeMismatch {
                expected: hash::Algorithm::SHA256,
                actual: algorithm,
            });
        }
        hash::NarHash::from_slice(hash).map_err(From::from)
    }

    #[inline]
    fn implied_algorithm() -> Option<hash::Algorithm> {
        Some(hash::Algorithm::SHA256)
    }

    #[inline]
    fn algorithm(&self) -> hash::Algorithm {
        hash::Algorithm::SHA256
    }

    #[inline]
    fn digest_bytes(&self) -> &[u8] {
        self.0.digest_bytes()
    }

    #[inline]
    fn as_base16(&self) -> &Base16<Self> {
        // SAFETY: `NarHash` and `Base16<NarHash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base16<Self>) }
    }

    #[inline]
    fn as_base32(&self) -> &Base32<Self> {
        // SAFETY: `NarHash` and `Base32<NarHash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base32<Self>) }
    }

    #[inline]
    fn as_base64(&self) -> &Base64<Self> {
        // SAFETY: `NarHash` and `Base64<NarHash>` have the same ABI
        unsafe { &*(self as *const Self as *const Base64<Self>) }
    }

    #[inline]
    fn as_sri(&self) -> &SRI<Self> {
        // SAFETY: `NarHash` and `SRI<NarHash>` have the same ABI
        unsafe { &*(self as *const Self as *const SRI<Self>) }
    }
}

/*
impl FromStr for hash::NarHash {
    type Err = hash::ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<Bare<Base16<Self>>>().map(From::from)
    }
}

impl sfmt::Display for hash::NarHash {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        self.as_base16().as_bare().fmt(f)
    }
}
*/

impl sfmt::Debug for hash::NarHash {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        f.debug_tuple("NarHash")
            .field(&format_args!("{}", self.as_base16().as_bare()))
            .finish()
    }
}

impl sfmt::LowerHex for hash::NarHash {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        for val in self.0.as_ref() {
            write!(f, "{val:02x}")?;
        }
        Ok(())
    }
}

impl sfmt::UpperHex for hash::NarHash {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        for val in self.0.as_ref() {
            write!(f, "{val:02X}")?;
        }
        Ok(())
    }
}

pub trait Format: private::Sealed {
    type Hash;
    fn parse(algorithm: hash::Algorithm, s: &str) -> Result<Self::Hash, ParseHashError>;
    fn into_inner(self) -> Self::Hash;
    fn from_inner(inner: Self::Hash) -> Self;
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Base16<H>(H);
impl<H: CommonHash + Sized> Base16<H> {
    pub const fn from_hash(hash: H) -> Self {
        Self(hash)
    }

    pub const fn as_hash(&self) -> &H {
        &self.0
    }

    /// Consumes the [`Base16`], returning the underlying [`Hash`](hash::Hash).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use harmonia_store_core::hash;
    ///
    /// let hex = hash::Algorithm::SHA256.digest(b"Hello World!").base16();
    /// assert_eq!(hex.into_hash(), hash::Algorithm::SHA256.digest(b"Hello World!"));
    /// ```
    #[inline]
    pub fn into_hash(self) -> H {
        self.0
    }

    #[inline]
    pub fn as_bare(&self) -> &Bare<Self> {
        // SAFETY: `Base16<H>` and `Bare<Base16<H>>` have the same ABI
        unsafe { &*(self as *const Self as *const Bare<Self>) }
    }

    #[inline]
    pub fn bare(self) -> Bare<Self> {
        Bare(self)
    }
}
impl<H: CommonHash> Base16<H> {
    pub fn parse(algorithm: hash::Algorithm, s: &str) -> Result<H, ParseHashError> {
        <Self as Format>::parse(algorithm, s)
    }
}
impl<H> private::Sealed for Base16<H> {}
impl<H: CommonHash> Format for Base16<H> {
    type Hash = H;

    fn parse(algorithm: hash::Algorithm, s: &str) -> Result<Self::Hash, ParseHashError> {
        if s.len() != algorithm.base16_len() {
            return Err(ParseHashError::new(
                s,
                ParseHashErrorKind::WrongHashLength(algorithm),
            ));
        }
        let mut hash = [0u8; hash::LARGEST_ALGORITHM.size()];
        HEXLOWER_PERMISSIVE
            .decode_mut(s.as_bytes(), &mut hash[..algorithm.size()])
            .map_err(|err| {
                ParseHashError::new(
                    s,
                    ParseHashErrorKind::BadEncoding(Encoding::NoAlgo(Base::Hex), err.error),
                )
            })?;
        H::from_slice(algorithm, &hash[..algorithm.size()])
            .map_err(|kind| ParseHashError::new(s, kind))
    }

    fn into_inner(self) -> Self::Hash {
        self.0
    }

    fn from_inner(inner: Self::Hash) -> Self {
        Self(inner)
    }
}

impl<H: CommonHash> sfmt::Display for Base16<H> {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        if !f.alternate() {
            write!(f, "{}:", self.0.algorithm())?;
        }
        for val in self.0.digest_bytes() {
            write!(f, "{val:02x}")?;
        }
        Ok(())
    }
}

/// Parse base16 (hexidecimal) prefixed hash
///
/// These have the format `<type>:<base16>`,
impl<H: CommonHash> FromStr for Base16<H> {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((prefix, rest)) = s.split_once(':') {
            let algorithm = prefix
                .parse::<hash::Algorithm>()
                .map_err(|err| ParseHashError::new(s, err.into()))?;
            Self::parse(algorithm, rest).map(Self).map_err(|mut err| {
                err.hash = s.into();
                err.kind.adjust_position(prefix.len() + 1);
                err
            })
        } else {
            Err(ParseHashError::new(s, ParseHashErrorKind::MissingType))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Base32<H>(H);
impl<H: CommonHash + Sized> Base32<H> {
    pub const fn from_hash(hash: H) -> Self {
        Self(hash)
    }

    pub const fn as_hash(&self) -> &H {
        &self.0
    }

    /// Consumes the [`Base32`], returning the underlying [`Hash`](hash::Hash).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use harmonia_store_core::hash;
    ///
    /// let base32 = hash::Algorithm::SHA256.digest(b"Hello World!").base32();
    /// assert_eq!(base32.into_hash(), hash::Algorithm::SHA256.digest(b"Hello World!"));
    /// ```
    #[inline]
    pub fn into_hash(self) -> H {
        self.0
    }

    #[inline]
    pub fn as_bare(&self) -> &Bare<Self> {
        // SAFETY: `Base32<H>` and `Bare<Base32<H>>` have the same ABI
        unsafe { &*(self as *const Self as *const Bare<Self>) }
    }

    #[inline]
    pub fn bare(self) -> Bare<Self> {
        Bare(self)
    }
}
impl<H: CommonHash> Base32<H> {
    pub fn parse(algorithm: hash::Algorithm, s: &str) -> Result<H, ParseHashError> {
        <Self as Format>::parse(algorithm, s)
    }
}
impl<H> private::Sealed for Base32<H> {}
impl<H: CommonHash> Format for Base32<H> {
    type Hash = H;

    fn parse(algorithm: hash::Algorithm, s: &str) -> Result<Self::Hash, ParseHashError> {
        if s.len() != algorithm.base32_len() {
            return Err(ParseHashError::new(
                s,
                ParseHashErrorKind::WrongHashLength(algorithm),
            ));
        }
        let mut hash = [0u8; hash::LARGEST_ALGORITHM.size()];
        base32::decode_mut(s.as_bytes(), &mut hash[..algorithm.size()]).map_err(|err| {
            ParseHashError::new(
                s,
                ParseHashErrorKind::BadEncoding(Encoding::NoAlgo(Base::NixBase32), err.error),
            )
        })?;
        H::from_slice(algorithm, &hash[..algorithm.size()])
            .map_err(|kind| ParseHashError::new(s, kind))
    }

    fn into_inner(self) -> Self::Hash {
        self.0
    }

    fn from_inner(inner: Self::Hash) -> Self {
        Self(inner)
    }
}

impl<H: CommonHash> sfmt::Display for Base32<H> {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        let mut buf = [0u8; hash::LARGEST_ALGORITHM.base32_len()];
        let output = &mut buf[..self.0.algorithm().base32_len()];
        base32::encode_mut(self.0.digest_bytes(), output);

        // SAFETY: Nix Base32 is a subset of ASCII, which guarantees valid UTF-8.
        let s = unsafe { std::str::from_utf8_unchecked(output) };
        if f.alternate() {
            f.write_str(s)
        } else {
            write!(f, "{}:{}", self.0.algorithm(), s)
        }
    }
}

/// Parse nixbase32 prefixed hash
///
/// These have the format `<type>:<base32>`,
impl<H: CommonHash> FromStr for Base32<H> {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((prefix, rest)) = s.split_once(':') {
            let algorithm = prefix
                .parse::<hash::Algorithm>()
                .map_err(|err| ParseHashError::new(s, err.into()))?;
            Self::parse(algorithm, rest).map(Self).map_err(|mut err| {
                err.hash = s.into();
                err.kind.adjust_position(prefix.len() + 1);
                err
            })
        } else {
            Err(ParseHashError::new(s, ParseHashErrorKind::MissingType))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Base64<H>(H);
impl<H: CommonHash> Base64<H> {
    pub const fn from_hash(hash: H) -> Self {
        Self(hash)
    }

    pub const fn as_hash(&self) -> &H {
        &self.0
    }

    /// Consumes the [`Base64`], returning the underlying [`Hash`](hash::Hash).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use harmonia_store_core::hash;
    ///
    /// let base64 = hash::Algorithm::SHA256.digest(b"Hello World!").base64();
    /// assert_eq!(base64.into_hash(), hash::Algorithm::SHA256.digest(b"Hello World!"));
    /// ```
    pub fn into_hash(self) -> H {
        self.0
    }

    #[inline]
    pub fn as_bare(&self) -> &Bare<Self> {
        // SAFETY: `Base64<H>` and `Bare<Base64<H>>` have the same ABI
        unsafe { &*(self as *const Self as *const Bare<Self>) }
    }

    #[inline]
    pub fn bare(self) -> Bare<Self> {
        Bare(self)
    }
}
impl<H: CommonHash> Base64<H> {
    pub fn parse(algorithm: hash::Algorithm, s: &str) -> Result<H, ParseHashError> {
        <Self as Format>::parse(algorithm, s)
    }
}
impl<H: CommonHash> private::Sealed for Base64<H> {}
impl<H: CommonHash> Format for Base64<H> {
    type Hash = H;

    fn parse(algorithm: hash::Algorithm, s: &str) -> Result<Self::Hash, ParseHashError> {
        if s.len() != algorithm.base64_len() {
            return Err(ParseHashError::new(
                s,
                ParseHashErrorKind::WrongHashLength(algorithm),
            ));
        }
        let mut hash = [0u8; hash::LARGEST_ALGORITHM.base64_decoded()];
        let len = BASE64
            .decode_mut(s.as_bytes(), &mut hash[..algorithm.base64_decoded()])
            .map_err(|err| {
                ParseHashError::new(
                    s,
                    ParseHashErrorKind::BadEncoding(Encoding::NoAlgo(Base::Base64), err.error),
                )
            })?;
        if len != algorithm.size() {
            Err(ParseHashError::new(
                s,
                ParseHashErrorKind::BadEncoding(
                    Encoding::NoAlgo(Base::Base64),
                    DecodeError {
                        position: 0,
                        kind: DecodeKind::Length,
                    },
                ),
            ))
        } else {
            H::from_slice(algorithm, &hash[..algorithm.size()])
                .map_err(|kind| ParseHashError::new(s, kind))
        }
    }

    fn into_inner(self) -> Self::Hash {
        self.0
    }

    fn from_inner(inner: Self::Hash) -> Self {
        Self(inner)
    }
}

impl<H: CommonHash> sfmt::Display for Base64<H> {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        let mut buf = [0u8; hash::LARGEST_ALGORITHM.base64_len()];
        let output = &mut buf[..self.0.algorithm().base64_len()];
        BASE64.encode_mut(self.0.digest_bytes(), output);

        // SAFETY: Bas64 is a subset of ASCII, which guarantees valid UTF-8.
        let s = unsafe { std::str::from_utf8_unchecked(output) };

        if f.alternate() {
            f.write_str(s)
        } else {
            write!(f, "{}:{s}", self.0.algorithm())
        }
    }
}

/// Parse base64 prefixed hash
///
/// These have the format `<type>:<base64>`,
impl<H: CommonHash> FromStr for Base64<H> {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((prefix, rest)) = s.split_once(':') {
            let algorithm = prefix
                .parse::<hash::Algorithm>()
                .map_err(|err| ParseHashError::new(s, err.into()))?;
            Self::parse(algorithm, rest).map(Self).map_err(|mut err| {
                err.hash = s.into();
                err.kind.adjust_position(prefix.len() + 1);
                err
            })
        } else {
            Err(ParseHashError::new(s, ParseHashErrorKind::MissingType))
        }
    }
}

/// Parse the hash from a string representation.
///
/// This will parse a hash in the format
/// `[<type>:]<base16|base32|base64>` or `<type>-<base64>` (a
/// Subresource Integrity hash expression).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct Any<H>(H);
impl<H> Any<H> {
    pub const fn as_hash(&self) -> &H {
        &self.0
    }

    pub fn into_hash(self) -> H {
        self.0
    }
}
impl<H: CommonHash> Any<H> {
    pub fn parse(algorithm: hash::Algorithm, s: &str) -> Result<H, ParseHashError> {
        <Self as Format>::parse(algorithm, s)
    }
}
impl<H> private::Sealed for Any<H> {}
impl<H: CommonHash> Format for Any<H> {
    type Hash = H;

    fn parse(algorithm: hash::Algorithm, s: &str) -> Result<Self::Hash, ParseHashError> {
        if s.len() == algorithm.base16_len() {
            Ok(Base16::parse(algorithm, s)?)
        } else if s.len() == algorithm.base32_len() {
            Ok(Base32::parse(algorithm, s)?)
        } else if s.len() == algorithm.base64_len() {
            Ok(Base64::parse(algorithm, s)?)
        } else {
            Err(ParseHashError::new(
                s,
                ParseHashErrorKind::WrongHashLength(algorithm),
            ))
        }
    }

    fn into_inner(self) -> Self::Hash {
        self.0
    }

    fn from_inner(inner: Self::Hash) -> Self {
        Any(inner)
    }
}

impl<H: CommonHash> FromStr for Any<H> {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((prefix, rest)) = s.split_once(':') {
            let algorithm = prefix
                .parse::<hash::Algorithm>()
                .map_err(|err| ParseHashError::new(s, err.into()))?;
            let hash = Self::parse(algorithm, rest).map_err(|mut err| {
                err.hash = s.into();
                err.kind.adjust_position(prefix.len() + 1);
                err
            })?;
            Ok(Self(hash))
        } else if let Some((prefix, rest)) = s.split_once('-') {
            let algorithm = prefix
                .parse::<hash::Algorithm>()
                .map_err(|err| ParseHashError::new(s, err.into()))?;
            if rest.len() == algorithm.base64_len() {
                let hash = Base64::parse(algorithm, rest).map_err(|mut err| {
                    err.hash = s.into();
                    err.kind.adjust_position(prefix.len() + 1);
                    err.kind.adjust_encoding(Encoding::Sri);
                    err
                })?;
                Ok(Self(hash))
            } else {
                Err(ParseHashError::new(
                    s,
                    ParseHashErrorKind::WrongHashLength(algorithm),
                ))
            }
        } else if let Some(algorithm) = H::implied_algorithm() {
            Ok(Self(Self::parse(algorithm, s)?))
        } else {
            Err(ParseHashError::new(s, ParseHashErrorKind::MissingType))
        }
    }
}

/// Subresource Integrity hash expression.
///
/// These have the format `<type>-<base64>`,
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct SRI<H>(H);
impl<H: CommonHash + Sized> SRI<H> {
    pub const fn from_hash(hash: H) -> Self {
        Self(hash)
    }

    pub const fn as_hash(&self) -> &H {
        &self.0
    }

    /// Consumes the [`SRI`], returning the underlying [`Hash`](hash::Hash).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use harmonia_store_core::hash;
    ///
    /// let sri = hash::Algorithm::SHA256.digest(b"Hello World!").sri();
    /// assert_eq!(sri.into_hash(), hash::Algorithm::SHA256.digest(b"Hello World!"));
    /// ```
    pub fn into_hash(self) -> H {
        self.0
    }
}

impl<H: CommonHash> sfmt::Display for SRI<H> {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        write!(f, "{}-{}", self.0.algorithm(), self.0.as_base64().as_bare())
    }
}

/// Parse Subresource Integrity hash expression.
///
/// These have the format `<type>-<base64>`,
impl<H: CommonHash> FromStr for SRI<H> {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((prefix, rest)) = s.split_once('-') {
            let algorithm = prefix
                .parse::<hash::Algorithm>()
                .map_err(|err| ParseHashError::new(s, err.into()))?;
            Base64::parse(algorithm, rest).map(Self).map_err(|mut err| {
                err.hash = s.into();
                err.kind.adjust_position(prefix.len() + 1);
                err.kind.adjust_encoding(Encoding::Sri);
                err
            })
        } else {
            Err(ParseHashError::new(s, ParseHashErrorKind::NotSRI))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct NonSRI<H>(H);
impl<H> NonSRI<H> {
    pub const fn as_hash(&self) -> &H {
        &self.0
    }

    pub fn into_hash(self) -> H {
        self.0
    }
}
impl<H: CommonHash> NonSRI<H> {
    pub fn parse(algorithm: hash::Algorithm, s: &str) -> Result<H, ParseHashError> {
        <Self as Format>::parse(algorithm, s)
    }
}
impl<H> private::Sealed for NonSRI<H> {}
impl<H: CommonHash> Format for NonSRI<H> {
    type Hash = H;

    fn parse(algorithm: hash::Algorithm, s: &str) -> Result<Self::Hash, ParseHashError> {
        if s.len() == algorithm.base16_len() {
            Ok(Base16::parse(algorithm, s)?)
        } else if s.len() == algorithm.base32_len() {
            Ok(Base32::parse(algorithm, s)?)
        } else if s.len() == algorithm.base64_len() {
            Ok(Base64::parse(algorithm, s)?)
        } else {
            Err(ParseHashError::new(
                s,
                ParseHashErrorKind::WrongHashLength(algorithm),
            ))
        }
    }

    fn into_inner(self) -> Self::Hash {
        self.0
    }

    fn from_inner(inner: Self::Hash) -> Self {
        Self(inner)
    }
}

impl<H: CommonHash> FromStr for NonSRI<H> {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((prefix, rest)) = s.split_once(':') {
            let algorithm = prefix
                .parse::<hash::Algorithm>()
                .map_err(|err| ParseHashError::new(s, err.into()))?;
            let hash = Self::parse(algorithm, rest).map_err(|mut err| {
                err.hash = s.into();
                err.kind.adjust_position(prefix.len() + 1);
                err
            })?;
            Ok(Self(hash))
        } else if let Some(algorithm) = H::implied_algorithm() {
            Ok(Self(Self::parse(algorithm, s)?))
        } else {
            Err(ParseHashError::new(s, ParseHashErrorKind::MissingType))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Bare<F>(F);
impl<F> sfmt::Display for Bare<F>
where
    F: sfmt::Display,
{
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl<F> FromStr for Bare<F>
where
    F: Format,
    <F as Format>::Hash: CommonHash,
{
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(algorithm) = <<F as Format>::Hash as CommonHash>::implied_algorithm() {
            Ok(Bare(F::from_inner(F::parse(algorithm, s)?)))
        } else {
            Err(ParseHashError::new(s, ParseHashErrorKind::MissingType))
        }
    }
}

macro_rules! impl_fmt_from {
    ($T:ident<>) => {
        impl_fmt_from!($T, hash::Hash);
    };
    ($T:ident<$IT:path>) => {
        impl_fmt_from!($T<$IT>, $IT);
    };
    ($T:path, $FT:path) => {
        impl From<$FT> for $T {
            #[inline]
            fn from(value: $FT) -> Self {
                $T(value)
            }
        }

        impl From<$T> for $FT {
            #[inline]
            fn from(value: $T) -> Self {
                value.0
            }
        }

        impl AsRef<$FT> for $T {
            #[inline]
            fn as_ref(&self) -> &$FT {
                &self.0
            }
        }

        impl std::borrow::Borrow<$FT> for $T {
            #[inline]
            fn borrow(&self) -> &$FT {
                &self.0
            }
        }

        impl AsRef<[u8]> for $T {
            #[inline]
            fn as_ref(&self) -> &[u8] {
                self.0.digest_bytes()
            }
        }
    };
}

impl_fmt_from!(hash::NarHash, hash::Sha256);
impl_fmt_from!(SRI<hash::Hash>);
impl_fmt_from!(SRI<hash::NarHash>);
impl_fmt_from!(SRI<hash::Sha256>);

macro_rules! impl_format_from {
    ($T:ident<$IT:path>) => {
        impl_format_from!($T<$IT>, $IT);
    };
    ($T:ident<$($IT:path)?>, $FT:path) => {
        impl_fmt_from!($T<$($IT)?>, $FT);

        impl From<$FT> for Bare<$T<$($IT)?>> {
            #[inline]
            fn from(value: $FT) -> Self {
                Bare($T(value))
            }
        }

        impl From<Bare<$T<$($IT)?>>> for $FT {
            #[inline]
            fn from(value: Bare<$T<$($IT)?>>) -> Self {
                value.0.0
            }
        }

        impl AsRef<$FT> for Bare<$T<$($IT)?>> {
            #[inline]
            fn as_ref(&self) -> &$FT {
                &self.0.0
            }
        }

        impl std::borrow::Borrow<$FT> for Bare<$T<$($IT)?>> {
            #[inline]
            fn borrow(&self) -> &$FT {
                &self.0.0
            }
        }

        impl AsRef<[u8]> for Bare<$T<$($IT)?>> {
            #[inline]
            fn as_ref(&self) -> &[u8] {
                self.0.as_ref()
            }
        }
    };
}

impl_format_from!(Base64<hash::Hash>);
impl_format_from!(Base64<hash::NarHash>);
impl_format_from!(Base64<hash::Sha256>);
impl_format_from!(Base32<hash::Hash>);
impl_format_from!(Base32<hash::NarHash>);
impl_format_from!(Base32<hash::Sha256>);
impl_format_from!(Base16<hash::Hash>);
impl_format_from!(Base16<hash::NarHash>);
impl_format_from!(Base16<hash::Sha256>);
impl_format_from!(Any<hash::Hash>);
impl_format_from!(Any<hash::NarHash>);
impl_format_from!(Any<hash::Sha256>);
impl_format_from!(NonSRI<hash::Hash>);
impl_format_from!(NonSRI<hash::NarHash>);
impl_format_from!(NonSRI<hash::Sha256>);

#[cfg(test)]
mod unittests {
    use hex_literal::hex;

    use super::*;
    use crate::hash::{Algorithm, Hash, NarHash};

    struct HashFormats {
        hash: Hash,
        algorithm: &'static str,
        base16: &'static str,
        base32: &'static str,
        base64: &'static str,
    }

    impl HashFormats {
        pub fn prefix_base16(&self) -> String {
            format!("{}:{}", self.algorithm, self.base16)
        }
        pub fn prefix_base32(&self) -> String {
            format!("{}:{}", self.algorithm, self.base32)
        }
        pub fn prefix_base64(&self) -> String {
            format!("{}:{}", self.algorithm, self.base64)
        }
        pub fn sri(&self) -> String {
            format!("{}-{}", self.algorithm, self.base64)
        }
    }

    /// value taken from: https://tools.ietf.org/html/rfc1321
    const MD5_EMPTY: HashFormats = HashFormats {
        hash: Hash::new(Algorithm::MD5, &hex!("d41d8cd98f00b204e9800998ecf8427e")),
        algorithm: "md5",
        base16: "d41d8cd98f00b204e9800998ecf8427e",
        base32: "3y8bwfr609h3lh9ch0izcqq7fl",
        base64: "1B2M2Y8AsgTpgAmY7PhCfg==",
    };

    /// value taken from: https://tools.ietf.org/html/rfc1321
    const MD5_ABC: HashFormats = HashFormats {
        hash: Hash::new(Algorithm::MD5, &hex!("900150983cd24fb0d6963f7d28e17f72")),
        algorithm: "md5",
        base16: "900150983cd24fb0d6963f7d28e17f72",
        base32: "3jgzhjhz9zjvbb0kyj7jc500ch",
        base64: "kAFQmDzST7DWlj99KOF/cg==",
    };

    /// value taken from: https://tools.ietf.org/html/rfc3174
    const SHA1_ABC: HashFormats = HashFormats {
        hash: Hash::new(
            Algorithm::SHA1,
            &hex!("a9993e364706816aba3e25717850c26c9cd0d89d"),
        ),
        algorithm: "sha1",
        base16: "a9993e364706816aba3e25717850c26c9cd0d89d",
        base32: "kpcd173cq987hw957sx6m0868wv3x6d9",
        base64: "qZk+NkcGgWq6PiVxeFDCbJzQ2J0=",
    };

    /// value taken from: https://tools.ietf.org/html/rfc3174
    const SHA1_LONG: HashFormats = HashFormats {
        hash: Hash::new(
            Algorithm::SHA1,
            &hex!("84983e441c3bd26ebaae4aa1f95129e5e54670f1"),
        ),
        algorithm: "sha1",
        base16: "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
        base32: "y5q4drg5558zk8aamsx6xliv3i23x644",
        base64: "hJg+RBw70m66rkqh+VEp5eVGcPE=",
    };

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA256_ABC: HashFormats = HashFormats {
        hash: Hash::new(
            Algorithm::SHA256,
            &hex!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        ),
        algorithm: "sha256",
        base16: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        base32: "1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s",
        base64: "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=",
    };

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA256_LONG: HashFormats = HashFormats {
        hash: Hash::new(
            Algorithm::SHA256,
            &hex!("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"),
        ),
        algorithm: "sha256",
        base16: "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        base32: "1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394",
        base64: "JI1qYdIGOLjlwCaTDD5gOaM85Flk/yFn9uzt1BnbBsE=",
    };

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA512_ABC: HashFormats = HashFormats {
        hash: Hash::new(
            Algorithm::SHA512,
            &hex!(
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
            ),
        ),
        algorithm: "sha512",
        base16: "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        base32: "2gs8k559z4rlahfx0y688s49m2vvszylcikrfinm30ly9rak69236nkam5ydvly1ai7xac99vxfc4ii84hawjbk876blyk1jfhkbbyx",
        base64: "3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw==",
    };

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA512_LONG: HashFormats = HashFormats {
        hash: Hash::new(
            Algorithm::SHA512,
            &hex!(
                "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"
            ),
        ),
        algorithm: "sha512",
        base16: "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909",
        base32: "04yjjw7bgjrcpjl4vfvdvi9sg3klhxmqkg9j6rkwkvh0jcy50fm064hi2vavblrfahpz7zbqrwpg3rz2ky18a7pyj6dl4z3v9srp5cf",
        base64: "jpWbddrjE9qM9PcoFPwUP493ecbrn3+hcpmurbaIkBhQHSieSQD35DMbmd7EtUM6x9Mp7rbdJlReluVbh0vpCQ==",
    };

    #[rstest_reuse::template]
    #[rstest::rstest]
    #[case::md5_empty(MD5_EMPTY)]
    #[case::md5_abc(MD5_ABC)]
    #[case::sha1_abc(SHA1_ABC)]
    #[case::sha1_long(SHA1_LONG)]
    #[case::sha256_abc(SHA256_ABC)]
    #[case::sha256_long(SHA256_LONG)]
    #[case::sha512_abc(SHA512_ABC)]
    #[case::sha512_long(SHA512_LONG)]
    fn hash_formats(#[case] hash: HashFormats) {}

    #[rstest_reuse::apply(hash_formats)]
    fn lower_hex(#[case] hash: HashFormats) {
        let actual = format!("{:x}", hash.hash);
        assert_eq!(hash.prefix_base16(), actual);
    }

    #[rstest_reuse::apply(hash_formats)]
    fn lower_hex_alt(#[case] hash: HashFormats) {
        let actual = format!("{:#x}", hash.hash);
        assert_eq!(hash.base16, actual);
    }

    #[rstest_reuse::apply(hash_formats)]
    fn upper_hex(#[case] hash: HashFormats) {
        let expected = format!("{}:{}", hash.algorithm, hash.base16.to_uppercase());
        let actual = format!("{:X}", hash.hash);
        assert_eq!(expected, actual);
    }

    #[rstest_reuse::apply(hash_formats)]
    fn upper_hex_alt(#[case] hash: HashFormats) {
        let actual = format!("{:#X}", hash.hash);
        assert_eq!(hash.base16.to_uppercase(), actual);
    }

    mod base16 {
        use rstest::rstest;

        use super::*;

        #[rstest_reuse::apply(hash_formats)]
        fn hash_eq(#[case] hash: HashFormats) {
            let actual = hash.hash.base16();
            assert_eq!(*hash.hash.as_base16(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_bare_eq(#[case] hash: HashFormats) {
            let actual = hash.hash.base16().bare();
            assert_eq!(*hash.hash.as_base16().as_bare(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display(#[case] hash: HashFormats) {
            let actual = hash.hash.base16().to_string();
            assert_eq!(hash.prefix_base16(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_alt(#[case] hash: HashFormats) {
            let actual = format!("{:#}", hash.hash.base16());
            assert_eq!(hash.base16, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_bare(#[case] hash: HashFormats) {
            let actual = hash.hash.base16().bare().to_string();
            assert_eq!(hash.base16, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_bare_alt(#[case] hash: HashFormats) {
            let actual = format!("{:#}", hash.hash.base16().bare());
            assert_eq!(hash.base16, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str(#[case] hash: HashFormats) {
            let s = hash.prefix_base16();
            let actual = s.parse::<Base16<Hash>>().unwrap();
            assert_eq!(*hash.hash.as_base16(), actual);
        }

        // UnknownAlgorithm
        // Base16 bad symbol
        // WrongHashLength
        // MissingType
        #[rstest]
        #[should_panic = "hash 'sha25:a9993e364706816aba3e25717850c26c9cd0d89d' has unsupported digest algorithm 'sha25'"]
        #[case::unknown_algorithm("sha25:a9993e364706816aba3e25717850c26c9cd0d89d")]
        #[should_panic = "hash 'sha1:Ka9993e364706816aba3e25717850c26c9cd0d89' has invalid symbol at 5 when decoding as hex"]
        #[case::bad_symbol("sha1:Ka9993e364706816aba3e25717850c26c9cd0d89")]
        #[should_panic = "hash 'sha1:a9993e364706816aba3e25717850c26c9cd0d89' has wrong length for hash type 'sha1'"]
        #[case::wrong_length("sha1:a9993e364706816aba3e25717850c26c9cd0d89")]
        #[should_panic = "hash 'a9993e364706816aba3e25717850c26c9cd0d89d' does not include a type, nor is the type otherwise known from context"]
        #[case::missing_type("a9993e364706816aba3e25717850c26c9cd0d89d")]
        fn hash_from_str_error(#[case] input: &str) {
            let actual = input.parse::<Base16<Hash>>().unwrap_err();
            panic!("{actual}");
        }

        // TypeMismatch
        #[rstest]
        #[should_panic = "hash 'sha1:a9993e364706816aba3e25717850c26c9cd0d89d' should have type 'sha256' but got 'sha1'"]
        #[case::type_mismatch("sha1:a9993e364706816aba3e25717850c26c9cd0d89d")]
        fn nar_hash_from_str_error(#[case] input: &str) {
            let actual = input.parse::<Base16<NarHash>>().unwrap_err();
            panic!("{actual}");
        }
    }

    mod base32 {
        use super::*;

        #[rstest_reuse::apply(hash_formats)]
        fn hash_eq(#[case] hash: HashFormats) {
            let actual = hash.hash.base32();
            assert_eq!(*hash.hash.as_base32(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_bare_eq(#[case] hash: HashFormats) {
            let actual = hash.hash.base32().bare();
            assert_eq!(*hash.hash.as_base32().as_bare(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display(#[case] hash: HashFormats) {
            let actual = hash.hash.base32().to_string();
            assert_eq!(hash.prefix_base32(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_alt(#[case] hash: HashFormats) {
            let actual = format!("{:#}", hash.hash.base32());
            assert_eq!(hash.base32, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_bare(#[case] hash: HashFormats) {
            let actual = hash.hash.base32().bare().to_string();
            assert_eq!(hash.base32, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_bare_alt(#[case] hash: HashFormats) {
            let actual = format!("{:#}", hash.hash.base32().bare());
            assert_eq!(hash.base32, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str(#[case] hash: HashFormats) {
            let s = hash.prefix_base32();
            let actual = s.parse::<Base32<Hash>>().unwrap();
            assert_eq!(*hash.hash.as_base32(), actual);
        }
    }

    mod base64 {
        use super::*;

        #[rstest_reuse::apply(hash_formats)]
        fn hash_eq(#[case] hash: HashFormats) {
            let actual = hash.hash.base64();
            assert_eq!(*hash.hash.as_base64(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_bare_eq(#[case] hash: HashFormats) {
            let actual = hash.hash.base64().bare();
            assert_eq!(*hash.hash.as_base64().as_bare(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display(#[case] hash: HashFormats) {
            let actual = hash.hash.base64().to_string();
            assert_eq!(hash.prefix_base64(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_alt(#[case] hash: HashFormats) {
            let actual = format!("{:#}", hash.hash.base64());
            assert_eq!(hash.base64, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_bare(#[case] hash: HashFormats) {
            let actual = hash.hash.base64().bare().to_string();
            assert_eq!(hash.base64, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_bare_alt(#[case] hash: HashFormats) {
            let actual = format!("{:#}", hash.hash.base64().bare());
            assert_eq!(hash.base64, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str(#[case] hash: HashFormats) {
            let s = hash.prefix_base64();
            let actual = s.parse::<Base64<Hash>>().unwrap();
            assert_eq!(*hash.hash.as_base64(), actual);
        }
    }

    mod sri {
        use rstest::rstest;

        use super::*;

        #[rstest_reuse::apply(hash_formats)]
        fn hash_eq(#[case] hash: HashFormats) {
            let actual = hash.hash.sri();
            assert_eq!(*hash.hash.as_sri(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display(#[case] hash: HashFormats) {
            let actual = format!("{}", hash.hash.sri());
            assert_eq!(hash.sri(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_display_alt(#[case] hash: HashFormats) {
            let actual = format!("{:#}", hash.hash.sri());
            assert_eq!(hash.sri(), actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str(#[case] hash: HashFormats) {
            let s = hash.sri();
            let actual = s.parse::<SRI<Hash>>().unwrap();
            assert_eq!(*hash.hash.as_sri(), actual);
        }

        // UnknownAlgorithm
        // Base16 bad symbol
        // WrongHashLength
        // MissingType
        #[rstest]
        #[should_panic = "hash 'sha1:qZk+NkcGgWq6PiVxeFDCbJzQ2J0=' is not SRI"]
        #[case::not_sri("sha1:qZk+NkcGgWq6PiVxeFDCbJzQ2J0=")]
        #[should_panic = "hash 'sha25-qZk+NkcGgWq6PiVxeFDCbJzQ2J0=' has unsupported digest algorithm 'sha25'"]
        #[case::unknown_algorithm("sha25-qZk+NkcGgWq6PiVxeFDCbJzQ2J0=")]
        #[should_panic = "hash 'sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2Å=' has invalid symbol at 30 when decoding as sri"]
        #[case::bad_symbol("sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2Å=")]
        #[should_panic = "hash 'sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2JJ0=' has wrong length for hash type 'sha1'"]
        #[case::wrong_length("sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2JJ0=")]
        #[should_panic = "hash 'sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2J0a' has invalid length at 5 when decoding as sri"]
        #[case::wrong_length2("sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2J0a")]
        #[should_panic = "hash 'qZk+NkcGgWq6PiVxeFDCbJzQ2J0=' is not SRI"]
        #[case::missing_type("qZk+NkcGgWq6PiVxeFDCbJzQ2J0=")]
        fn hash_from_str_error(#[case] input: &str) {
            let actual = input.parse::<SRI<Hash>>().unwrap_err();
            panic!("{actual}");
        }

        #[rstest]
        #[should_panic = "hash 'sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2J0=' should have type 'sha256' but got 'sha1'"]
        #[case::type_mismatch("sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2J0=")]
        fn nar_hash_from_str_error(#[case] input: &str) {
            let actual = input.parse::<SRI<NarHash>>().unwrap_err();
            panic!("{actual}");
        }
    }

    mod any {
        use rstest::rstest;

        use super::*;

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str_base16(#[case] hash: HashFormats) {
            let s = hash.prefix_base16();
            let actual = s.parse::<Any<hash::Hash>>().unwrap().into_hash();
            assert_eq!(hash.hash, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str_base32(#[case] hash: HashFormats) {
            let s = hash.prefix_base32();
            let actual = s.parse::<Any<hash::Hash>>().unwrap().into_hash();
            assert_eq!(hash.hash, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str_base64(#[case] hash: HashFormats) {
            let s = hash.prefix_base64();
            let actual = s.parse::<Any<hash::Hash>>().unwrap().into_hash();
            assert_eq!(hash.hash, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str_sri(#[case] hash: HashFormats) {
            let s = hash.sri();
            let actual = s.parse::<Any<hash::Hash>>().unwrap().into_hash();
            assert_eq!(hash.hash, actual);
        }

        #[rstest]
        #[should_panic = "hash 'sha1:k9993e364706816aba3e25717850c26c9cd0d89d' has invalid symbol at 5 when decoding as hex"]
        #[case::bad_hex_symbol("sha1:k9993e364706816aba3e25717850c26c9cd0d89d")]
        #[should_panic = "hash 'sha1:!pcd173cq987hw957sx6m0868wv3x6d9' has invalid symbol at 5 when decoding as nixbase32"]
        #[case::bad_nixbase32_symbol("sha1:!pcd173cq987hw957sx6m0868wv3x6d9")]
        #[should_panic = "hash 'sha1:!Zk+NkcGgWq6PiVxeFDCbJzQ2J0=' has invalid symbol at 5 when decoding as base64"]
        #[case::bad_base64_symbol("sha1:!Zk+NkcGgWq6PiVxeFDCbJzQ2J0=")]
        #[should_panic = "hash 'sha1:qZk+NkcGgWq6PiVxeFDCbJzQ2J0a' has invalid length at 5 when decoding as base64"]
        #[case::bad_base64_length("sha1:qZk+NkcGgWq6PiVxeFDCbJzQ2J0a")]
        #[should_panic = "hash 'sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2J0a' has invalid length at 5 when decoding as sri"]
        #[case::bad_sri_length("sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2J0a")]
        #[should_panic = "hash 'sha1:12345' has wrong length for hash type 'sha1'"]
        #[case::wrong_length("sha1:12345")]
        #[should_panic = "hash 'a9993e364706816aba3e25717850c26c9cd0d89d' does not include a type, nor is the type otherwise known from context"]
        #[case::prefix_missing("a9993e364706816aba3e25717850c26c9cd0d89d")]
        #[should_panic = "hash 'sha25:12345' has unsupported digest algorithm 'sha25'"]
        #[case::unknown_algorithm("sha25:12345")]
        fn hash_from_str_error(#[case] input: &str) {
            let actual = input.parse::<Any<Hash>>().unwrap_err();
            panic!("{actual}");
        }

        #[rstest]
        #[should_panic = "hash 'sha1:kpcd173cq987hw957sx6m0868wv3x6d9' should have type 'sha256' but got 'sha1'"]
        #[case::type_mismatch("sha1:kpcd173cq987hw957sx6m0868wv3x6d9")]
        fn nar_hash_from_str_error(#[case] input: &str) {
            let actual = input.parse::<Any<NarHash>>().unwrap_err();
            panic!("{actual}");
        }
    }

    mod non_sri {
        use super::*;

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str_base16(#[case] hash: HashFormats) {
            let s = hash.prefix_base16();
            let actual = s.parse::<NonSRI<hash::Hash>>().unwrap().into_hash();
            assert_eq!(hash.hash, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str_base32(#[case] hash: HashFormats) {
            let s = hash.prefix_base32();
            let actual = s.parse::<NonSRI<hash::Hash>>().unwrap().into_hash();
            assert_eq!(hash.hash, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn hash_from_str_base64(#[case] hash: HashFormats) {
            let s = hash.prefix_base64();
            let actual = s.parse::<NonSRI<hash::Hash>>().unwrap().into_hash();
            assert_eq!(hash.hash, actual);
        }

        #[test]
        fn parse_non_sri_prefixed_missing() {
            assert_eq!(
                Err(ParseHashError::new(
                    "12345",
                    ParseHashErrorKind::MissingType
                )),
                "12345".parse::<NonSRI<Hash>>()
            );
        }
    }

    /*
    mod hash_from_str {
        use super::*;

        #[rstest_reuse::apply(hash_formats)]
        fn base16(#[case] hash: HashFormats) {
            let s = hash.prefix_base16();
            let actual = s.parse().unwrap();
            assert_eq!(hash.hash, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn base32(#[case] hash: HashFormats) {
            let s = hash.prefix_base32();
            let actual = s.parse().unwrap();
            assert_eq!(hash.hash, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn base64(#[case] hash: HashFormats) {
            let s = hash.prefix_base64();
            let actual = s.parse().unwrap();
            assert_eq!(hash.hash, actual);
        }

        #[rstest_reuse::apply(hash_formats)]
        fn sri(#[case] hash: HashFormats) {
            let s = hash.sri();
            let actual = s.parse().unwrap();
            assert_eq!(hash.hash, actual);
        }
    }
     */
}
