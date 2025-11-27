#![allow(unsafe_code)]

use std::borrow::Cow;
use std::fmt;
use std::hash as std_hash;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::base32;
use crate::hash;

use super::FromStoreDirStr;
use super::StoreDir;
use super::StoreDirDisplay;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorePath {
    hash: StorePathHash,
    name: StorePathName,
}

fn strip_store<'s>(s: &'s str, store_dir: &StoreDir) -> Result<&'s str, StorePathError> {
    let path = Path::new(s);
    if !path.is_absolute() {
        return Err(StorePathError::NonAbsolute(path.to_owned()));
    }
    let name = s
        .strip_prefix(store_dir.to_str())
        .ok_or_else(|| StorePathError::NotInStore(path.into()))?;
    if name.as_bytes()[0] != b'/' {
        return Err(StorePathError::NotInStore(path.into()));
    }

    Ok(&name[1..])
}

impl StorePath {
    fn new(s: &str, store_dir: &StoreDir) -> Result<Self, ParseStorePathError> {
        let name = strip_store(s, store_dir).map_err(|error| ParseStorePathError {
            path: s.to_owned(),
            error,
        })?;
        name.parse::<Self>().map_err(|error| ParseStorePathError {
            path: s.to_owned(),
            error: error.error,
        })
    }

    pub fn from_bytes(buf: &[u8]) -> Result<Self, StorePathError> {
        if buf.len() < STORE_PATH_HASH_ENCODED_SIZE + 1 {
            return Err(StorePathError::HashLength);
        }
        if buf[STORE_PATH_HASH_ENCODED_SIZE] != b'-' {
            return Err(StorePathError::Symbol(
                STORE_PATH_HASH_ENCODED_SIZE as u8,
                buf[STORE_PATH_HASH_ENCODED_SIZE],
            ));
        }
        let hash = StorePathHash::decode_digest(&buf[..STORE_PATH_HASH_ENCODED_SIZE])?;
        let name = buf[(STORE_PATH_HASH_ENCODED_SIZE + 1)..]
            .try_into()
            .map_err(|err| {
                StorePathError::adjust_index(STORE_PATH_HASH_ENCODED_SIZE as u8 + 1, err)
            })?;
        Ok(StorePath { hash, name })
    }

    pub fn from_hash(hash: &hash::Sha256, name: StorePathName) -> Self {
        StorePath {
            hash: StorePathHash::new_from_hash(hash),
            name,
        }
    }

    pub fn name(&self) -> &StorePathName {
        &self.name
    }

    pub fn hash(&self) -> &StorePathHash {
        &self.hash
    }

    /// Convert to base path (just the hash-name part, without store directory)
    pub fn to_base_path(&self) -> String {
        self.to_string()
    }

    /// Parse from base path (just the hash-name part, without store directory)
    pub fn from_base_path(s: &str) -> Result<Self, ParseStorePathError> {
        s.parse()
    }

    /// Serialize as base path (just the hash-name part, without store directory)
    pub fn serialize_as_base_path<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_base_path())
    }

    /// Deserialize from base path (just the hash-name part, without store directory)
    pub fn deserialize_from_base_path<'de, D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_base_path(&s).map_err(serde::de::Error::custom)
    }
}

impl fmt::Debug for StorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StorePath")
            .field(&format_args!("{self}"))
            .finish()
    }
}

impl fmt::Display for StorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.hash, self.name)
    }
}

impl Serialize for StorePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.serialize_as_base_path(serializer)
    }
}

impl<'de> Deserialize<'de> for StorePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::deserialize_from_base_path(deserializer)
    }
}

impl From<(StorePathHash, StorePathName)> for StorePath {
    fn from((hash, name): (StorePathHash, StorePathName)) -> Self {
        StorePath { hash, name }
    }
}

impl TryFrom<&[u8]> for StorePath {
    type Error = StorePathError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        StorePath::from_bytes(value)
    }
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
#[error("parse error {path}, {error}")]
pub struct ParseStorePathError {
    pub path: String,
    pub error: StorePathError,
}

impl ParseStorePathError {
    pub fn new<E>(path: &str, error: E) -> ParseStorePathError
    where
        E: Into<StorePathError>,
    {
        ParseStorePathError {
            path: path.to_owned(),
            error: error.into(),
        }
    }
}

impl FromStr for StorePath {
    type Err = ParseStorePathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        StorePath::from_bytes(s.as_bytes()).map_err(|error| ParseStorePathError::new(s, error))
    }
}

impl AsRef<StorePathName> for StorePath {
    fn as_ref(&self) -> &StorePathName {
        &self.name
    }
}

impl AsRef<StorePathHash> for StorePath {
    fn as_ref(&self) -> &StorePathHash {
        &self.hash
    }
}

impl FromStoreDirStr for StorePath {
    type Error = ParseStorePathError;

    fn from_store_dir_str(store_dir: &super::StoreDir, s: &str) -> Result<Self, Self::Error> {
        StorePath::new(s, store_dir)
    }
}

impl StoreDirDisplay for StorePath {
    fn fmt(&self, store_dir: &StoreDir, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{store_dir}/{self}")
    }
}

const STORE_PATH_HASH_SIZE: usize = 20;
const STORE_PATH_HASH_ENCODED_SIZE: usize = base32::encode_len(STORE_PATH_HASH_SIZE);

#[derive(
    Clone, Copy, PartialEq, Eq, zerocopy::FromBytes, zerocopy::IntoBytes, zerocopy::Immutable,
)]
#[repr(transparent)]
pub struct StorePathHash([u8; STORE_PATH_HASH_SIZE]);

impl StorePathHash {
    pub const fn len() -> usize {
        STORE_PATH_HASH_SIZE
    }

    pub const fn encoded_len() -> usize {
        STORE_PATH_HASH_ENCODED_SIZE
    }

    pub fn new(value: [u8; STORE_PATH_HASH_SIZE]) -> StorePathHash {
        StorePathHash(value)
    }

    pub fn new_from_hash(hash: &hash::Sha256) -> Self {
        let mut digest = [0u8; STORE_PATH_HASH_SIZE];
        for (i, item) in hash.as_ref().iter().enumerate() {
            let idx = i % STORE_PATH_HASH_SIZE;
            digest[idx] ^= item;
        }
        StorePathHash(digest)
    }

    pub fn copy_from_slice(data: &[u8]) -> StorePathHash {
        let mut digest = [0u8; STORE_PATH_HASH_SIZE];
        digest.copy_from_slice(data);
        StorePathHash::new(digest)
    }

    pub fn decode_digest(data: &[u8]) -> Result<StorePathHash, StorePathError> {
        if data.len() != base32::encode_len(STORE_PATH_HASH_SIZE) {
            return Err(StorePathError::HashLength);
        }
        let mut hash_output = [0u8; STORE_PATH_HASH_SIZE];
        base32::decode_mut(data, &mut hash_output).map_err(|err| {
            StorePathError::Symbol(err.error.position as u8, data[err.error.position])
        })?;
        Ok(StorePathHash::new(hash_output))
    }
}

impl fmt::Debug for StorePathHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StorePathHash({self})")
    }
}

impl fmt::Display for StorePathHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut output = [0u8; base32::encode_len(STORE_PATH_HASH_SIZE)];
        base32::encode_mut(&self.0, &mut output);

        // SAFETY: Nix Base32 is a subset of ASCII, which guarantees valid UTF-8.
        let s = unsafe { std::str::from_utf8_unchecked(&output) };
        f.write_str(s)
    }
}

impl TryFrom<&[u8]> for StorePathHash {
    type Error = StorePathError;

    fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
        if data.len() != Self::len() {
            return Err(StorePathError::HashLength);
        }
        Ok(Self::copy_from_slice(data))
    }
}

impl FromStr for StorePathHash {
    type Err = StorePathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        StorePathHash::decode_digest(s.as_bytes())
    }
}

impl std_hash::Hash for StorePathHash {
    fn hash<H: std_hash::Hasher>(&self, state: &mut H) {
        for c in self.0.iter().rev() {
            c.hash(state);
        }
    }
}

impl Ord for StorePathHash {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

impl PartialOrd for StorePathHash {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<[u8]> for StorePathHash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8; STORE_PATH_HASH_SIZE]> for StorePathHash {
    fn as_ref(&self) -> &[u8; STORE_PATH_HASH_SIZE] {
        &self.0
    }
}

impl AsMut<[u8]> for StorePathHash {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl AsMut<[u8; STORE_PATH_HASH_SIZE]> for StorePathHash {
    fn as_mut(&mut self) -> &mut [u8; STORE_PATH_HASH_SIZE] {
        &mut self.0
    }
}

impl Deref for StorePathHash {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

macro_rules! partial_eq_self {
    ($own:ty) => {
        impl PartialEq<&$own> for $own {
            fn eq(&self, other: &&$own) -> bool {
                self == **other
            }
        }
        impl PartialEq<$own> for &$own {
            fn eq(&self, other: &$own) -> bool {
                *self == other
            }
        }
    };
}
macro_rules! partial_eq {
    ($own:ty, $ty:ty) => {
        impl PartialEq<$ty> for $own {
            fn eq(&self, other: &$ty) -> bool {
                self.0 == *other
            }
        }
        impl PartialEq<$own> for $ty {
            fn eq(&self, other: &$own) -> bool {
                *self == other.0
            }
        }
    };
}
partial_eq_self!(StorePathHash);
partial_eq!(StorePathHash, &'_ [u8]);
partial_eq!(StorePathHash, [u8; STORE_PATH_HASH_SIZE]);

const NAME_LOOKUP: [bool; 256] = {
    let mut ret = [false; 256];
    let mut idx = 0usize;
    while idx < u8::MAX as usize {
        let ch = idx as u8;
        ret[idx] = matches!(ch, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'_' | b'?' | b'=' | b'.');
        idx += 1;
    }
    ret
};
pub(crate) const MAX_NAME_LEN: usize = 211;

pub(crate) fn into_name<V: AsRef<[u8]>>(s: &V) -> Result<&str, StorePathNameError> {
    let s = s.as_ref();
    if s.is_empty() || s.len() > MAX_NAME_LEN {
        return Err(StorePathNameError::NameLength);
    }

    for (idx, ch) in s.iter().enumerate() {
        if !NAME_LOOKUP[*ch as usize] {
            return Err(StorePathNameError::Symbol(idx as u8, *ch));
        }
    }

    // SAFETY: We checked above that it is a subset of ASCII, which guarantees valid UTF-8.
    let ret = unsafe { std::str::from_utf8_unchecked(s) };
    Ok(ret)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorePathName(String);

impl fmt::Display for StorePathName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&[u8]> for StorePathName {
    type Error = StorePathNameError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let name = into_name(&value)?;
        Ok(StorePathName(name.into()))
    }
}

impl FromStr for StorePathName {
    type Err = StorePathNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.as_bytes().try_into()
    }
}

impl AsRef<str> for StorePathName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for StorePathName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

partial_eq_self!(StorePathName);
partial_eq!(StorePathName, &'_ str);
partial_eq!(StorePathName, String);
partial_eq!(StorePathName, Cow<'_, str>);

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum StorePathError {
    #[error("non-absolute store path {0:?}")]
    NonAbsolute(PathBuf),
    #[error("path {0:?} is not in store")]
    NotInStore(PathBuf),
    #[error("invalid store path hash length")]
    HashLength,
    #[error("invalid store path name length")]
    NameLength,
    #[error("invalid store path {ch} symbol at {0}", ch = char::from_u32(*.1 as u32).map(|c| c.to_string()).unwrap_or_else(|| .1.to_string()))]
    Symbol(u8, u8),
}

impl StorePathError {
    fn adjust_index(prefix: u8, other: StorePathNameError) -> StorePathError {
        match other {
            StorePathNameError::Symbol(old, ch) => StorePathError::Symbol(prefix + old, ch),
            StorePathNameError::NameLength => StorePathError::NameLength,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum StorePathNameError {
    #[error("invalid name length")]
    NameLength,
    #[error("invalid name symbol '{ch}' at position {0}", ch = char::from_u32(*.1 as u32).map(|c| c.to_string()).unwrap_or_else(|| .1.to_string()))]
    Symbol(u8, u8),
}

impl From<StorePathNameError> for StorePathError {
    fn from(value: StorePathNameError) -> Self {
        match value {
            StorePathNameError::NameLength => StorePathError::NameLength,
            StorePathNameError::Symbol(idx, ch) => StorePathError::Symbol(idx, ch),
        }
    }
}

#[cfg(any(test, feature = "test"))]
pub mod proptest {
    use super::*;
    use ::proptest::{arbitrary::Arbitrary, prelude::*};

    pub fn arb_output_name() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9+\\-_?=][a-zA-Z0-9+\\-_?=.]{0,13}"
    }

    impl Arbitrary for StorePathHash {
        type Parameters = ();
        type Strategy = BoxedStrategy<StorePathHash>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            any::<[u8; STORE_PATH_HASH_SIZE]>()
                .prop_map(StorePathHash)
                .boxed()
        }
    }

    pub fn arb_store_path_name(
        max: u8,
        extension: Option<String>,
    ) -> impl Strategy<Value = StorePathName> {
        "[a-zA-Z0-9+\\-_?=][a-zA-Z0-9+\\-_?=.]{0,210}".prop_map(move |mut s| {
            let mut max = max;
            let len = extension.as_ref().map(|e| e.len() + 1).unwrap_or(0) as u8;
            if max > MAX_NAME_LEN as u8 - len {
                max = MAX_NAME_LEN as u8 - len;
            }
            max -= 1;
            if s.len() > max as usize {
                s.truncate(max as usize);
            }
            if let Some(ext) = extension.as_ref() {
                s.push('.');
                s.push_str(ext);
            }
            s.parse().unwrap()
        })
    }

    impl Arbitrary for StorePathName {
        type Parameters = Option<String>;
        type Strategy = BoxedStrategy<StorePathName>;

        fn arbitrary_with(args: Self::Parameters) -> Self::Strategy {
            arb_store_path_name(MAX_NAME_LEN as u8, args).boxed()
        }
    }

    pub fn arb_store_path(max: u8, extension: Option<String>) -> impl Strategy<Value = StorePath> {
        (any::<StorePathHash>(), arb_store_path_name(max, extension))
            .prop_map(|(hash, name)| StorePath { hash, name })
    }

    pub fn arb_drv_store_path() -> impl Strategy<Value = StorePath> {
        arb_store_path(MAX_NAME_LEN as u8 - 4 - 15, Some("drv".into()))
    }

    impl Arbitrary for StorePath {
        type Parameters = Option<String>;
        type Strategy = BoxedStrategy<StorePath>;
        fn arbitrary_with(args: Self::Parameters) -> Self::Strategy {
            arb_store_path(MAX_NAME_LEN as u8, args).boxed()
        }
    }
}

#[cfg(test)]
mod unittests {
    use std::cmp::Ordering;
    use std::collections::BTreeSet;

    use hex_literal::hex;
    use rstest::rstest;

    use super::*;

    // zzcfcjwxkn4cf1nh8dh521vffyq24179-perl5.38.0-libnet-3.12
    #[test]
    fn parse_hash() {
        let hash = "zzcfcjwxkn4cf1nh8dh521vffyq24179"
            .parse::<StorePathHash>()
            .unwrap();
        let expected = hex!("E904 22B0 776E 0751 6043 D006 C788 9D9D 4BE6 D8FF");
        assert_eq!(hash, expected);
        assert_eq!(*hash, expected);
        assert_eq!(hash.as_ref(), expected);
    }

    #[rstest]
    #[case::empty("", StorePathError::HashLength)]
    #[case::too_short("zzcfcjwxkn4cf1nh8dh521vffyq2417", StorePathError::HashLength)]
    #[case::too_long("zzcfcjwxkn4cf1nh8dh521vffyq24179a", StorePathError::HashLength)]
    #[case::invalid_symbol("zzcfcjwxkn4|f1nh8dh521vffyq24179", StorePathError::Symbol(11, b'|'))]
    #[test]
    fn parse_hash_error(#[case] hash: &str, #[case] expected: StorePathError) {
        let err = hash.parse::<StorePathHash>().expect_err("parse failure");
        assert_eq!(err, expected);
    }

    #[test]
    fn set_order() {
        let e = [
            "2q000000000000000000000000000000",
            "000h0000000000000000000000000000",
        ];
        let o = [
            "000h0000000000000000000000000000",
            "2q000000000000000000000000000000",
        ];
        let e_list: Vec<_> = e
            .into_iter()
            .map(|e| e.parse::<StorePathHash>().unwrap())
            .collect();
        let o_list: Vec<_> = o
            .into_iter()
            .map(|e| e.parse::<StorePathHash>().unwrap())
            .collect();
        let e_list1: BTreeSet<_> = e_list.iter().cloned().collect();
        let o_list1: BTreeSet<_> = o_list.iter().cloned().collect();

        let mut e_list2 = BTreeSet::new();
        for item in e_list.iter() {
            e_list2.insert(*item);
        }

        let mut o_list2 = BTreeSet::new();
        for item in o_list.iter() {
            o_list2.insert(*item);
        }
        assert_eq!(e_list1, e_list2);
        assert_eq!(o_list1, o_list2);
        assert_eq!(e_list1, o_list1);
        assert_eq!(e_list2, o_list2);
    }

    #[test]
    fn hash_order() {
        let list = [
            "00ljmhbmf3d12aq4l5l7yr7bxn03yqvf",
            "0sbwqgpi6jbqr710w5vn0b4s5w6z8n8n",
            "1hghwlv8pxghnkk1q0jvhlh2pzc1sc2f",
            "22dnr9nysk3gpy0jzw44fbi6gr5czzi3",
            "24sgyxikjg8i2sifywnczf6q697yds3z",
            "2nkhabskrzm94xr1fjdag3xbxy6qx75a",
            "2v8lw59nsmgqidcpw5szkxzd890ffr49",
            "2vr9ihd95b9xjvzzpxay1a1vzi3gx0xw",
            "2vr9ihd95b9xjvzzpxay1a1vzi3gx0xx",
            "3qxrdnqxbahxqsxb2rlifnmil7j1vxjh",
            "545hv9qa2jmkpd752nalbq4v1j1vm216",
            "5xfvjkml0qv8r5lq60s9br21wkhw2dmr",
            "7h623qgw0j4vmhx7cbis2dz6pps3j1bm",
            "868l02pyyr76vzcx6s3yfh9r69axarpn",
            "9hmpxy56lak38d06hwdsihnq2cxdcjk0",
            "a4z7pxg4xh6mm66s77d72ks1myzlk777",
            "agkqfd119da4f08d0s7l26ldj8nnxhv5",
            "b3pw0k3ww2avacsm89ik46bvcc511mxv",
            "hk60ghp7kcc1a0s2zmglizyhj6hmrbad",
            "lzdk0y2liz1jh9s34dcp7fijp96sxa7d",
            "mgn0lmx7jxqs64ixm3aamppmj23lfmpj",
            "nbyybld7l13gawd2rp2b6s7wwrpwlgck",
            "v8i77z0qbdm0k8hwhagrj7wjkjd3yiw9",
            "xn1jv0bmrybmmvlvcmjix77pkqq646ci",
            "ysmbabd63la2fydhv53qky5q8k1m7kp8",
            "zs498qq1arym4p4z6bkpid3xgrbl29rj",
        ];
        let parsed_list = list.map(|i| i.parse::<StorePathHash>().unwrap());
        for window in parsed_list.windows(2) {
            assert_eq!(Ordering::Less, window[0].cmp(&window[1]));
            assert_eq!(Some(Ordering::Less), window[0].partial_cmp(&window[1]));
        }
    }

    #[rstest]
    #[case("perl5.38.0-libnet-3.12")]
    #[case::all(".-_?+=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSSTUVWXYZ")]
    #[case::dot(".")]
    #[case::dotdot("..")]
    #[case::dotdash(".-")]
    #[case::dotdotdash("..-")]
    #[case::longest(
        "test-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    )]
    fn name_ok(#[case] case: &str) {
        let name = case.parse::<StorePathName>().expect("parses");
        assert_eq!(case, name.to_string());
        assert_eq!(case, name);
        assert_eq!(case.to_string(), name);
        assert_eq!(Cow::Borrowed(case), name);
        assert_eq!(case, name.as_ref());
        assert_eq!(case.as_bytes(), name.as_bytes());
        let name2: StorePathName = case.as_bytes().try_into().expect("parses bytes");
        assert_eq!(name, name2);
    }

    #[rstest]
    #[case::empty("", StorePathNameError::NameLength)]
    #[case::too_long(
        "test-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        StorePathNameError::NameLength
    )]
    #[case::invalid_char("test|more", StorePathNameError::Symbol(4, b'|'))]
    fn name_errors(#[case] name: &str, #[case] expected: StorePathNameError) {
        assert_eq!(
            name.parse::<StorePathName>().expect_err("parse succeeded"),
            expected
        );
    }

    #[rstest]
    #[case("perl5.38.0-libnet-3.12")]
    #[case::all(".-_?+=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSSTUVWXYZ")]
    #[case::dot(".")]
    #[case::dotdot("..")]
    #[case::dotdash(".-")]
    #[case::dotdotdash("..-")]
    #[case::longest(
        "test-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    )]
    fn store_path_ok(#[case] case_name: &str) {
        let case_hash = "00ljmhbmf3d12aq4l5l7yr7bxn03yqvf";
        let path_name = format!("{case_hash}-{case_name}");
        let path = path_name.parse::<StorePath>().expect("parses path");
        assert_eq!(path_name, path.to_string());
        let path2 = path_name.as_bytes().try_into().expect("parses path bytes");
        assert_eq!(path, path2);

        let name = case_name.parse::<StorePathName>().expect("parses name");
        assert_eq!(name, path.name());
        assert_eq!(name, AsRef::<StorePathName>::as_ref(&path));

        let hash = case_hash.parse::<StorePathHash>().expect("parses hash");
        assert_eq!(hash, path.hash());
        assert_eq!(hash, AsRef::<StorePathHash>::as_ref(&path));
    }

    #[rstest]
    #[case::empty("", ParseStorePathError::new("", StorePathError::HashLength))]
    #[case::too_short_hash(
        "00ljmhbmf3d12aq4l5l7yr7bxn03yqv-",
        ParseStorePathError::new("00ljmhbmf3d12aq4l5l7yr7bxn03yqv-", StorePathError::HashLength)
    )]
    #[case::invalid_hash_symbol(
        "00ljmhbmf3=12aq4l5l7yr7bxn03yqvv-test",
        ParseStorePathError::new(
            "00ljmhbmf3=12aq4l5l7yr7bxn03yqvv-test",
            StorePathError::Symbol(10, b'=')
        )
    )]
    #[case::wrong_dash(
        "00ljmhbmf3=12aq4l5l7yr7bxn03yqvv.test",
        ParseStorePathError::new(
            "00ljmhbmf3=12aq4l5l7yr7bxn03yqvv.test",
            StorePathError::Symbol(32, b'.')
        )
    )]
    #[case::missing_name(
        "00ljmhbmf3d12aq4l5l7yr7bxn03yqvv-",
        ParseStorePathError::new("00ljmhbmf3d12aq4l5l7yr7bxn03yqvv-", StorePathError::NameLength)
    )]
    #[case::name_too_long(
        "00ljmhbmf3d12aq4l5l7yr7bxn03yqvv-test-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ParseStorePathError::new(
            "00ljmhbmf3d12aq4l5l7yr7bxn03yqvv-test-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            StorePathError::NameLength
        )
    )]
    #[case::name_with_invalid_char(
        "00ljmhbmf3d12aq4l5l7yr7bxn03yqvv-test|more",
        ParseStorePathError::new(
            "00ljmhbmf3d12aq4l5l7yr7bxn03yqvv-test|more",
            StorePathError::Symbol(37, b'|')
        )
    )]
    fn store_path_error(#[case] path: &str, #[case] expected: ParseStorePathError) {
        assert_eq!(
            path.parse::<StorePath>().expect_err("parse succeeded"),
            expected
        );
    }

    #[test]
    fn store_path_order() {
        let list = [
            "3431a7m1xm7k8ggibfqjciji1h4hcpdg-polly-12.0.1.src.tar.xz.drv",
            "3n3vph932sfznfvp472jsr02wypg00c1-apple-framework-OpenGL.drv",
            "3rf1grj8n7akzy98rm4xlw0k0bsrhhb7-apple-framework-CoreFoundation.drv",
            "3sld67h643yp9l2496k567gn8zs63xmd-string_argv___string_argv_0.0.2.tgz.drv",
            "4ql4g3ss782y7c7a5i8bdj2v7b5izs4d-codemap-diagnostic-0.1.2.drv",
            "5qgdakl33rc83dcdln68ca62llp5zy9q-guava-parent-26.0-android.pom",
            "63k48d02cs6fqc5qb4m4qij2lp21rd74-cargo-build-hook.sh.drv",
            "6ky7iz3c7bbv35d7nkb69kjgl0mpkn6b-6531da946949a94643e6d8424236174ae64fe0ca.patch.drv",
            "6vb5s19cbmsfybizb67xv1y6pgricmk9-pytest-7.1.3.tar.gz.drv",
            "84ilav9kiyhfzw7a5lppngdhw48bbihs-hatchling-1.24.2.tar.gz.drv",
            "8hlynkqwgg3dkgyx6x6m549yqrx28py7-tools.logging-1.2.4.jar.drv",
            "9klylswa11kr8sqq65x1pfcl5y2lghls-tr46___tr46_0.0.3.tgz",
            "djg6gy8iymm9arxnmg0yyz3pms831wr0-libcxx-headers-src-16.0.6.drv",
            "inhj4681cf02mqvhkw3xrbbwmy6xbn9x-perl5.38.0-gettext-1.07.drv",
            "jdsl20r8mqjnr8vasqwsjvf2yg7ykdzw-libwebp-1.4.0.drv",
            "jnklxpwl49zdvqv04g2jfiq3719ic12z-xz-5.4.7-bin",
            "kkszlid4fss1s74bchh7hbpbndxzqq6i-https___registry.npmjs.org_assert___assert_1.5.0.tgz",
            "m2iin4fliaplacyrwq7l7bxyvk7cd9y0-https___registry.npmjs.org_is_svg___is_svg_3.0.0.tgz",
            "p0fgxad36glz02fpmfkbd4v19b58ja52-https___registry.npmjs.org_string_width___string_width_1.0.2.tgz.drv",
            "pfhr3caay320aklm05bf0z39aajk4sjx-transit-js-0.8.874.jar.drv",
            "q8abgca8z91caq4jkwh6sh3qyprrqmwl-rouge-4.1.3.gem.drv",
            "q8lq5j433pf27m3j6l7ki217dy9dpdgs-jackson-coreutils-1.8.pom.drv",
            "yishjp1jmaq1gw1n84v0k8hmj73d60p9-bash52-017.drv",
            "yq0lz1byj4v2rym2ng23a3nj4n6pvqdj-pgrp-pipe-5.patch",
            "ywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home",
        ];
        let parsed_list = list.map(|i| i.parse::<StorePath>().unwrap());
        for window in parsed_list.windows(2) {
            assert_eq!(Ordering::Less, window[0].cmp(&window[1]));
        }
    }

    #[rstest]
    #[case(
        "/nix/store/ywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home",
        "ywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home"
    )]
    #[case(
        "/nix/store/ywrs8hr8fa4244bpdxi88bd87qxqgmy0-.-_?+=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSSTUVWXYZ",
        "ywrs8hr8fa4244bpdxi88bd87qxqgmy0-.-_?+=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSSTUVWXYZ"
    )]
    #[test]
    fn from_store_dir_str(#[case] store_path: &str, #[case] base_path: StorePath) {
        let store = StoreDir::default();
        let path: StorePath = store.parse(store_path).expect("Can parse store path");
        assert_eq!(path, base_path);
    }

    #[rstest]
    #[case::empty(
        "",
        ParseStorePathError::new("", StorePathError::NonAbsolute(PathBuf::from("")))
    )]
    #[case::mising_file_name(
        "/nix/store/",
        ParseStorePathError::new("/nix/store/", StorePathError::HashLength)
    )]
    #[case::not_in_store(
        "/outsise/ywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home",
        ParseStorePathError::new(
            "/outsise/ywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home",
            StorePathError::NotInStore(PathBuf::from(
                "/outsise/ywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home"
            ))
        )
    )]
    #[case::missing_slash(
        "/nix/storeywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home",
        ParseStorePathError::new(
            "/nix/storeywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home",
            StorePathError::NotInStore(PathBuf::from(
                "/nix/storeywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home"
            ))
        )
    )]
    #[case::too_short(
        "/nix/store/zzcfcjwxkn4cf1nh8dh521vffyq2417",
        ParseStorePathError::new(
            "/nix/store/zzcfcjwxkn4cf1nh8dh521vffyq2417",
            StorePathError::HashLength
        )
    )]
    #[case::hash_too_long(
        "/nix/store/zzcfcjwxkn4cf1nh8dh521vffyq24179a-app",
        ParseStorePathError::new(
            "/nix/store/zzcfcjwxkn4cf1nh8dh521vffyq24179a-app",
            StorePathError::Symbol(32, b'a')
        )
    )]
    #[case::missing_name(
        "/nix/store/zzcfcjwxkn4cf1nh8dh521vffyq24179-",
        ParseStorePathError::new(
            "/nix/store/zzcfcjwxkn4cf1nh8dh521vffyq24179-",
            StorePathError::NameLength
        )
    )]
    #[case::bad_name(
        "/nix/store/zzcfcjwxkn4cf1nh8dh521vffyq24179-å",
        ParseStorePathError::new(
            "/nix/store/zzcfcjwxkn4cf1nh8dh521vffyq24179-å",
            StorePathError::Symbol(33, 195)
        )
    )]
    #[case::invalid_symbol(
        "/nix/store/zzcfcjwxkn4|f1nh8dh521vffyq24179-app",
        ParseStorePathError::new(
            "/nix/store/zzcfcjwxkn4|f1nh8dh521vffyq24179-app",
            StorePathError::Symbol(11, b'|')
        )
    )]
    #[test]
    fn from_store_dir_str_error(#[case] store_path: &str, #[case] expected: ParseStorePathError) {
        let store = StoreDir::default();
        let err = store
            .parse::<StorePath>(store_path)
            .expect_err("parse failure");
        assert_eq!(err, expected);
    }

    #[rstest]
    #[case(
        "/nix/store/ywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home",
        "ywrs8hr8fa4244bpdxi88bd87qxqgmy0-app-home"
    )]
    #[case(
        "/nix/store/ywrs8hr8fa4244bpdxi88bd87qxqgmy0-.-_?+=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSSTUVWXYZ",
        "ywrs8hr8fa4244bpdxi88bd87qxqgmy0-.-_?+=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSSTUVWXYZ"
    )]
    #[test]
    fn store_dir_display(#[case] store_path: &str, #[case] base_path: StorePath) {
        let store = StoreDir::default();
        let s = store.display(&base_path).to_string();
        assert_eq!(store_path, s);
    }
}

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    use crate::store_path::{StoreDir, StorePath, StorePathName};

    proptest! {
        #[test]
        fn proptest_store_name_parse_display(path in any::<StorePathName>()) {
            let s = path.to_string();
            let parsed = s.parse::<StorePathName>().expect("Parsing display");
            prop_assert_eq!(path, parsed);
        }
    }

    proptest! {
        #[test]
        fn proptest_store_path_parse_display(path in any::<StorePath>()) {
            let s = path.to_string();
            let parsed = s.parse::<StorePath>().expect("Parsing display");
            prop_assert_eq!(path, parsed);
        }
    }

    proptest! {
        #[test]
        fn proptest_store_dir_display_parse(store_dir in any::<StoreDir>(), path in any::<StorePath>()) {
            let s = store_dir.display(&path).to_string();
            let parsed = store_dir.parse::<StorePath>(&s).expect("Parsing display");
            prop_assert_eq!(path, parsed);
        }
    }
}
