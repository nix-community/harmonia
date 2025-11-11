use data_encoding::{BASE64, DecodePartial, HEXLOWER_PERMISSIVE};

use crate::base32::{self};

#[derive(derive_more::Display, Debug, PartialEq, Clone, Copy)]
pub enum Base {
    #[display("hex")]
    Hex,
    #[display("nixbase32")]
    NixBase32,
    #[display("base64")]
    Base64,
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
