use thiserror::Error;

mod algo;
mod borrowed;
mod context;
pub mod fmt;
mod owned;
mod reader;
mod sha256;
mod view;
mod writer;

pub use algo::{Algorithm, UnknownAlgorithm};
pub use borrowed::BorrowedHash;
pub use context::Context;
pub use fmt::HashFormat;
pub use owned::Hash;
pub use reader::{HashReader, HashState};
pub use sha256::Sha256;
pub use view::HashView;
pub use writer::HashWriter;

#[derive(Error, Debug, PartialEq, Eq, Clone, Copy)]
#[error("hash has wrong length {length} != {} for hash type '{algorithm}'", algorithm.size())]
pub struct InvalidHashError {
    pub(crate) algorithm: Algorithm,
    pub(crate) length: usize,
}

#[cfg(feature = "test")]
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
                2 => Just(Algorithm::SHA512),
                2 => Just(Algorithm::BLAKE3)
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

// Serde and formatting tests remain here because they use `fmt` traits.
// Algorithm and digest tests moved to `algo.rs`.
#[cfg(test)]
mod unittests {
    use hex_literal::hex;
    use rstest::rstest;

    use super::*;

    const MD5_ABC: Hash = Hash::new(Algorithm::MD5, &hex!("900150983cd24fb0d6963f7d28e17f72"));
    const SHA1_ABC: Hash = Hash::new(
        Algorithm::SHA1,
        &hex!("a9993e364706816aba3e25717850c26c9cd0d89d"),
    );
    const SHA256_ABC: Hash = Hash::new(
        Algorithm::SHA256,
        &hex!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
    );
    const SHA512_ABC: Hash = Hash::new(
        Algorithm::SHA512,
        &hex!(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        ),
    );
    const BLAKE3_ABC: Hash = Hash::new(
        Algorithm::BLAKE3,
        &hex!("6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"),
    );

    #[rstest]
    #[case::sha256(&SHA256_ABC, "sha256-ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=")]
    #[case::sha1(&SHA1_ABC, "sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2J0=")]
    #[case::md5(&MD5_ABC, "md5-kAFQmDzST7DWlj99KOF/cg==")]
    #[case::sha512(&SHA512_ABC, "sha512-3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw==")]
    #[case::blake3(&BLAKE3_ABC, "blake3-ZDezrDhGUTP/tjt1JzqNtUjFWEZdedsD/TWcbNW9nYU=")]
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
