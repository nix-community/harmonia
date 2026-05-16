use harmonia_store_core::derivation::DerivationOutput;
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::{
    ContentAddress, ContentAddressMethodAlgorithm, StoreDir, StorePath, StorePathName,
};
use harmonia_utils_hash::Hash;
use harmonia_utils_hash::fmt::CommonHash as _;
use harmonia_utils_hash::fmt::NonSRI;

use crate::error::ParseError;

fn parse_utf8(bytes: &[u8]) -> Result<&str, ParseError> {
    std::str::from_utf8(bytes).map_err(|_| ParseError::InvalidUtf8 { pos: 0 })
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

/// Trait for types that can be converted to/from the raw ATerm output
/// representation. Both directions need `store_dir` to render/parse
/// absolute store paths.
pub trait AtermOutput: Sized {
    fn to_raw(
        &self,
        store_dir: &StoreDir,
        drv_name: &StorePathName,
        output_name: &OutputName,
    ) -> RawOutput;
    fn from_raw(
        raw: BorrowedRawOutput,
        store_dir: &StoreDir,
        drv_name: &StorePathName,
        output_name: &OutputName,
    ) -> Result<Self, ParseError>;
}

impl AtermOutput for DerivationOutput {
    fn to_raw(
        &self,
        store_dir: &StoreDir,
        drv_name: &StorePathName,
        output_name: &OutputName,
    ) -> RawOutput {
        match self {
            DerivationOutput::InputAddressed(path) => RawOutput {
                path: path
                    .to_absolute_path(store_dir)
                    .to_string_lossy()
                    .into_owned()
                    .into_bytes(),
                hash_algo: Vec::new(),
                hash: Vec::new(),
            },
            DerivationOutput::CAFixed(ca) => {
                let cama = ca.method_algorithm();
                let h = ca.hash();
                let path = self
                    .path(store_dir, drv_name, output_name)
                    .expect("CAFixed output path name should always be valid")
                    .expect("CAFixed output path should always be computable")
                    .to_absolute_path(store_dir)
                    .to_string_lossy()
                    .into_owned()
                    .into_bytes();
                RawOutput {
                    path,
                    hash_algo: cama.to_string().into_bytes(),
                    hash: h.as_base16().as_bare().to_string().into_bytes(),
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
        }
    }

    fn from_raw(
        r: BorrowedRawOutput,
        store_dir: &StoreDir,
        drv_name: &StorePathName,
        output_name: &OutputName,
    ) -> Result<Self, ParseError> {
        match r {
            // hashAlgo present, hash is "impure" → Impure
            // (path is ignored — nix leaves it empty for impure outputs)
            BorrowedRawOutput {
                path: [],
                hash_algo: algo @ [_, ..],
                hash: b"impure",
            } => {
                let cama: ContentAddressMethodAlgorithm = parse_utf8(algo)?.parse()?;
                Ok(Self::Impure(cama))
            }

            // hashAlgo and hash both present → CAFixed
            // Verify that the path (if present) matches what the CA computes.
            BorrowedRawOutput {
                path,
                hash_algo: algo @ [_, ..],
                hash: hash @ [_, ..],
            } => {
                let cama: ContentAddressMethodAlgorithm = parse_utf8(algo)?.parse()?;
                let hash: Hash = NonSRI::<Hash>::parse(cama.algorithm(), parse_utf8(hash)?)
                    .map_err(|e| ParseError::Hash(e.to_string()))?;
                let ca = ContentAddress::from_hash(cama.method(), hash)
                    .map_err(|e| ParseError::Hash(e.to_string()))?;
                let output = Self::CAFixed(ca);
                if let [_, ..] = path {
                    let actual: StorePath = store_dir
                        .parse(parse_utf8(path)?)
                        .map_err(|e| ParseError::StorePath { pos: 0, source: e })?;
                    if let Ok(Some(expected)) = output.path(store_dir, drv_name, output_name)
                        && actual != expected
                    {
                        return Err(ParseError::Hash(format!(
                            "CAFixed output path mismatch: expected {expected}, got {actual}",
                        )));
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
                let cama: ContentAddressMethodAlgorithm = parse_utf8(algo)?.parse()?;
                Ok(Self::CAFloating(cama))
            }

            // path present, hashAlgo and hash empty → InputAddressed
            BorrowedRawOutput {
                path: path @ [_, ..],
                hash_algo: [],
                hash: [],
            } => {
                let path: StorePath = store_dir
                    .parse(parse_utf8(path)?)
                    .map_err(|e| ParseError::StorePath { pos: 0, source: e })?;
                Ok(Self::InputAddressed(path))
            }

            // all empty → Deferred
            BorrowedRawOutput {
                path: [],
                hash_algo: [],
                hash: [],
            } => Ok(Self::Deferred),

            // Any other combination is invalid
            r => Err(ParseError::Hash(format!(
                "invalid output field combination: path={:?}, hashAlgo={:?}, hash={:?}",
                String::from_utf8_lossy(r.path),
                String::from_utf8_lossy(r.hash_algo),
                String::from_utf8_lossy(r.hash),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmonia_store_core::derivation::derivation_output_arbitrary::arb_output_name_for_drv;
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
            let raw = output.to_raw(&store_dir, &drv_name, &output_name);
            let roundtripped = DerivationOutput::from_raw(
                raw.borrow(), &store_dir, &drv_name, &output_name,
            ).unwrap();
            prop_assert_eq!(output, roundtripped);
        }
    }
}
