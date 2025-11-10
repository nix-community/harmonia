use crate::nix_serialize_remote;

use super::{Error, NixSerialize, NixWrite};

impl NixSerialize for u64 {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_number(*self).await
    }
}

impl NixSerialize for usize {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        let v = (*self).try_into().map_err(W::Error::unsupported_data)?;
        writer.write_number(v).await
    }
}

/*
nix_serialize_remote!(
    #[nix(into = "u64")]
    u8
);
*/
nix_serialize_remote!(
    #[nix(into = "u64")]
    u16
);
nix_serialize_remote!(
    #[nix(into = "u64")]
    u32
);

impl NixSerialize for bool {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        if *self {
            writer.write_number(1).await
        } else {
            writer.write_number(0).await
        }
    }
}

impl NixSerialize for i64 {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_number(*self as u64).await
    }
}

#[cfg(test)]
mod unittests {
    use hex_literal::hex;
    use rstest::rstest;
    use tokio::io::AsyncWriteExt as _;
    use tokio_test::io::Builder;

    use crate::daemon::ser::{NixWrite, NixWriter};

    #[rstest]
    #[case::simple_false(false, &hex!("0000 0000 0000 0000"))]
    #[case::simple_true(true, &hex!("0100 0000 0000 0000"))]
    #[tokio::test]
    async fn test_write_bool(#[case] value: bool, #[case] expected: &[u8]) {
        let mock = Builder::new().write(expected).build();
        let mut writer = NixWriter::new(mock);
        writer.write_value(&value).await.unwrap();
        writer.flush().await.unwrap();
    }

    #[rstest]
    #[case::zero(0, &hex!("0000 0000 0000 0000"))]
    #[case::one(1, &hex!("0100 0000 0000 0000"))]
    #[case::other(0x563412, &hex!("1234 5600 0000 0000"))]
    #[case::max_value(u64::MAX, &hex!("FFFF FFFF FFFF FFFF"))]
    #[tokio::test]
    async fn test_write_u64(#[case] value: u64, #[case] expected: &[u8]) {
        let mock = Builder::new().write(expected).build();
        let mut writer = NixWriter::new(mock);
        writer.write_value(&value).await.unwrap();
        writer.flush().await.unwrap();
    }

    #[rstest]
    #[case::zero(0, &hex!("0000 0000 0000 0000"))]
    #[case::one(1, &hex!("0100 0000 0000 0000"))]
    #[case::other(0x563412, &hex!("1234 5600 0000 0000"))]
    #[case::max_value(usize::MAX, &usize::MAX.to_le_bytes())]
    #[tokio::test]
    async fn test_write_usize(#[case] value: usize, #[case] expected: &[u8]) {
        let mock = Builder::new().write(expected).build();
        let mut writer = NixWriter::new(mock);
        writer.write_value(&value).await.unwrap();
        writer.flush().await.unwrap();
    }
}
