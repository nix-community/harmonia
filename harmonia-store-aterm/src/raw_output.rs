use std::str::Utf8Error;

use harmonia_store_content_address::{
    ContentAddress, ContentAddressMethodAlgorithm, ParseContentAddressError,
};
use harmonia_store_derivation::derivation::DerivationOutput;
use harmonia_store_derivation::derived_path::OutputName;
use harmonia_store_path::{
    ParseStorePathError, StoreDir, StorePath, StorePathName, StorePathNameError,
};
use harmonia_utils_base_encoding::Base;
use harmonia_utils_hash::fmt::ParseHashErrorKind;
use harmonia_utils_hash::{Hash, InvalidHashError};

/// Error from decoding raw output fields into a typed output.
#[derive(Debug, thiserror::Error)]
pub enum FromRawOutputError {
    // Byte-decoding errors
    #[error("invalid UTF-8: {0}")]
    InvalidUtf8(#[from] Utf8Error),
    #[error("invalid store path: {0}")]
    StorePath(#[from] ParseStorePathError),
    #[error("invalid content address method/algorithm: {0}")]
    ParseMethodAlgo(#[from] ParseContentAddressError),
    #[error("invalid {base} hash at position {position}")]
    HashDecode { base: Base, position: usize },

    // Variant-reconstruction errors
    #[error("invalid hash: {0}")]
    InvalidHash(#[from] InvalidHashError),
    #[error("invalid content address: {0}")]
    InvalidContentAddress(#[from] ParseHashErrorKind),
    #[error("failed to compute CAFixed output path: {0}")]
    PathComputation(#[from] StorePathNameError),
    #[error("CAFixed output did not yield a path")]
    MissingFixedPath,
    #[error("CAFixed output path mismatch: expected {expected}, got {actual}")]
    PathMismatch {
        expected: StorePath,
        actual: StorePath,
    },
    #[error("invalid output field combination: {0:?}")]
    InvalidCombination(#[from] InvalidCombination),
}

/// Raw ATerm-level representation of a derivation output.
///
/// Each output in the ATerm is a tuple `(name, path, hashAlgo, hash)`.
/// This struct holds the `path`, `hashAlgo`, and `hash` fields —
/// the shared representation between parser and printer.
#[derive(Debug, Clone)]
pub struct RawOutput {
    pub path: Vec<u8>,
    pub hash_algo: Vec<u8>,
    pub hash: Vec<u8>,
}

impl RawOutput {
    pub fn borrow(&self) -> BorrowedRawOutput<'_> {
        BorrowedRawOutput {
            path: &self.path,
            hash_algo: &self.hash_algo,
            hash: &self.hash,
        }
    }
}

/// Borrowed slice counterpart of `RawOutput`.
#[derive(Debug, Clone)]
pub struct BorrowedRawOutput<'a> {
    pub path: &'a [u8],
    pub hash_algo: &'a [u8],
    pub hash: &'a [u8],
}

impl BorrowedRawOutput<'_> {
    pub fn to_owned(&self) -> RawOutput {
        RawOutput {
            path: self.path.to_vec(),
            hash_algo: self.hash_algo.to_vec(),
            hash: self.hash.to_vec(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid output field combination: {0:?}")]
pub struct InvalidCombination(pub RawOutput);

/// Trait for types that can be converted to/from the raw ATerm output
/// representation. Both directions need `store_dir` to render/parse
/// absolute store paths.
pub trait AtermOutput: Sized {
    type Error;

    fn to_raw(
        &self,
        store_dir: &StoreDir,
        drv_name: &StorePathName,
        output_name: &OutputName,
        base: Base,
    ) -> Result<RawOutput, StorePathNameError>;

    fn from_raw(
        raw: BorrowedRawOutput,
        store_dir: &StoreDir,
        drv_name: &StorePathName,
        output_name: &OutputName,
        base: Base,
    ) -> Result<Self, Self::Error>;
}

impl AtermOutput for DerivationOutput {
    type Error = FromRawOutputError;

    fn to_raw(
        &self,
        store_dir: &StoreDir,
        drv_name: &StorePathName,
        output_name: &OutputName,
        base: Base,
    ) -> Result<RawOutput, StorePathNameError> {
        let path_bytes = |p: &StorePath| -> Vec<u8> {
            p.to_absolute_path(store_dir)
                .to_string_lossy()
                .into_owned()
                .into_bytes()
        };

        Ok(match self {
            DerivationOutput::InputAddressed(path) => RawOutput {
                path: path_bytes(path),
                hash_algo: Vec::new(),
                hash: Vec::new(),
            },
            DerivationOutput::CAFixed(ca) => {
                let path = self
                    .path(store_dir, drv_name, output_name)?
                    .expect("CAFixed always has a path");
                RawOutput {
                    path: path_bytes(&path),
                    hash_algo: ca.method_algorithm().to_string().into_bytes(),
                    hash: {
                        let mut hash = vec![0u8; base.input_len(ca.hash().digest_bytes().len())];
                        harmonia_utils_base_encoding::encode_for_base(base)(
                            ca.hash().digest_bytes(),
                            &mut hash,
                        );
                        hash
                    },
                }
            }
            DerivationOutput::CAFloating(cama) => RawOutput {
                path: Vec::new(),
                hash_algo: cama.to_string().into_bytes(),
                hash: Vec::new(),
            },
            DerivationOutput::Impure(cama) => RawOutput {
                path: Vec::new(),
                hash_algo: cama.to_string().into_bytes(),
                hash: b"impure".to_vec(),
            },
            DerivationOutput::Deferred => RawOutput {
                path: Vec::new(),
                hash_algo: Vec::new(),
                hash: Vec::new(),
            },
        })
    }

    fn from_raw(
        raw: BorrowedRawOutput,
        store_dir: &StoreDir,
        drv_name: &StorePathName,
        output_name: &OutputName,
        base: Base,
    ) -> Result<Self, FromRawOutputError> {
        let decode_hash = |hash_bytes: &[u8]| -> Result<Vec<u8>, FromRawOutputError> {
            let mut digest = vec![0u8; base.decode_len(hash_bytes.len())];
            harmonia_utils_base_encoding::decode_for_base(base)(hash_bytes, &mut digest).map_err(
                |e| FromRawOutputError::HashDecode {
                    base,
                    position: e.error.position,
                },
            )?;
            Ok(digest)
        };

        match raw {
            // hashAlgo present, hash is "impure" → Impure
            // (path is ignored — nix leaves it empty for impure outputs)
            BorrowedRawOutput {
                path: [],
                hash_algo: algo @ [_, ..],
                hash: b"impure",
            } => {
                let cama: ContentAddressMethodAlgorithm = std::str::from_utf8(algo)?.parse()?;
                Ok(Self::Impure(cama))
            }

            // hashAlgo and hash both present → CAFixed
            // Verify that the path (if present) matches what the CA computes.
            BorrowedRawOutput {
                path,
                hash_algo: algo @ [_, ..],
                hash: hash @ [_, ..],
            } => {
                let cama: ContentAddressMethodAlgorithm = std::str::from_utf8(algo)?.parse()?;
                let digest = decode_hash(hash)?;
                let hash = Hash::from_slice(cama.algorithm(), &digest)?;
                let ca = ContentAddress::from_hash(cama.method(), hash)?;
                let output = Self::CAFixed(ca);
                // If a path was supplied, verify it matches what the CA computes.
                if let [_, ..] = path {
                    let actual: StorePath = store_dir.parse(std::str::from_utf8(path)?)?;
                    let expected = output
                        .path(store_dir, drv_name, output_name)?
                        .ok_or(FromRawOutputError::MissingFixedPath)?;
                    if actual != expected {
                        return Err(FromRawOutputError::PathMismatch { expected, actual });
                    }
                }
                Ok(output)
            }

            // hashAlgo present, hash empty → CAFloating
            BorrowedRawOutput {
                path: [],
                hash_algo: algo @ [_, ..],
                hash: [],
            } => {
                let cama: ContentAddressMethodAlgorithm = std::str::from_utf8(algo)?.parse()?;
                Ok(Self::CAFloating(cama))
            }

            // path present, hashAlgo and hash empty → InputAddressed
            BorrowedRawOutput {
                path: path @ [_, ..],
                hash_algo: [],
                hash: [],
            } => {
                let path: StorePath = store_dir.parse(std::str::from_utf8(path)?)?;
                Ok(Self::InputAddressed(path))
            }

            // all empty → Deferred
            BorrowedRawOutput {
                path: [],
                hash_algo: [],
                hash: [],
            } => Ok(Self::Deferred),

            // Any other combination is invalid
            r => Err(InvalidCombination(r.to_owned()).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmonia_store_derivation::derivation::derivation_output_arbitrary::arb_output_name_for_drv;
    use proptest::prelude::*;

    fn arb_output_with_names()
    -> impl Strategy<Value = (DerivationOutput, StorePathName, OutputName)> {
        (any::<DerivationOutput>(), any::<StorePathName>()).prop_flat_map(|(output, drv_name)| {
            let output_name_strategy = arb_output_name_for_drv(&drv_name);
            output_name_strategy
                .prop_map(move |output_name| (output.clone(), drv_name.clone(), output_name))
        })
    }

    proptest! {
        #[test]
        fn derivation_output_roundtrips(
            (output, drv_name, output_name) in arb_output_with_names(),
        ) {
            let store_dir = StoreDir::default();
            let raw = output.to_raw(&store_dir, &drv_name, &output_name, Base::Hex).unwrap();
            let roundtripped = DerivationOutput::from_raw(
                raw.borrow(), &store_dir, &drv_name, &output_name, Base::Hex,
            ).unwrap();
            prop_assert_eq!(output, roundtripped);
        }
    }
}
