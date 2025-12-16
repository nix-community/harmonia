use std::str::FromStr;

use derive_more::Display;

#[cfg(any(test, feature = "test"))]
use proptest_derive::Arbitrary;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use harmonia_utils_hash::fmt::{CommonHash, NonSRI, ParseHashError, ParseHashErrorKind};
use harmonia_utils_hash::{Algorithm, Hash, Sha256, UnknownAlgorithm};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display, Serialize, Deserialize,
)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
#[serde(rename_all = "lowercase")]
pub enum ContentAddressMethod {
    #[display("text")]
    Text,
    #[display("fixed")]
    Flat,
    #[display("fixed:r")]
    #[serde(rename = "nar")]
    Recursive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub enum ContentAddressMethodAlgorithm {
    #[display("text:sha256")]
    Text,
    #[display("{_0}")]
    Flat(Algorithm),
    #[display("r:{_0}")]
    Recursive(Algorithm),
}

/// Raw representation for JSON serialization/deserialization
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawContentAddressMethodAlgorithm {
    method: ContentAddressMethod,
    hash_algo: Algorithm,
}

impl Serialize for ContentAddressMethodAlgorithm {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let raw = RawContentAddressMethodAlgorithm {
            method: self.method(),
            hash_algo: self.algorithm(),
        };
        raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ContentAddressMethodAlgorithm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawContentAddressMethodAlgorithm::deserialize(deserializer)?;
        Ok(match raw.method {
            ContentAddressMethod::Text => ContentAddressMethodAlgorithm::Text,
            ContentAddressMethod::Flat => ContentAddressMethodAlgorithm::Flat(raw.hash_algo),
            ContentAddressMethod::Recursive => {
                ContentAddressMethodAlgorithm::Recursive(raw.hash_algo)
            }
        })
    }
}

impl ContentAddressMethodAlgorithm {
    pub fn algorithm(&self) -> Algorithm {
        match self {
            ContentAddressMethodAlgorithm::Text => Algorithm::SHA256,
            ContentAddressMethodAlgorithm::Flat(algorithm) => *algorithm,
            ContentAddressMethodAlgorithm::Recursive(algorithm) => *algorithm,
        }
    }

    pub fn method(&self) -> ContentAddressMethod {
        match self {
            ContentAddressMethodAlgorithm::Text => ContentAddressMethod::Text,
            ContentAddressMethodAlgorithm::Flat(_) => ContentAddressMethod::Flat,
            ContentAddressMethodAlgorithm::Recursive(_) => ContentAddressMethod::Recursive,
        }
    }
}

impl FromStr for ContentAddressMethodAlgorithm {
    type Err = ParseContentAddressError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "text:sha256" {
            Ok(Self::Text)
        } else if let Some(algo) = s.strip_prefix("r:") {
            Ok(Self::Recursive(algo.parse()?))
        } else {
            Ok(Self::Flat(s.parse()?))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub enum ContentAddress {
    #[display("text:{}", _0.as_base32())]
    Text(Sha256),
    #[display("fixed:{}", _0.as_base32())]
    Flat(Hash),
    #[display("fixed:r:{}", _0.as_base32())]
    Recursive(Hash),
}

impl ContentAddress {
    pub fn from_hash(
        method: ContentAddressMethod,
        hash: Hash,
    ) -> Result<ContentAddress, ParseHashErrorKind> {
        Ok(match method {
            ContentAddressMethod::Text => ContentAddress::Text(hash.try_into()?),
            ContentAddressMethod::Flat => ContentAddress::Flat(hash),
            ContentAddressMethod::Recursive => ContentAddress::Recursive(hash),
        })
    }
    pub fn algorithm(&self) -> Algorithm {
        self.method_algorithm().algorithm()
    }
    pub fn method(&self) -> ContentAddressMethod {
        match self {
            ContentAddress::Text(_) => ContentAddressMethod::Text,
            ContentAddress::Flat(_) => ContentAddressMethod::Flat,
            ContentAddress::Recursive(_) => ContentAddressMethod::Recursive,
        }
    }

    pub fn method_algorithm(&self) -> ContentAddressMethodAlgorithm {
        match self {
            ContentAddress::Text(_) => ContentAddressMethodAlgorithm::Text,
            ContentAddress::Flat(hash) => ContentAddressMethodAlgorithm::Flat(hash.algorithm()),
            ContentAddress::Recursive(hash) => {
                ContentAddressMethodAlgorithm::Recursive(hash.algorithm())
            }
        }
    }

    pub fn hash(&self) -> Hash {
        match *self {
            ContentAddress::Text(sha256) => sha256.into(),
            ContentAddress::Flat(hash) => hash,
            ContentAddress::Recursive(hash) => hash,
        }
    }
}

/// Raw content address representation for JSON serialization/deserialization
/// Uses SRI string format for hash to match upstream Nix JSON format
#[derive(Serialize, Deserialize)]
struct RawContentAddress {
    method: ContentAddressMethod,
    hash: Hash,
}

impl Serialize for ContentAddress {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let raw = RawContentAddress {
            method: self.method(),
            hash: self.hash(),
        };
        raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ContentAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de;

        let raw = RawContentAddress::deserialize(deserializer)?;

        ContentAddress::from_hash(raw.method, raw.hash).map_err(de::Error::custom)
    }
}

impl FromStr for ContentAddress {
    type Err = ParseContentAddressError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(hash_s) = s.strip_prefix("text:") {
            let sha256 = hash_s
                .parse::<NonSRI<Sha256>>()
                .map_err(|err| {
                    ParseContentAddressError::InvalidHash(ContentAddressMethod::Text, err)
                })?
                .into_hash();
            Ok(Self::Text(sha256))
        } else if let Some(hash_s) = s.strip_prefix("fixed:r:") {
            let hash = hash_s
                .parse::<NonSRI<Hash>>()
                .map_err(|err| {
                    ParseContentAddressError::InvalidHash(ContentAddressMethod::Recursive, err)
                })?
                .into_hash();
            Ok(Self::Recursive(hash))
        } else if let Some(hash_s) = s.strip_prefix("fixed:") {
            let hash = hash_s
                .parse::<NonSRI<Hash>>()
                .map_err(|err| {
                    ParseContentAddressError::InvalidHash(ContentAddressMethod::Flat, err)
                })?
                .into_hash();
            Ok(Self::Flat(hash))
        } else {
            Err(ParseContentAddressError::InvalidForm(s.into()))
        }
    }
}

#[derive(Error, Debug, PartialEq, Clone)]
pub enum ParseContentAddressError {
    #[error("content address {0} {1}")]
    InvalidHash(ContentAddressMethod, #[source] ParseHashError),
    #[error("{0} for content address")]
    UnknownAlgorithm(
        #[from]
        #[source]
        UnknownAlgorithm,
    ),
    #[error("'{0}' is not a content address because it is not in the form '<fixed | text>:<rest>'")]
    InvalidForm(String),
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use super::*;
    use harmonia_utils_hash::Algorithm;

    #[rstest]
    #[case::text(
        "text:sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s",
        ContentAddressMethod::Text,
        Algorithm::SHA256
    )]
    #[case::flat(
        "fixed:sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s",
        ContentAddressMethod::Flat,
        Algorithm::SHA256
    )]
    #[case::recursive(
        "fixed:r:sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s",
        ContentAddressMethod::Recursive,
        Algorithm::SHA256
    )]
    fn content_address_parse(
        #[case] v: &str,
        #[case] method: ContentAddressMethod,
        #[case] algo: Algorithm,
    ) {
        let s1 = "abc";
        let hash = algo.digest(s1);
        let content_address = ContentAddress::from_hash(method, hash).unwrap();

        assert_eq!(content_address.to_string(), v);
        assert_eq!(content_address, v.parse().unwrap());
    }

    #[rstest]
    #[should_panic = "content address text hash 'sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5' has wrong length for hash type 'sha256'"]
    #[case("text:sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5")]
    #[should_panic = "content address fixed hash 'sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5' has wrong length for hash type 'sha256'"]
    #[case("fixed:sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5")]
    #[should_panic = "content address fixed:r hash 'sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5' has wrong length for hash type 'sha256'"]
    #[case("fixed:r:sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5")]
    #[should_panic = "'test:sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5' is not a content address because it is not in the form '<fixed | text>:<rest>'"]
    #[case("test:sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5")]
    #[should_panic = "'test-12345' is not a content address because it is not in the form '<fixed | text>:<rest>'"]
    #[case("test-12345")]
    #[should_panic = "content address text hash 'sha1:kpcd173cq987hw957sx6m0868wv3x6d9' should have type 'sha256' but got 'sha1'"]
    #[case("text:sha1:kpcd173cq987hw957sx6m0868wv3x6d9")]
    fn test_content_address_error(#[case] value: &str) {
        let actual = value.parse::<ContentAddress>().unwrap_err();
        panic!("{actual}");
    }

    /*
    #[rstest]
    #[case(ContentAddressMethod::Text, "text:")]
    #[case(ContentAddressMethod::Flat, "")]
    #[case(ContentAddressMethod::Recursive, "r:")]
    fn content_address_method_parse(#[case] method: ContentAddressMethod, #[case] value: &str) {
        assert_eq!(method.to_string(), value);
        let actual = value.parse::<ContentAddressMethod>().unwrap();
        assert_eq!(actual, method);
    }
    */

    #[rstest]
    #[case(ContentAddressMethodAlgorithm::Text, "text:sha256")]
    #[case(ContentAddressMethodAlgorithm::Flat(Algorithm::MD5), "md5")]
    #[case(ContentAddressMethodAlgorithm::Flat(Algorithm::SHA1), "sha1")]
    #[case(ContentAddressMethodAlgorithm::Flat(Algorithm::SHA256), "sha256")]
    #[case(ContentAddressMethodAlgorithm::Flat(Algorithm::SHA512), "sha512")]
    #[case(ContentAddressMethodAlgorithm::Recursive(Algorithm::MD5), "r:md5")]
    #[case(ContentAddressMethodAlgorithm::Recursive(Algorithm::SHA1), "r:sha1")]
    #[case(
        ContentAddressMethodAlgorithm::Recursive(Algorithm::SHA256),
        "r:sha256"
    )]
    #[case(
        ContentAddressMethodAlgorithm::Recursive(Algorithm::SHA512),
        "r:sha512"
    )]
    fn content_address_method_algo_parse(
        #[case] method: ContentAddressMethodAlgorithm,
        #[case] value: &str,
    ) {
        assert_eq!(method.to_string(), value);
        let actual = value.parse::<ContentAddressMethodAlgorithm>().unwrap();
        assert_eq!(actual, method);
    }
}
