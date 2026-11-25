//! Computes Nix content-addressed store path fingerprints:
//!
//! - `Text(sha256)`: `text:sha256:<hex>:<store_dir>:<name>`
//! - `NixArchive(sha256)`: `source:sha256:<hex>:<store_dir>:<name>`
//! - `NixArchive(other)`: `output:out:sha256:<sha256("fixed:out:r:<algo>:<hex>:")>:<store_dir>:<name>`
//! - `Flat(any)`: `output:out:sha256:<sha256("fixed:out:<algo>:<hex>:")>:<store_dir>:<name>`

use data_encoding::HEXLOWER;
use harmonia_store_path::{StoreDir, StorePathName};
use harmonia_utils_hash::{Algorithm, Context, Hash, HashView as _, Sha256};

use super::{ContentAddress, ContentAddressMethodAlgorithm};

/// Stack buffer ceiling for outer fingerprints: covers `/nix/store` (10 B) plus
/// the maximum 211-byte `StorePathName` (305 B worst case) with headroom.
const STACK_FINGERPRINT_CAP: usize = 320;

/// Hashes Nix's `fixed:out:<method><algo>:<hex>:` inner fingerprint on the stack.
fn fixed_output_inner_hash(method_algo: ContentAddressMethodAlgorithm, hash: &Hash) -> Sha256 {
    // Max length: "fixed:out:" (10) + "r:blake3" (8) + ":" (1) + 128 hex (sha512) + ":" (1) = 148.
    let mut buf = [0u8; 148];
    let prefix = b"fixed:out:";
    let algo = method_algo.as_str().as_bytes();
    let digest = hash.digest_bytes();
    let hex_len = digest.len() * 2;

    let mut pos = 0;
    buf[pos..pos + prefix.len()].copy_from_slice(prefix);
    pos += prefix.len();
    buf[pos..pos + algo.len()].copy_from_slice(algo);
    pos += algo.len();
    buf[pos] = b':';
    pos += 1;
    HEXLOWER.encode_mut(digest, &mut buf[pos..pos + hex_len]);
    pos += hex_len;
    buf[pos] = b':';
    pos += 1;

    Sha256::digest(&buf[..pos])
}

#[cold]
#[inline(never)]
fn hash_ca_fingerprint_slow(
    prefix: &[u8],
    inner: &Sha256,
    dir_bytes: &[u8],
    name_bytes: &[u8],
) -> Sha256 {
    // Max header: "output:out:sha256:" (18) + 64 hex + ":" (1) = 83 bytes.
    let mut header = [0u8; 84];
    let plen = prefix.len();
    header[..plen].copy_from_slice(prefix);
    HEXLOWER.encode_mut(inner.digest_bytes(), &mut header[plen..plen + 64]);
    header[plen + 64] = b':';

    let mut ctx = Context::new(Algorithm::SHA256);
    ctx.update(&header[..plen + 65]);
    ctx.update(dir_bytes);
    ctx.update(b":");
    ctx.update(name_bytes);
    ctx.finish()
        .try_into()
        .expect("SHA-256 context yields SHA-256")
}

/// Computes the SHA-256 fingerprint for `make_store_path_from_ca` without heap allocation.
pub(crate) fn hash_ca_fingerprint(
    store_dir: &StoreDir,
    name: &StorePathName,
    ca: ContentAddress,
) -> Sha256 {
    let (prefix, inner): (&[u8], Sha256) = match ca {
        ContentAddress::Text(digest) => (b"text:sha256:", digest),
        ContentAddress::NixArchive(Hash::SHA256(digest)) => (b"source:sha256:", digest),
        ContentAddress::NixArchive(hash) | ContentAddress::Flat(hash) => (
            b"output:out:sha256:",
            fixed_output_inner_hash(ca.method_algorithm(), &hash),
        ),
    };

    let dir_bytes = store_dir.to_str().as_bytes();
    let name_bytes = name.as_bytes();

    // "<prefix><64-hex>:<store_dir>:<name>"
    let total = prefix
        .len()
        .saturating_add(64 + 1)
        .saturating_add(dir_bytes.len())
        .saturating_add(1)
        .saturating_add(name_bytes.len());

    if total <= STACK_FINGERPRINT_CAP {
        let mut buf = [0u8; STACK_FINGERPRINT_CAP];
        let mut pos = 0;
        buf[pos..pos + prefix.len()].copy_from_slice(prefix);
        pos += prefix.len();
        HEXLOWER.encode_mut(inner.digest_bytes(), &mut buf[pos..pos + 64]);
        pos += 64;
        buf[pos] = b':';
        pos += 1;
        buf[pos..pos + dir_bytes.len()].copy_from_slice(dir_bytes);
        pos += dir_bytes.len();
        buf[pos] = b':';
        pos += 1;
        buf[pos..pos + name_bytes.len()].copy_from_slice(name_bytes);
        pos += name_bytes.len();
        Sha256::digest(&buf[..pos])
    } else {
        hash_ca_fingerprint_slow(prefix, &inner, dir_bytes, name_bytes)
    }
}
