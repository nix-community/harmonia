// SPDX-FileCopyrightText: 2025 PDT Partners, LLC.
// SPDX-License-Identifier: MIT
//
// This crate is derived from Nix-Ninja (https://github.com/pdtpartners/nix-ninja)
// Upstream commit: 8da02bd560f8bb406b82ae17ca99375f2b841b12

use std::fmt;
use std::path::PathBuf;

use crate::derived_path::{OutputName, SingleDerivedPath};
use harmonia_store_path::{StoreDir, StoreDirDisplay, StorePath, StorePathHash};
use harmonia_utils_base_encoding::base32;
use harmonia_utils_hash::{Algorithm, Context, Sha256};

const SHA256_LEN: usize = 32;
const SHA256_BASE32_LEN: usize = base32::encode_len(SHA256_LEN);
const STACK_HASH_BUF_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlaceholderHash {
    Sha256([u8; SHA256_LEN]),
    Other(Box<[u8]>),
}

impl PlaceholderHash {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Sha256(arr) => arr,
            Self::Other(slice) => slice,
        }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Self::Sha256(arr) => arr,
            Self::Other(slice) => slice,
        }
    }
}

/// A placeholder for a Nix store path
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    /// The hash of the placeholder
    hash: PlaceholderHash,
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
            StorePathOrPlaceholder::Placeholder(placeholder) => placeholder.fmt_rendered(f),
        }
    }
}

fn sha256_parts(parts: &[&[u8]]) -> Placeholder {
    let total: usize = parts
        .iter()
        .fold(0usize, |acc, p| acc.saturating_add(p.len()));
    let digest = if total <= STACK_HASH_BUF_LEN {
        let mut buf = [0u8; STACK_HASH_BUF_LEN];
        let mut pos = 0;
        for part in parts {
            buf[pos..pos + part.len()].copy_from_slice(part);
            pos += part.len();
        }
        Sha256::digest(&buf[..pos])
    } else {
        sha256_parts_slow(parts)
    };
    Placeholder {
        hash: PlaceholderHash::Sha256(*digest.digest_bytes()),
    }
}

#[cold]
#[inline(never)]
fn sha256_parts_slow(parts: &[&[u8]]) -> Sha256 {
    let mut ctx = Context::new(Algorithm::SHA256);
    for part in parts {
        ctx.update(part);
    }
    ctx.finish()
        .try_into()
        .expect("SHA-256 context yields SHA-256")
}

impl Placeholder {
    fn fmt_rendered(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.hash {
            PlaceholderHash::Sha256(arr) => {
                let mut buf = [0u8; 1 + SHA256_BASE32_LEN];
                buf[0] = b'/';
                base32::encode_mut(arr, &mut buf[1..]);
                let s = std::str::from_utf8(&buf).expect("nix-base32 is ASCII");
                f.write_str(s)
            }
            PlaceholderHash::Other(slice) => {
                write!(f, "/{}", base32::encode_string(slice))
            }
        }
    }

    fn encode_rendered(&self) -> Vec<u8> {
        let slice = self.hash.as_slice();
        let mut out = vec![0u8; 1 + base32::encode_len(slice.len())];
        out[0] = b'/';
        base32::encode_mut(slice, &mut out[1..]);
        out
    }

    pub(crate) fn render_bytes(&self) -> bytes::Bytes {
        bytes::Bytes::from(self.encode_rendered())
    }

    /// Render the placeholder as a string
    pub fn render(&self) -> PathBuf {
        let s = String::from_utf8(self.encode_rendered()).expect("nix-base32 is ASCII");
        PathBuf::from(s)
    }

    /// Generate a placeholder for a standard output
    pub fn standard_output(output_name: &OutputName) -> Self {
        sha256_parts(&[b"nix-output:", output_name.as_ref().as_bytes()])
    }

    /// Generate a placeholder for a content-addressed derivation output
    pub fn ca_output(drv_path: &StorePath, output_name: &OutputName) -> Self {
        let drv_name_str: &str = drv_path.name().as_ref();
        let drv_name_str = drv_name_str.strip_suffix(".drv").unwrap_or(drv_name_str);

        let mut hash_b32 = [0u8; StorePathHash::encoded_len()];
        base32::encode_mut(drv_path.hash().as_ref(), &mut hash_b32);

        let (sep, out): (&[u8], &[u8]) = if output_name.is_default() {
            (b"", b"")
        } else {
            (b"-", output_name.as_ref().as_bytes())
        };

        sha256_parts(&[
            b"nix-upstream-output:",
            &hash_b32,
            b":",
            drv_name_str.as_bytes(),
            sep,
            out,
        ])
    }

    /// Generate a placeholder for a dynamic derivation output
    pub fn dynamic_output(placeholder: &Placeholder, output_name: &OutputName) -> Self {
        const COMPRESSED_LEN: usize = StorePathHash::len();
        let mut compressed_b32 = [0u8; StorePathHash::encoded_len()];
        let b32_slice: &[u8] = match &placeholder.hash {
            PlaceholderHash::Sha256(arr) => {
                let mut compressed: [u8; COMPRESSED_LEN] = arr[..COMPRESSED_LEN]
                    .try_into()
                    .expect("SHA256_LEN >= COMPRESSED_LEN");
                for (dst, &src) in compressed[..SHA256_LEN - COMPRESSED_LEN]
                    .iter_mut()
                    .zip(&arr[COMPRESSED_LEN..])
                {
                    *dst ^= src;
                }
                base32::encode_mut(&compressed, &mut compressed_b32);
                &compressed_b32
            }
            PlaceholderHash::Other(slice) if slice.is_empty() => b"",
            PlaceholderHash::Other(slice) => {
                let mut compressed = [0u8; COMPRESSED_LEN];
                for chunk in slice.chunks(COMPRESSED_LEN) {
                    for (dst, &src) in compressed.iter_mut().zip(chunk) {
                        *dst ^= src;
                    }
                }
                base32::encode_mut(&compressed, &mut compressed_b32);
                &compressed_b32
            }
        };

        sha256_parts(&[
            b"nix-computed-output:",
            b32_slice,
            b":",
            output_name.as_ref().as_bytes(),
        ])
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

    /// Generate a placeholder for an output of a [`SingleDerivedPath`] without
    /// cloning intermediate store paths.
    pub(crate) fn from_single_derived_path(
        drv_path: &SingleDerivedPath,
        output_name: &OutputName,
    ) -> Self {
        match drv_path {
            SingleDerivedPath::Opaque(store_path) => Self::ca_output(store_path, output_name),
            SingleDerivedPath::Built { drv_path, output } => {
                let inner = Self::from_single_derived_path(drv_path, output);
                Self::dynamic_output(&inner, output_name)
            }
        }
    }
}

impl TryFrom<String> for Placeholder {
    type Error = std::io::Error;

    fn try_from(str: String) -> Result<Self, Self::Error> {
        if (str.len() * 5) % 8 >= 5 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Not valid nix base32 string: {str} (invalid length)"),
            ));
        }
        let decoded_len = base32::decode_len(str.len());
        let mut hash = if decoded_len == SHA256_LEN {
            PlaceholderHash::Sha256([0u8; SHA256_LEN])
        } else {
            PlaceholderHash::Other(vec![0u8; decoded_len].into_boxed_slice())
        };
        base32::decode_mut(str.as_bytes(), hash.as_mut_slice()).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Not valid nix base32 string: {str} (error: {:?})", err),
            )
        })?;

        Ok(Placeholder { hash })
    }
}

impl From<&SingleDerivedPath> for StorePathOrPlaceholder {
    fn from(path: &SingleDerivedPath) -> Self {
        match path {
            SingleDerivedPath::Opaque(store_path) => {
                StorePathOrPlaceholder::StorePath(store_path.clone())
            }
            SingleDerivedPath::Built { drv_path, output } => StorePathOrPlaceholder::Placeholder(
                Placeholder::from_single_derived_path(drv_path, output),
            ),
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

    #[test]
    fn test_placeholder_try_from_variants_and_slow_path() {
        let out: OutputName = "out".parse().unwrap();
        let expected = Placeholder::standard_output(&out);
        let b32 = expected
            .render()
            .to_string_lossy()
            .strip_prefix('/')
            .unwrap()
            .to_owned();
        let parsed = Placeholder::try_from(b32).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(
            StoreDir::default()
                .display(&StorePathOrPlaceholder::Placeholder(parsed.clone()))
                .to_string(),
            expected.render().to_string_lossy()
        );

        // Non-empty 20-byte placeholder (Other variant)
        let b32_20 = "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q".to_owned();
        let ph_20 = Placeholder::try_from(b32_20.clone()).unwrap();
        assert_eq!(ph_20.render(), PathBuf::from(format!("/{b32_20}")));
        assert_eq!(
            ph_20.render_bytes().as_ref(),
            format!("/{b32_20}").as_bytes()
        );
        assert_eq!(
            StoreDir::default()
                .display(&StorePathOrPlaceholder::Placeholder(ph_20.clone()))
                .to_string(),
            format!("/{b32_20}")
        );
        let dyn_20 = Placeholder::dynamic_output(&ph_20, &out);
        assert_eq!(
            dyn_20,
            Placeholder {
                hash: PlaceholderHash::Sha256(
                    *Sha256::digest(format!("nix-computed-output:{b32_20}:out")).digest_bytes()
                ),
            }
        );

        // Empty placeholder
        let empty = Placeholder::try_from(String::new()).unwrap();
        assert_eq!(empty.render(), PathBuf::from("/"));
        assert_eq!(empty.render_bytes().as_ref(), b"/");
        assert_eq!(
            Placeholder::dynamic_output(&empty, &out),
            sha256_parts(&[b"nix-computed-output::out"])
        );

        // Invalid base32 length returns Err instead of panicking
        assert!(Placeholder::try_from("abc".to_owned()).is_err());

        // Long name exercises sha256_parts_slow (> STACK_HASH_BUF_LEN = 128)
        let long_out: OutputName = "a".repeat(160).parse().unwrap();
        let long_ph = Placeholder::standard_output(&long_out);
        assert_eq!(
            long_ph,
            Placeholder {
                hash: PlaceholderHash::Sha256(
                    *Sha256::digest(format!("nix-output:{long_out}")).digest_bytes()
                ),
            }
        );
    }
}
