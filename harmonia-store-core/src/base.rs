use data_encoding::{BASE64, DecodePartial, HEXLOWER_PERMISSIVE};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::base32::{self};
use crate::wire::base64_len;

#[derive(derive_more::Display, Debug, PartialEq, Clone, Copy)]
pub enum Base {
    #[display("hex")]
    Hex,
    #[display("nixbase32")]
    NixBase32,
    #[display("base64")]
    Base64,
}

impl Serialize for Base {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Base {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "base16" | "hex" => Ok(Base::Hex),
            "base64" => Ok(Base::Base64),
            "nix32" | "nixbase32" => Ok(Base::NixBase32),
            _ => Err(serde::de::Error::unknown_variant(
                &s,
                &["base16", "hex", "base64", "nix32", "nixbase32"],
            )),
        }
    }
}

impl Base {
    /// Calculate the encoded string length for a given decoded byte size
    #[inline]
    pub const fn input_len(&self, decoded_size: usize) -> usize {
        match self {
            Base::Hex => decoded_size * 2,
            Base::NixBase32 => base32::encode_len(decoded_size),
            Base::Base64 => base64_len(decoded_size),
        }
    }

    /// Calculate the scratch buffer size needed for decoding
    #[inline]
    pub const fn scratch_len(&self, decoded_size: usize) -> usize {
        match self {
            Base::Hex => decoded_size,
            Base::NixBase32 => decoded_size,
            Base::Base64 => {
                // Base64 decoded size: (encoded_len / 4) * 3
                base64_len(decoded_size) / 4 * 3
            }
        }
    }
}

/// Get the decode function for a given base encoding
pub fn decode_for_base(
    base: Base,
) -> impl Fn(&[u8], &mut [u8]) -> Result<usize, DecodePartial> + 'static {
    match base {
        Base::Hex => {
            move |input: &[u8], output: &mut [u8]| HEXLOWER_PERMISSIVE.decode_mut(input, output)
        }
        Base::NixBase32 => move |input: &[u8], output: &mut [u8]| base32::decode_mut(input, output),
        Base::Base64 => move |input: &[u8], output: &mut [u8]| BASE64.decode_mut(input, output),
    }
}

/// Get the encode function for a given base encoding
pub fn encode_for_base(base: Base) -> impl Fn(&[u8], &mut [u8]) + 'static {
    match base {
        Base::Hex => {
            move |input: &[u8], output: &mut [u8]| HEXLOWER_PERMISSIVE.encode_mut(input, output)
        }
        Base::NixBase32 => move |input: &[u8], output: &mut [u8]| base32::encode_mut(input, output),
        Base::Base64 => move |input: &[u8], output: &mut [u8]| BASE64.encode_mut(input, output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_serde() {
        // Test serialization
        assert_eq!(serde_json::to_string(&Base::Hex).unwrap(), "\"hex\"");
        assert_eq!(
            serde_json::to_string(&Base::NixBase32).unwrap(),
            "\"nixbase32\""
        );
        assert_eq!(serde_json::to_string(&Base::Base64).unwrap(), "\"base64\"");

        // Test deserialization with canonical names
        assert_eq!(serde_json::from_str::<Base>("\"hex\"").unwrap(), Base::Hex);
        assert_eq!(
            serde_json::from_str::<Base>("\"nixbase32\"").unwrap(),
            Base::NixBase32
        );
        assert_eq!(
            serde_json::from_str::<Base>("\"base64\"").unwrap(),
            Base::Base64
        );

        // Test deserialization with aliases
        assert_eq!(
            serde_json::from_str::<Base>("\"base16\"").unwrap(),
            Base::Hex
        );
        assert_eq!(
            serde_json::from_str::<Base>("\"nix32\"").unwrap(),
            Base::NixBase32
        );

        // Test invalid format
        assert!(serde_json::from_str::<Base>("\"invalid\"").is_err());
    }
}
