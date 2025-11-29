use bytes::Bytes;

use super::{NixSerialize, NixWrite};

impl NixSerialize for Bytes {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_slice(self).await
    }
}

impl NixSerialize for &[u8] {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_slice(self).await
    }
}

impl NixSerialize for String {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_slice(self.as_bytes()).await
    }
}

impl NixSerialize for str {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_slice(self.as_bytes()).await
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
    #[case::empty("", &hex!("0000 0000 0000 0000"))]
    #[case::one(")", &hex!("0100 0000 0000 0000 2900 0000 0000 0000"))]
    #[case::two("it", &hex!("0200 0000 0000 0000 6974 0000 0000 0000"))]
    #[case::three("tea", &hex!("0300 0000 0000 0000 7465 6100 0000 0000"))]
    #[case::four("were", &hex!("0400 0000 0000 0000 7765 7265 0000 0000"))]
    #[case::five("where", &hex!("0500 0000 0000 0000 7768 6572 6500 0000"))]
    #[case::six("unwrap", &hex!("0600 0000 0000 0000 756E 7772 6170 0000"))]
    #[case::seven("where's", &hex!("0700 0000 0000 0000 7768 6572 6527 7300"))]
    #[case::aligned("read_tea", &hex!("0800 0000 0000 0000 7265 6164 5F74 6561"))]
    #[case::more_bytes("read_tess", &hex!("0900 0000 0000 0000 7265 6164 5F74 6573 7300 0000 0000 0000"))]
    #[case::utf8("The quick brown 🦊 jumps over 13 lazy 🐶.", &hex!("2D00 0000 0000 0000  5468 6520 7175 6963  6b20 6272 6f77 6e20  f09f a68a 206a 756d  7073 206f 7665 7220  3133 206c 617a 7920  f09f 90b6 2e00 0000"))]
    #[tokio::test]
    async fn test_write_str(#[case] value: &str, #[case] data: &[u8]) {
        let mock = Builder::new().write(data).build();
        let mut writer = NixWriter::new(mock);
        writer.write_value(value).await.unwrap();
        writer.flush().await.unwrap();
    }

    #[rstest]
    #[case::empty("", &hex!("0000 0000 0000 0000"))]
    #[case::one(")", &hex!("0100 0000 0000 0000 2900 0000 0000 0000"))]
    #[case::two("it", &hex!("0200 0000 0000 0000 6974 0000 0000 0000"))]
    #[case::three("tea", &hex!("0300 0000 0000 0000 7465 6100 0000 0000"))]
    #[case::four("were", &hex!("0400 0000 0000 0000 7765 7265 0000 0000"))]
    #[case::five("where", &hex!("0500 0000 0000 0000 7768 6572 6500 0000"))]
    #[case::six("unwrap", &hex!("0600 0000 0000 0000 756E 7772 6170 0000"))]
    #[case::seven("where's", &hex!("0700 0000 0000 0000 7768 6572 6527 7300"))]
    #[case::aligned("read_tea", &hex!("0800 0000 0000 0000 7265 6164 5F74 6561"))]
    #[case::more_bytes("read_tess", &hex!("0900 0000 0000 0000 7265 6164 5F74 6573 7300 0000 0000 0000"))]
    #[case::utf8("The quick brown 🦊 jumps over 13 lazy 🐶.", &hex!("2D00 0000 0000 0000  5468 6520 7175 6963  6b20 6272 6f77 6e20  f09f a68a 206a 756d  7073 206f 7665 7220  3133 206c 617a 7920  f09f 90b6 2e00 0000"))]
    #[tokio::test]
    async fn test_write_string(#[case] value: &str, #[case] data: &[u8]) {
        let mock = Builder::new().write(data).build();
        let mut writer = NixWriter::new(mock);
        writer.write_value(&value.to_string()).await.unwrap();
        writer.flush().await.unwrap();
    }
}
