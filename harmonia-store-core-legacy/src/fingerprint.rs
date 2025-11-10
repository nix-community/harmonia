use crate::StorePath;
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FingerprintError {
    #[error("Store path too short")]
    StorePathTooShort,

    #[error("Store path does not start with store dir")]
    InvalidStorePrefix,

    #[error("NAR hash must start with 'sha256:'")]
    InvalidNarHashPrefix,

    #[error("NAR hash has invalid length: expected 59, got {0}")]
    InvalidNarHashLength(usize),

    #[error("Reference path does not start with store dir")]
    InvalidReferencePrefix,
}

/// Generate a fingerprint for signing a store path
///
/// The fingerprint format is:
/// `1;<store-path>;<nar-hash>;<nar-size>;<comma-separated-references>`
///
/// # Arguments
/// * `store_dir` - The Nix store directory (e.g., b"/nix/store")
/// * `store_path` - The full store path to fingerprint
/// * `nar_hash` - The NAR hash in format "sha256:..."
/// * `nar_size` - The size of the NAR in bytes
/// * `references` - Sorted references to other store paths
pub fn fingerprint_path(
    store_dir: &[u8],
    store_path: &StorePath,
    nar_hash: &[u8],
    nar_size: u64,
    references: &BTreeSet<StorePath>,
) -> Result<Vec<u8>, FingerprintError> {
    // Validate store path
    let store_path_bytes = store_path.as_bytes();
    if store_path_bytes.len() < store_dir.len() {
        return Err(FingerprintError::StorePathTooShort);
    }
    if &store_path_bytes[0..store_dir.len()] != store_dir {
        return Err(FingerprintError::InvalidStorePrefix);
    }

    // Validate NAR hash
    if !nar_hash.starts_with(b"sha256:") {
        return Err(FingerprintError::InvalidNarHashPrefix);
    }
    if nar_hash.len() != 59 {
        return Err(FingerprintError::InvalidNarHashLength(nar_hash.len()));
    }

    // Validate references
    for reference in references {
        let ref_bytes = reference.as_bytes();
        if ref_bytes.len() < store_dir.len() {
            return Err(FingerprintError::StorePathTooShort);
        }
        if &ref_bytes[0..store_dir.len()] != store_dir {
            return Err(FingerprintError::InvalidReferencePrefix);
        }
    }

    // Build the fingerprint
    let nar_size_str = nar_size.to_string();
    let nar_size_bytes = nar_size_str.as_bytes();

    // Calculate capacity
    let fixed_len = 3 + // "1;"
        store_path.len() + 1 + // store path + ";"
        nar_hash.len() + 1 + // nar hash + ";"
        nar_size_bytes.len() + 1; // nar size + ";"

    let refs_len = if references.is_empty() {
        0
    } else {
        references.iter().map(|r| r.len()).sum::<usize>() + references.len().saturating_sub(1) // commas between refs
    };

    let mut result = Vec::with_capacity(fixed_len + refs_len);

    // Add fixed parts
    result.extend_from_slice(b"1;");
    result.extend_from_slice(store_path);
    result.push(b';');
    result.extend_from_slice(nar_hash);
    result.push(b';');
    result.extend_from_slice(nar_size_bytes);
    result.push(b';');

    // Add references (comma-separated)
    for (i, reference) in references.iter().enumerate() {
        if i > 0 {
            result.push(b',');
        }
        result.extend_from_slice(reference);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_basic() {
        let store_dir = b"/nix/store";
        let store_path = StorePath::from(
            b"/nix/store/syd87l2rxw8cbsxmxl853h0r6pdwhwjr-curl-7.82.0-bin" as &[u8],
        );
        let nar_hash = b"sha256:1b4sb93wp679q4zx9k1ignby1yna3z7c4c2ri3wphylbc2dwsys0";
        let nar_size = 196040;
        let mut references = BTreeSet::new();
        references.insert(StorePath::from(
            b"/nix/store/0jqd0rlxzra1rs38rdxl43yh6rxchgc6-curl-7.82.0" as &[u8],
        ));
        references.insert(StorePath::from(
            b"/nix/store/5dq2jj6d7k197p6fzqn8l5n0jfmhxmcg-glibc-2.33-59" as &[u8],
        ));

        let fingerprint =
            fingerprint_path(store_dir, &store_path, nar_hash, nar_size, &references).unwrap();
        let expected = b"1;/nix/store/syd87l2rxw8cbsxmxl853h0r6pdwhwjr-curl-7.82.0-bin;sha256:1b4sb93wp679q4zx9k1ignby1yna3z7c4c2ri3wphylbc2dwsys0;196040;/nix/store/0jqd0rlxzra1rs38rdxl43yh6rxchgc6-curl-7.82.0,/nix/store/5dq2jj6d7k197p6fzqn8l5n0jfmhxmcg-glibc-2.33-59";
        assert_eq!(fingerprint, expected);
    }

    #[test]
    fn test_fingerprint_no_references() {
        let store_dir = b"/nix/store";
        let store_path =
            StorePath::from(b"/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1" as &[u8]);
        let nar_hash = b"sha256:1mkvday29m2qxg1fnbv8xh9s6151bh8a2xzhh0k86j7lqhyfwibh";
        let nar_size = 226560;
        let references = BTreeSet::new();

        let fingerprint =
            fingerprint_path(store_dir, &store_path, nar_hash, nar_size, &references).unwrap();
        let expected = b"1;/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1;sha256:1mkvday29m2qxg1fnbv8xh9s6151bh8a2xzhh0k86j7lqhyfwibh;226560;";
        assert_eq!(fingerprint, expected);
    }

    #[test]
    fn test_invalid_nar_hash() {
        let references = BTreeSet::new();
        let store_path = StorePath::from(b"/nix/store/test" as &[u8]);
        let result = fingerprint_path(
            b"/nix/store",
            &store_path,
            b"sha512:abc", // Wrong algorithm
            100,
            &references,
        );
        assert!(matches!(
            result,
            Err(FingerprintError::InvalidNarHashPrefix)
        ));
    }
}
