use crate::config::SigningKey;
use crate::error::{CacheError, IoErrorContext, Result, SigningError};
use base64::{engine::general_purpose, Engine};
use ed25519_dalek::{Signer, SigningKey as DalekSigningKey};
use harmonia_store_remote::protocol::StorePath;
use std::path::Path;

// this is from the nix32 crate

// omitted: E O U T
const BASE32_CHARS: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";

/// Converts the given byte slice to a nix-compatible base32 encoded Vec<u8>.
fn to_nix_base32(bytes: &[u8]) -> Vec<u8> {
    let len = (bytes.len() * 8 - 1) / 5 + 1;

    (0..len)
        .rev()
        .map(|n| {
            let b: usize = n * 5;
            let i: usize = b / 8;
            let j: usize = b % 8;
            // bits from the lower byte
            let v1 = bytes[i].checked_shr(j as u32).unwrap_or(0);
            // bits from the upper byte
            let v2 = if i >= bytes.len() - 1 {
                0
            } else {
                bytes[i + 1].checked_shl(8 - j as u32).unwrap_or(0)
            };
            let v: usize = (v1 | v2) as usize;
            BASE32_CHARS[v % BASE32_CHARS.len()]
        })
        .collect()
}

fn val(c: u8, idx: usize) -> Result<u8> {
    match c {
        b'A'..=b'F' => Ok(c - b'A' + 10),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'0'..=b'9' => Ok(c - b'0'),
        _ => Err(SigningError::Operation {
            reason: format!("invalid hex character: c: {}, index: {}", c as char, idx),
        }
        .into()),
    }
}

fn from_hex<T: AsRef<[u8]>>(hex: T) -> Result<Vec<u8>> {
    let hex = hex.as_ref();
    if hex.len() % 2 != 0 {
        return Err(SigningError::Operation {
            reason: "Odd length hex string".to_string(),
        }
        .into());
    }

    hex.chunks(2)
        .enumerate()
        .map(|(i, pair)| Ok(val(pair[0], 2 * i)? << 4 | val(pair[1], 2 * i + 1)?))
        .collect()
}

pub(crate) fn convert_base16_to_nix32(hash_bytes: &[u8]) -> Result<Vec<u8>> {
    let bytes = from_hex(hash_bytes).map_err(|e| {
        // Extract just the error message to avoid nested "Signing error: Signing operation failed:" prefixes
        let error_msg = if let CacheError::Signing(SigningError::Operation { reason }) = &e {
            reason.clone()
        } else {
            e.to_string()
        };
        SigningError::Operation {
            reason: format!(
                "Failed to convert hash '{}': {}",
                String::from_utf8_lossy(hash_bytes),
                error_msg
            ),
        }
    })?;
    Ok(to_nix_base32(&bytes))
}

pub(crate) fn parse_secret_key(path: &Path) -> Result<SigningKey> {
    let sign_key = std::fs::read_to_string(path)
        .io_context(format!("Couldn't read sign_key file: {}", path.display()))?;
    let (sign_name, sign_key64) =
        sign_key
            .split_once(':')
            .ok_or_else(|| SigningError::ParseKey {
                reason: "Sign key does not contain a ':'".to_string(),
            })?;
    let sign_keyno64 = general_purpose::STANDARD
        .decode(sign_key64.trim())
        .map_err(SigningError::Base64Decode)?;

    if sign_keyno64.len() == 64 || sign_keyno64.len() == 32 {
        if sign_keyno64.len() == 32 {
            let key_bytes: [u8; 32] =
                sign_keyno64
                    .as_slice()
                    .try_into()
                    .map_err(|_| SigningError::ParseKey {
                        reason: "Failed to convert key bytes to [u8; 32]".to_string(),
                    })?;
            let _ = DalekSigningKey::from_bytes(&key_bytes);
        } else if sign_keyno64.len() == 64 {
            let keypair_bytes: [u8; 64] =
                sign_keyno64
                    .as_slice()
                    .try_into()
                    .map_err(|_| SigningError::ParseKey {
                        reason: "Failed to convert key bytes to [u8; 64]".to_string(),
                    })?;
            let _ = DalekSigningKey::from_keypair_bytes(&keypair_bytes).map_err(|e| {
                SigningError::ParseKey {
                    reason: format!("Invalid Ed25519 keypair: {e}"),
                }
            })?;
        }

        return Ok(SigningKey {
            name: sign_name.to_string(),
            key: sign_keyno64,
        });
    }

    Err(SigningError::ParseKey {
        reason: format!(
            "Invalid signing key. Expected 32 or 64 bytes, got {}",
            sign_keyno64.len()
        ),
    }
    .into())
}

pub(crate) fn sign_string(sign_key: &SigningKey, msg: &[u8]) -> Vec<u8> {
    let dalek_key = if sign_key.key.len() == 32 {
        let key_bytes: [u8; 32] = sign_key
            .key
            .as_slice()
            .try_into()
            .expect("Invalid key length for Ed25519");
        DalekSigningKey::from_bytes(&key_bytes)
    } else if sign_key.key.len() == 64 {
        let keypair_bytes: [u8; 64] = sign_key
            .key
            .as_slice()
            .try_into()
            .expect("Invalid key length for Ed25519 keypair");
        DalekSigningKey::from_keypair_bytes(&keypair_bytes).expect("Invalid Ed25519 keypair")
    } else {
        panic!("Invalid signing key length: {}", sign_key.key.len());
    };

    let signature = dalek_key.sign(msg);
    let base64 = general_purpose::STANDARD.encode(signature.to_bytes());

    crate::build_bytes!(sign_key.name.as_bytes(), b":", base64.as_bytes())
}

pub(crate) fn fingerprint_path(
    virtual_nix_store: &[u8],
    store_path: &StorePath,
    nar_hash: &[u8],
    nar_size: u64,
    refs: &[StorePath],
) -> Result<Option<Vec<u8>>> {
    let store_path_bytes = store_path.as_bytes();
    if store_path_bytes.len() < virtual_nix_store.len() {
        return Err(SigningError::InvalidSignature {
            reason: "store path too short".to_string(),
        }
        .into());
    }
    if &store_path_bytes[0..virtual_nix_store.len()] != virtual_nix_store {
        return Err(SigningError::InvalidSignature {
            reason: "store path does not start with store dir".to_string(),
        }
        .into());
    }

    if !nar_hash.starts_with(b"sha256:") {
        return Err(SigningError::InvalidSignature {
            reason: "nar hash must start with sha256:".to_string(),
        }
        .into());
    }

    if nar_hash.len() != 59 {
        return Err(SigningError::InvalidSignature {
            reason: format!(
                "nar has not the right length, expected 59, got {}",
                nar_hash.len()
            ),
        }
        .into());
    }

    for r in refs {
        let r_bytes = r.as_bytes();
        if &r_bytes[0..virtual_nix_store.len()] != virtual_nix_store {
            return Err(SigningError::InvalidSignature {
                reason: "ref path invalid".to_string(),
            }
            .into());
        }
    }

    let nar_size_str = nar_size.to_string();
    let nar_size_bytes = nar_size_str.as_bytes();

    // Build parts slice for the fixed portion
    let parts: &[&[u8]] = &[
        b"1;",
        store_path_bytes,
        b";",
        nar_hash,
        b";",
        nar_size_bytes,
        b";",
    ];

    // Calculate total capacity including references
    let fixed_len: usize = parts.iter().map(|p| p.len()).sum();
    let refs_len = if refs.is_empty() {
        0
    } else {
        refs.iter().map(|r| r.as_bytes().len()).sum::<usize>() + refs.len().saturating_sub(1)
        // commas between refs
    };

    let mut result = Vec::with_capacity(fixed_len + refs_len);

    // Add fixed parts
    for part in parts {
        result.extend_from_slice(part);
    }

    // Add references if present (comma-separated)
    for (i, r) in refs.iter().enumerate() {
        if i > 0 {
            result.push(b',');
        }
        result.extend_from_slice(r.as_bytes());
    }

    Ok(Some(result))
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::PathBuf;

    fn test_assets_path() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("..");
        path.push("tests");
        path
    }

    #[test]
    fn test_signing() -> Result<()> {
        let sign_key = test_assets_path().join("cache.sk");

        let store_path =
            StorePath::new(b"/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_vec());
        let references = vec![
            StorePath::new(b"/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_vec()),
            StorePath::new(b"/nix/store/sl141d1g77wvhr050ah87lcyz2czdxa3-glibc-2.40-36".to_vec()),
        ];
        let key = parse_secret_key(&sign_key)?;
        let finger_print = fingerprint_path(
            b"/nix/store",
            &store_path,
            b"sha256:1mkvday29m2qxg1fnbv8xh9s6151bh8a2xzhh0k86j7lqhyfwibh",
            226560,
            &references,
        )?;
        let signature = sign_string(&key, &finger_print.unwrap());
        assert_eq!(signature, b"cache.example.com-1:6wzr1QlOPHG+knFuJIaw+85Z5ivwbdI512JikexG+nQ7JDSZM2hw8zzlcLrguzoLEpCA9VzaEEQflZEHVwy9AA==");
        Ok(())
    }
}
