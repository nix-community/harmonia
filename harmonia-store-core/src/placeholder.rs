// SPDX-FileCopyrightText: 2025 PDT Partners, LLC.
// SPDX-License-Identifier: MIT
//
// This crate is derived from Nix-Ninja (https://github.com/pdtpartners/nix-ninja)
// Upstream commit: 8da02bd560f8bb406b82ae17ca99375f2b841b12

use std::path::PathBuf;

use crate::base32;
use crate::derived_path::SingleDerivedPath;
use crate::hash::Sha256;
use crate::store_path::StorePath;

/// A placeholder for a Nix store path
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    /// The hash of the placeholder
    hash: Vec<u8>,
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
    pub fn standard_output(output_name: &str) -> Self {
        let clear_text = format!("nix-output:{output_name}");
        let hash = sha256_hash(clear_text.as_bytes());
        Self::new(hash)
    }

    /// Generate a placeholder for a content-addressed derivation output
    pub fn ca_output(drv_path: &StorePath, output_name: &str) -> Self {
        let drv_name = drv_path.name().as_ref();
        let drv_name = if drv_name.ends_with(".drv") {
            &drv_name[0..drv_name.len() - 4]
        } else {
            drv_name
        };

        // Format the output path name according to Nix conventions
        let output_path_name = output_path_name(drv_name, output_name);

        let clear_text = format!(
            "nix-upstream-output:{}:{}",
            drv_path.hash(),
            output_path_name
        );

        let hash = sha256_hash(clear_text.as_bytes());
        Self::new(hash)
    }

    /// Generate a placeholder for a dynamic derivation output
    pub fn dynamic_output(placeholder: &Placeholder, output_name: &str) -> Self {
        // Compress the hash according to Nix's implementation
        let compressed = compress_hash(&placeholder.hash, 20);

        let compressed_str = base32::encode_string(&compressed);
        let clear_text = format!("nix-computed-output:{compressed_str}:{output_name}");

        let hash = sha256_hash(clear_text.as_bytes());
        Self::new(hash)
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

/// Format an output path name according to Nix conventions
pub fn output_path_name(drv_name: &str, output_name: &str) -> String {
    if output_name == "out" {
        drv_name.to_string()
    } else {
        format!("{drv_name}-{output_name}")
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

/// Compute placeholder for a SingleDerivedPath::Built variant
pub fn compute_built_placeholder(drv_path: &SingleDerivedPath, output: &str) -> PathBuf {
    compute_built_placeholder_recursive(drv_path, output).render()
}

fn compute_built_placeholder_recursive(drv_path: &SingleDerivedPath, output: &str) -> Placeholder {
    match drv_path {
        SingleDerivedPath::Opaque(store_path) => {
            // Base case: regular ca_output placeholder
            Placeholder::ca_output(store_path, output)
        }
        SingleDerivedPath::Built {
            drv_path: inner_drv_path,
            output: inner_output,
        } => {
            // Recursive case: create dynamic_output placeholder
            let inner_placeholder =
                compute_built_placeholder_recursive(inner_drv_path, inner_output.as_ref());
            Placeholder::dynamic_output(&inner_placeholder, output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_placeholder() {
        let placeholder = Placeholder::standard_output("out");
        assert_eq!(
            placeholder.render(),
            PathBuf::from("/1rz4g4znpzjwh1xymhjpm42vipw92pr73vdgl6xs1hycac8kf2n9")
        );
    }

    #[test]
    fn test_ca_placeholder() {
        let store_path: StorePath = "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap();
        let placeholder = Placeholder::ca_output(&store_path, "out");
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
        let placeholder = Placeholder::ca_output(&store_path, "out");
        let dynamic = Placeholder::dynamic_output(&placeholder, "out");
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
    fn test_output_path_name() {
        // Test with "out" output
        assert_eq!(output_path_name("hello-2.10", "out"), "hello-2.10");

        // Test with non-"out" output
        assert_eq!(output_path_name("hello-2.10", "bin"), "hello-2.10-bin");
        assert_eq!(output_path_name("hello-2.10", "dev"), "hello-2.10-dev");
    }

    #[test]
    fn test_compute_built_placeholder_opaque() {
        let store_path: StorePath = "00000000000000000000000000000000-test".parse().unwrap();
        let placeholder = compute_built_placeholder(&SingleDerivedPath::Opaque(store_path), "out");
        assert!(!placeholder.to_string_lossy().is_empty());
    }

    #[test]
    fn test_compute_built_placeholder_nested() {
        use std::sync::Arc;

        let store_path: StorePath = "00000000000000000000000000000000-test".parse().unwrap();
        let inner_path = SingleDerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(store_path)),
            output: "inner".parse().unwrap(),
        };
        let placeholder = compute_built_placeholder(&inner_path, "outer");
        assert!(!placeholder.to_string_lossy().is_empty());
    }
}
