use anyhow::bail;
use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine};
use std::path::Path;

use crate::config::SigningKey;

// this is from the nix32 crate

// omitted: E O U T
const BASE32_CHARS: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";

#[link(name = "sodium")]
extern "C" {
    fn crypto_sign_detached(
        sig: *mut u8,
        sig_len: *mut usize,
        msg: *const u8,
        msg_len: usize,
        sk: *const u8,
    ) -> i32;
}

/// Converts the given byte slice to a nix-compatible base32 encoded String.
fn to_nix_base32(bytes: &[u8]) -> String {
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
            char::from(BASE32_CHARS[v % BASE32_CHARS.len()])
        })
        .collect()
}

fn val(c: u8, idx: usize) -> Result<u8> {
    match c {
        b'A'..=b'F' => Ok(c - b'A' + 10),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'0'..=b'9' => Ok(c - b'0'),
        _ => bail!("invalid hex character: c: {}, index: {}", c as char, idx),
    }
}

fn from_hex<T: AsRef<[u8]>>(hex: T) -> Result<Vec<u8>> {
    let hex = hex.as_ref();
    if hex.len() % 2 != 0 {
        bail!("Odd length");
    }

    hex.chunks(2)
        .enumerate()
        .map(|(i, pair)| Ok(val(pair[0], 2 * i)? << 4 | val(pair[1], 2 * i + 1)?))
        .collect()
}

pub(crate) fn convert_base16_to_nix32(hash_str: &str) -> Result<String> {
    let bytes =
        from_hex(hash_str).with_context(|| format!("Failed to convert hash: {}", hash_str))?;
    Ok(to_nix_base32(&bytes))
}

pub(crate) fn parse_secret_key(path: &Path) -> Result<SigningKey> {
    let sign_key = std::fs::read_to_string(path).context("Couldn't read sign_key file")?;
    let (sign_name, sign_key64) = sign_key
        .split_once(':')
        .context("Sign key does not contain a ':'")?;
    let sign_keyno64 = general_purpose::STANDARD
        .decode(sign_key64.trim())
        .context("Couldn't base64::decode sign key")?;
    if sign_keyno64.len() == 64 {
        return Ok(SigningKey {
            name: sign_name.to_string(),
            key: sign_keyno64,
        });
    }

    Err(anyhow::anyhow!(
        "Invalid signing key. Expected 64 bytes, got {}",
        sign_keyno64.len()
    ))
}

pub(crate) fn fingerprint_path(
    virtual_nix_store: &str,
    store_path: &str,
    nar_hash: &str,
    nar_size: u64,
    refs: &[String],
) -> Result<Option<String>> {
    if store_path.len() < virtual_nix_store.len() {
        bail!("store path too short");
    }
    if &store_path[0..virtual_nix_store.len()] != virtual_nix_store {
        bail!("store path does not start with store dir");
    }

    assert!(nar_hash.starts_with("sha256:"));

    if nar_hash.len() != 59 {
        bail!(
            "nar has not the right length, expected 59, got {}",
            nar_hash.len()
        );
    }

    for r in refs {
        if &r[0..virtual_nix_store.len()] != virtual_nix_store {
            bail!("ref path invalid");
        }
    }

    Ok(Some(format!(
        "1;{};{};{};{}",
        store_path,
        nar_hash,
        nar_size,
        refs.join(",")
    )))
}

pub(crate) fn sign_string(sign_key: &SigningKey, msg: &str) -> String {
    let mut signature = vec![0u8; 64]; // crypto_sign_BYTES -> 64
    let mut signature_len: usize = 0;
    let msg = msg.as_bytes();
    unsafe {
        crypto_sign_detached(
            signature.as_mut_ptr(),
            &mut signature_len,
            msg.as_ptr(),
            msg.len(),
            sign_key.key.as_ptr(),
        )
    };
    let base64 = general_purpose::STANDARD.encode(&signature[..signature_len]);
    format!("{}:{}", sign_key.name, base64)
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::Result;
    use std::path::PathBuf;

    fn test_assets_path() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("tests");
        path
    }

    #[test]
    fn test_signing() -> Result<()> {
        let sign_key = test_assets_path().join("cache.sk");

        let references = [
            String::from("/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1"),
            String::from("/nix/store/sl141d1g77wvhr050ah87lcyz2czdxa3-glibc-2.40-36"),
        ];
        let key = parse_secret_key(&sign_key)
            .with_context(|| format!("Could not parse signing key: {}", sign_key.display()))?;
        let finger_print = fingerprint_path(
            "/nix/store",
            "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1",
            "sha256:1mkvday29m2qxg1fnbv8xh9s6151bh8a2xzhh0k86j7lqhyfwibh",
            226560,
            references.as_ref(),
        )?;
        let signature = sign_string(&key, &finger_print.unwrap());
        assert_eq!(signature, "cache.example.com-1:6wzr1QlOPHG+knFuJIaw+85Z5ivwbdI512JikexG+nQ7JDSZM2hw8zzlcLrguzoLEpCA9VzaEEQflZEHVwy9AA==");
        Ok(())
    }
}
