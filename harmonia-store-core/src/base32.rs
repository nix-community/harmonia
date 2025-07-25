// This implementation is based on the nix32 crate
// https://crates.io/crates/nix32

// omitted: E O U T
const BASE32_CHARS: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";

/// Converts the given byte slice to a nix-compatible base32 encoded Vec<u8>.
pub fn to_nix_base32(bytes: &[u8]) -> Vec<u8> {
    let len = (bytes.len() * 8 - 1) / 5 + 1;

    (0..len)
        .rev()
        .map(|n| {
            let b: usize = n * 5;
            let i: usize = b / 8;
            let j: usize = b % 8;
            // bits from the lower byte
            let v1 = if i < bytes.len() {
                bytes[i].checked_shr(j as u32).unwrap_or(0)
            } else {
                0
            };
            // bits from the upper byte
            let v2 = if i + 1 < bytes.len() {
                bytes[i + 1].checked_shl((8 - j) as u32).unwrap_or(0)
            } else {
                0
            };
            let v: usize = ((v1 | v2) & 0x1f) as usize;
            BASE32_CHARS[v]
        })
        .collect()
}

/// Decodes nix base32 bytes to bytes
pub fn from_nix_base32(input: &[u8]) -> Result<Vec<u8>, String> {
    let output_len = (input.len() * 5) / 8;
    let mut output = vec![0u8; output_len];

    for (i, &c) in input.iter().rev().enumerate() {
        let digit = BASE32_CHARS
            .iter()
            .position(|&b| b == c)
            .ok_or_else(|| format!("Invalid base32 character: {}", c as char))?;

        let b = i * 5;
        let i = b / 8;
        let j = b % 8;

        if i < output_len {
            output[i] |= (digit as u8) << j;

            if i + 1 < output_len && j > 3 {
                output[i + 1] |= (digit as u8) >> (8 - j);
            }
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nix_base32_roundtrip() {
        let data = b"hello world";
        let encoded = to_nix_base32(data);
        let decoded = from_nix_base32(&encoded).unwrap();
        assert_eq!(data.to_vec(), decoded);
    }
}
