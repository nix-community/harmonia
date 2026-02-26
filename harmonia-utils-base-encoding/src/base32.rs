// SPDX-FileCopyrightText: 2024 griff (original Nix.rs)
// SPDX-FileCopyrightText: 2026 Jörg Thalheim (Harmonia adaptation)
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Nix base32 encoding/decoding.

#[cfg(test)]
use data_encoding::DecodeKind;
use data_encoding::{BitOrder, DecodeError, DecodePartial, Encoding, Specification};
use std::sync::LazyLock;

/// The 32-character alphabet used by Nix's base32 encoding.
///
/// This is the canonical source for the nix-base32 alphabet across the
/// workspace — use this constant instead of duplicating the string.
pub const ALPHABET: &str = "0123456789abcdfghijklmnpqrsvwxyz";

/// The nix-base32 alphabet as a byte slice (convenience alias).
pub const ALPHABET_BYTES: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

/// Nix base32 encoding (lowercase, without padding, LSB first, reversed)
static NIX_BASE32: LazyLock<Encoding> = LazyLock::new(|| {
    let mut spec = Specification::new();
    spec.symbols.push_str(ALPHABET);
    spec.bit_order = BitOrder::LeastSignificantFirst;
    spec.encoding().unwrap()
});

pub const fn encode_len(len: usize) -> usize {
    (8 * len).div_ceil(5)
}

const fn decode_len_internal(len: usize) -> (usize, usize) {
    let trail = len * 5 % 8;
    (len - trail / 5, 5 * len / 8)
}

pub const fn decode_len(len: usize) -> usize {
    let (ilen, olen) = decode_len_internal(len);
    if ilen != len {
        panic!("Invalid base32 length");
    }
    olen
}

#[allow(unsafe_code)]
pub fn encode_string(input: &[u8]) -> String {
    let mut output = NIX_BASE32.encode(input);
    // Nix base32 is reversed
    unsafe { output.as_bytes_mut() }.reverse();
    output
}

pub fn encode_mut(input: &[u8], output: &mut [u8]) {
    // Encode directly into output buffer
    NIX_BASE32.encode_mut(input, output);

    // Reverse for Nix format
    output.reverse();
}

pub fn decode_mut(input: &[u8], output: &mut [u8]) -> Result<usize, DecodePartial> {
    // Reverse the input for decoding
    let mut reversed = input.to_vec();
    reversed.reverse();

    // Decode using data-encoding
    NIX_BASE32.decode_mut(&reversed, output).map_err(|err| {
        // Adjust error position to account for reversal
        let adjusted_pos = input.len() - err.error.position - 1;
        DecodePartial {
            read: adjusted_pos / 8 * 8,
            written: adjusted_pos / 8 * 5,
            error: DecodeError {
                position: adjusted_pos,
                kind: err.error.kind,
            },
        }
    })
}

#[cfg(test)]
mod unittests {
    use hex_literal::hex;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::empty("", &[])]
    #[case::one_0("00", &hex!("00"))]
    #[case::one_1("01", &hex!("01"))]
    #[case::one_2("02", &hex!("02"))]
    #[case::one_3("03", &hex!("03"))]
    #[case::one_4("04", &hex!("04"))]
    #[case::one_5("05", &hex!("05"))]
    #[case::one_6("06", &hex!("06"))]
    #[case::one_7("07", &hex!("07"))]
    #[case::one_8("08", &hex!("08"))]
    #[case::one_9("09", &hex!("09"))]
    #[case::one_a("0a", &hex!("0A"))]
    #[case::one_b("0b", &hex!("0B"))]
    #[case::one_d("0c", &hex!("0C"))]
    #[case::one_d("0d", &hex!("0D"))]
    #[case::one_e("0f", &hex!("0E"))]
    #[case::one_f("0g", &hex!("0F"))]
    #[case::one_10("0h", &hex!("10"))]
    #[case::one_11("0i", &hex!("11"))]
    #[case::one_12("0j", &hex!("12"))]
    #[case::one_13("0k", &hex!("13"))]
    #[case::one_14("0l", &hex!("14"))]
    #[case::one_15("0m", &hex!("15"))]
    #[case::one_16("0n", &hex!("16"))]
    #[case::one_17("0p", &hex!("17"))]
    #[case::one_18("0q", &hex!("18"))]
    #[case::one_19("0r", &hex!("19"))]
    #[case::one_1a("0s", &hex!("1a"))]
    #[case::one_1b("0v", &hex!("1b"))]
    #[case::one_1c("0w", &hex!("1c"))]
    #[case::one_1d("0x", &hex!("1d"))]
    #[case::one_1e("0y", &hex!("1e"))]
    #[case::one_1f("0z", &hex!("1f"))]
    #[case::one_20("10", &hex!("20"))]
    #[case::one_21("11", &hex!("21"))]
    #[case::one_22("12", &hex!("22"))]
    #[case::one_23("13", &hex!("23"))]
    #[case::one_24("14", &hex!("24"))]
    #[case::one_25("15", &hex!("25"))]
    #[case::one_26("16", &hex!("26"))]
    #[case::one_27("17", &hex!("27"))]
    #[case::one_28("18", &hex!("28"))]
    #[case::one_29("19", &hex!("29"))]
    #[case::one_2a("1a", &hex!("2a"))]
    #[case::one_2b("1b", &hex!("2b"))]
    #[case::one_2c("1c", &hex!("2c"))]
    #[case::one_2d("1d", &hex!("2d"))]
    #[case::one_2e("1f", &hex!("2e"))]
    #[case::one_2f("1g", &hex!("2f"))]
    #[case::two("0bqz", &hex!("1f2f"))]
    #[case::three("gy003", &hex!("0300 FF"))]
    #[case::four("0s14004", &hex!("0400 1234"))]
    #[case::five("aqs14005", &hex!("0500 1234 56"))]
    #[case::six("3qaqs14006", &hex!("0600 1234 5678"))]
    #[case::seven("16kqaqs14007", &hex!("0700 1234 5678 9A"))]
    #[case::eight("br6kqaqs14008", &hex!("0800 1234 5678 9ABC"))]
    #[case::nine("3gbr6kqaqs14009", &hex!("0900 1234 5678 9ABC DE"))]
    #[case::nix1("x0xf8v9fxf3jk8zln1cwlsrmhqvp0f88", &hex!("0839 7037 8635 6bca 59b0 f4a3 2987 eb2e 6de4 3ae8"))]
    #[case::nix1("1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s", &hex!("ba78 16bf 8f01 cfea 4141 40de 5dae 2223 b003 61a3 9617 7a9c b410 ff61 f200 15ad"))]
    #[case::nix1("2gs8k559z4rlahfx0y688s49m2vvszylcikrfinm30ly9rak69236nkam5ydvly1ai7xac99vxfc4ii84hawjbk876blyk1jfhkbbyx", &hex!("ddaf 35a1 9361 7aba cc41 7349 ae20 4131 12e6 fa4e 89a9 7ea2 0a9e eee6 4b55 d39a 2192 992a 274f c1a8 36ba 3c23 a3fe ebbd 454d 4423 643c e80e 2a9a c94f a54c a49f"))]
    #[case::nix1("x0xf8v9fxf3jk8zln1cwlsrmhqvp0f88", &hex!("0839 7037 8635 6bca 59b0 f4a3 2987 eb2e 6de4 3ae8"))]
    fn test_encode_bytes(#[case] expected: &str, #[case] data: &[u8]) {
        // Test encode_string
        let actual = encode_string(data);
        assert_eq!(&actual[..], expected);

        // Test encode_mut
        let mut output = vec![0u8; encode_len(data.len())];
        encode_mut(data, &mut output);
        let actual2 = String::from_utf8(output).unwrap();
        assert_eq!(actual2, expected);
    }

    #[rstest]
    #[case::empty("", &[])]
    #[case::one_0("00", &hex!("00"))]
    #[case::one_1("01", &hex!("01"))]
    #[case::one_2("02", &hex!("02"))]
    #[case::one_3("03", &hex!("03"))]
    #[case::one_4("04", &hex!("04"))]
    #[case::one_5("05", &hex!("05"))]
    #[case::one_6("06", &hex!("06"))]
    #[case::one_7("07", &hex!("07"))]
    #[case::one_8("08", &hex!("08"))]
    #[case::one_9("09", &hex!("09"))]
    #[case::one_a("0a", &hex!("0A"))]
    #[case::one_b("0b", &hex!("0B"))]
    #[case::one_d("0c", &hex!("0C"))]
    #[case::one_d("0d", &hex!("0D"))]
    #[case::one_e("0f", &hex!("0E"))]
    #[case::one_f("0g", &hex!("0F"))]
    #[case::one_10("0h", &hex!("10"))]
    #[case::one_11("0i", &hex!("11"))]
    #[case::one_12("0j", &hex!("12"))]
    #[case::one_13("0k", &hex!("13"))]
    #[case::one_14("0l", &hex!("14"))]
    #[case::one_15("0m", &hex!("15"))]
    #[case::one_16("0n", &hex!("16"))]
    #[case::one_17("0p", &hex!("17"))]
    #[case::one_18("0q", &hex!("18"))]
    #[case::one_19("0r", &hex!("19"))]
    #[case::one_1a("0s", &hex!("1a"))]
    #[case::one_1b("0v", &hex!("1b"))]
    #[case::one_1c("0w", &hex!("1c"))]
    #[case::one_1d("0x", &hex!("1d"))]
    #[case::one_1e("0y", &hex!("1e"))]
    #[case::one_1f("0z", &hex!("1f"))]
    #[case::one_20("10", &hex!("20"))]
    #[case::one_21("11", &hex!("21"))]
    #[case::one_22("12", &hex!("22"))]
    #[case::one_23("13", &hex!("23"))]
    #[case::one_24("14", &hex!("24"))]
    #[case::one_25("15", &hex!("25"))]
    #[case::one_26("16", &hex!("26"))]
    #[case::one_27("17", &hex!("27"))]
    #[case::one_28("18", &hex!("28"))]
    #[case::one_29("19", &hex!("29"))]
    #[case::one_2a("1a", &hex!("2a"))]
    #[case::one_2b("1b", &hex!("2b"))]
    #[case::one_2c("1c", &hex!("2c"))]
    #[case::one_2d("1d", &hex!("2d"))]
    #[case::one_2e("1f", &hex!("2e"))]
    #[case::one_2f("1g", &hex!("2f"))]
    #[case::two("0bqz", &hex!("1f2f"))]
    #[case::three("gy003", &hex!("0300 FF"))]
    #[case::four("0s14004", &hex!("0400 1234"))]
    #[case::five("aqs14005", &hex!("0500 1234 56"))]
    #[case::six("3qaqs14006", &hex!("0600 1234 5678"))]
    #[case::seven("16kqaqs14007", &hex!("0700 1234 5678 9A"))]
    #[case::eight("br6kqaqs14008", &hex!("0800 1234 5678 9ABC"))]
    #[case::nine("3gbr6kqaqs14009", &hex!("0900 1234 5678 9ABC DE"))]
    #[case::nix1("x0xf8v9fxf3jk8zln1cwlsrmhqvp0f88", &hex!("0839 7037 8635 6bca 59b0 f4a3 2987 eb2e 6de4 3ae8"))]
    #[case::nix1("1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s", &hex!("ba78 16bf 8f01 cfea 4141 40de 5dae 2223 b003 61a3 9617 7a9c b410 ff61 f200 15ad"))]
    #[case::nix1("2gs8k559z4rlahfx0y688s49m2vvszylcikrfinm30ly9rak69236nkam5ydvly1ai7xac99vxfc4ii84hawjbk876blyk1jfhkbbyx", &hex!("ddaf 35a1 9361 7aba cc41 7349 ae20 4131 12e6 fa4e 89a9 7ea2 0a9e eee6 4b55 d39a 2192 992a 274f c1a8 36ba 3c23 a3fe ebbd 454d 4423 643c e80e 2a9a c94f a54c a49f"))]
    #[case::nix1("x0xf8v9fxf3jk8zln1cwlsrmhqvp0f88", &hex!("0839 7037 8635 6bca 59b0 f4a3 2987 eb2e 6de4 3ae8"))]
    fn test_decode_bytes(#[case] data: &str, #[case] expected: &[u8]) {
        let mut output = vec![0u8; decode_len(data.len())];
        decode_mut(data.as_bytes(), &mut output).unwrap();
        assert_eq!(output, expected);
    }

    fn fail(pos: usize, kind: DecodeKind) -> Result<usize, DecodePartial> {
        Err(DecodePartial {
            read: pos / 8 * 8,
            written: pos / 8 * 5,
            error: DecodeError {
                position: pos,
                kind,
            },
        })
    }

    #[rstest]
    #[case::invalid_trailer_1("zz", fail(0, DecodeKind::Trailing))]
    #[case::invalid_trailer_2("c0", fail(0, DecodeKind::Trailing))]
    #[case::invalid_char_0("|czz0", fail(0, DecodeKind::Symbol))]
    #[case::invalid_char_1("c|zz0", fail(1, DecodeKind::Symbol))]
    #[case::invalid_char_2("cz|z0", fail(2, DecodeKind::Symbol))]
    #[case::invalid_char_3("czz|0", fail(3, DecodeKind::Symbol))]
    #[case::invalid_char_4("czz0|", fail(4, DecodeKind::Symbol))]
    #[case::invalid_char_10("czzzzzzzzz|0", fail(10, DecodeKind::Symbol))]
    #[case::invalid_char_chunk_2("c|zzzzzzzzz0", fail(1, DecodeKind::Symbol))]
    fn test_decode_bytes_fail(#[case] data: &str, #[case] expected: Result<usize, DecodePartial>) {
        let mut output = vec![0u8; decode_len(data.len())];
        assert_eq!(decode_mut(data.as_bytes(), &mut output), expected);
    }
}

#[cfg(test)]
mod proptests {
    use proptest::{prop_assert_eq, proptest};

    use super::*;

    proptest! {
        #[test]
        fn proptest_roundtrip(data: Vec<u8>) {
            let mut encoded = vec![0u8; encode_len(data.len())];
            encode_mut(&data, &mut encoded);

            let mut decoded = vec![0u8; decode_len(encoded.len())];
            decode_mut(&encoded, &mut decoded).unwrap();
            prop_assert_eq!(data, decoded);
        }
    }
}
