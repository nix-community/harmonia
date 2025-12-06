// SPDX-FileCopyrightText: 2025 PDT Partners, LLC.
// SPDX-License-Identifier: MIT
//
// This crate is derived from Nix-Ninja (https://github.com/pdtpartners/nix-ninja)
// Upstream commit: 8da02bd560f8bb406b82ae17ca99375f2b841b12

use std::fmt;
use std::path::PathBuf;

use crate::derivation::OutputPathName;
use crate::derived_path::{OutputName, SingleDerivedPath};
use crate::drv_ref::DrvRef;
use crate::store_path::{StoreDir, StoreDirDisplay, StorePath};
use harmonia_utils_base_encoding::base32;
use harmonia_utils_hash::Sha256;

/// A placeholder for a Nix store path
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    /// The hash of the placeholder
    hash: Vec<u8>,
}

/// A `DrvRef` can be turned into either a store path or placeholder for
/// purposes of embedding in a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorePathOrPlaceholder {
    /// An external store path (already known)
    StorePath(StorePath),
    /// A placeholder path (for self-references or unbuilt CA outputs)
    Placeholder(Placeholder),
}

impl StoreDirDisplay for StorePathOrPlaceholder {
    fn fmt(&self, store_dir: &StoreDir, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorePathOrPlaceholder::StorePath(store_path) => store_path.fmt(store_dir, f),
            StorePathOrPlaceholder::Placeholder(placeholder) => {
                write!(f, "{}", placeholder.render().display())
            }
        }
    }
}

impl Placeholder {
    /// Create a new placeholder from a hash
    fn new(hash: Vec<u8>) -> Self {
        Self { hash }
    }

    /// Render the placeholder as a string
    pub fn render(&self) -> PathBuf {
        PathBuf::from(format!("/{}", base32::encode_string(&self.hash)))
    }

    /// Generate a placeholder for a standard output
    pub fn standard_output(output_name: &OutputName) -> Self {
        let clear_text = format!("nix-output:{output_name}");
        let hash = sha256_hash(clear_text.as_bytes());
        Self::new(hash)
    }

    /// Generate a placeholder for a content-addressed derivation output
    pub fn ca_output(drv_path: &StorePath, output_name: &OutputName) -> Self {
        let drv_name_str = drv_path.name().as_ref();
        let drv_name_str = drv_name_str.strip_suffix(".drv").unwrap_or(drv_name_str);
        // Safe to unwrap: stripping ".drv" from a valid store path name yields a valid name
        let drv_name = drv_name_str.parse().unwrap();

        let clear_text = format!(
            "nix-upstream-output:{}:{}",
            drv_path.hash(),
            OutputPathName {
                drv_name: &drv_name,
                output_name
            }
        );

        let hash = sha256_hash(clear_text.as_bytes());
        Self::new(hash)
    }

    /// Generate a placeholder for a dynamic derivation output
    pub fn dynamic_output(placeholder: &Placeholder, output_name: &OutputName) -> Self {
        // Compress the hash according to Nix's implementation
        let compressed = compress_hash(&placeholder.hash, 20);

        let compressed_str = base32::encode_string(&compressed);
        let clear_text = format!("nix-computed-output:{compressed_str}:{output_name}");

        let hash = sha256_hash(clear_text.as_bytes());
        Self::new(hash)
    }

    /// Generate a placeholder for an output of a derivation that may itself be a placeholder.
    ///
    /// This dispatches to `ca_output` for store paths or `dynamic_output` for placeholders.
    pub fn output(drv: &StorePathOrPlaceholder, output_name: &OutputName) -> Self {
        match drv {
            StorePathOrPlaceholder::StorePath(store_path) => {
                Self::ca_output(store_path, output_name)
            }
            StorePathOrPlaceholder::Placeholder(placeholder) => {
                Self::dynamic_output(placeholder, output_name)
            }
        }
    }
}

impl TryFrom<String> for Placeholder {
    type Error = std::io::Error;

    fn try_from(str: String) -> Result<Self, Self::Error> {
        let mut hash = vec![0u8; base32::decode_len(str.len())];
        base32::decode_mut(str.as_bytes(), &mut hash).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Not valid nix base32 string: {str} (error: {:?})", err),
            )
        })?;

        Ok(Placeholder::new(hash))
    }
}

impl From<&DrvRef<StorePath>> for StorePathOrPlaceholder {
    fn from(drv_ref: &DrvRef<StorePath>) -> Self {
        match drv_ref {
            DrvRef::External(path) => StorePathOrPlaceholder::StorePath(path.clone()),
            DrvRef::SelfOutput(output) => {
                StorePathOrPlaceholder::Placeholder(Placeholder::standard_output(output))
            }
        }
    }
}

impl From<&DrvRef<SingleDerivedPath>> for StorePathOrPlaceholder {
    fn from(drv_ref: &DrvRef<SingleDerivedPath>) -> Self {
        match drv_ref {
            DrvRef::External(path) => path.into(),
            DrvRef::SelfOutput(output) => {
                StorePathOrPlaceholder::Placeholder(Placeholder::standard_output(output))
            }
        }
    }
}

/// Compress a hash to a smaller size by XORing bytes
fn compress_hash(hash: &[u8], new_size: usize) -> Vec<u8> {
    if hash.is_empty() {
        return vec![];
    }

    let mut result = vec![0u8; new_size];

    for (i, &byte) in hash.iter().enumerate() {
        result[i % new_size] ^= byte;
    }

    result
}

/// Calculate SHA-256 hash of data
fn sha256_hash(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).digest_bytes().to_vec()
}

impl From<&SingleDerivedPath> for StorePathOrPlaceholder {
    fn from(path: &SingleDerivedPath) -> Self {
        match path {
            SingleDerivedPath::Opaque(store_path) => {
                StorePathOrPlaceholder::StorePath(store_path.clone())
            }
            SingleDerivedPath::Built { drv_path, output } => {
                let drv: StorePathOrPlaceholder = drv_path.as_ref().into();
                StorePathOrPlaceholder::Placeholder(Placeholder::output(&drv, output))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_placeholder() {
        let output: OutputName = "out".parse().unwrap();
        let placeholder = Placeholder::standard_output(&output);
        assert_eq!(
            placeholder.render(),
            PathBuf::from("/1rz4g4znpzjwh1xymhjpm42vipw92pr73vdgl6xs1hycac8kf2n9")
        );
    }

    #[test]
    fn test_ca_placeholder() {
        let store_path: StorePath = "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap();
        let output: OutputName = "out".parse().unwrap();
        let placeholder = Placeholder::ca_output(&store_path, &output);
        assert_eq!(
            placeholder.render(),
            PathBuf::from("/0c6rn30q4frawknapgwq386zq358m8r6msvywcvc89n6m5p2dgbz")
        );
    }

    #[test]
    fn test_dynamic_placeholder() {
        let store_path: StorePath = "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv.drv"
            .parse()
            .unwrap();
        let output: OutputName = "out".parse().unwrap();
        let placeholder = Placeholder::ca_output(&store_path, &output);
        let dynamic = Placeholder::dynamic_output(&placeholder, &output);
        assert_eq!(
            dynamic.render(),
            PathBuf::from("/0gn6agqxjyyalf0dpihgyf49xq5hqxgw100f0wydnj6yqrhqsb3w"),
        )
    }

    #[test]
    fn test_store_path_parsing() {
        let path: StorePath = "ac8da0sqpg4pyhzyr0qgl26d5dnpn7qp-hello-2.10.tar.gz"
            .parse()
            .unwrap();
        assert_eq!(path.hash().to_string(), "ac8da0sqpg4pyhzyr0qgl26d5dnpn7qp");
        assert_eq!(path.name().as_ref(), "hello-2.10.tar.gz");

        // Test with a derivation path
        let drv_path: StorePath = "q3lv9bi7r4di3kxdjhy7kvwgvpmanfza-hello-2.10.drv"
            .parse()
            .unwrap();
        assert_eq!(
            drv_path.hash().to_string(),
            "q3lv9bi7r4di3kxdjhy7kvwgvpmanfza"
        );
        assert_eq!(drv_path.name().as_ref(), "hello-2.10.drv");
        assert!(drv_path.is_derivation());
    }

    #[test]
    fn test_single_derived_path_opaque_to_store_path_or_placeholder() {
        let store_path: StorePath = "00000000000000000000000000000000-test".parse().unwrap();
        let path = SingleDerivedPath::Opaque(store_path.clone());
        let result: StorePathOrPlaceholder = (&path).into();
        assert_eq!(result, StorePathOrPlaceholder::StorePath(store_path));
    }

    #[test]
    fn test_compute_built_placeholder_nested() {
        use std::sync::Arc;

        let store_path: StorePath = "00000000000000000000000000000000-test".parse().unwrap();
        let inner_path = SingleDerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(store_path)),
            output: "inner".parse().unwrap(),
        };
        let drv: StorePathOrPlaceholder = (&inner_path).into();
        let output: OutputName = "outer".parse().unwrap();
        let placeholder = Placeholder::output(&drv, &output);
        assert!(!placeholder.render().to_string_lossy().is_empty());
    }
}
