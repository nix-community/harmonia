use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use super::{NixSerialize, NixWrite};

impl<T> NixSerialize for Vec<T>
where
    T: NixSerialize + Send + Sync,
{
    #[allow(clippy::manual_async_fn)]
    fn serialize<W>(&self, writer: &mut W) -> impl Future<Output = Result<(), W::Error>> + Send
    where
        W: NixWrite,
    {
        async move {
            writer.write_value(&self.len()).await?;
            for value in self.iter() {
                writer.write_value(value).await?;
            }
            Ok(())
        }
    }
}

impl<T> NixSerialize for BTreeSet<T>
where
    T: NixSerialize + Send + Sync,
{
    #[allow(clippy::manual_async_fn)]
    fn serialize<W>(&self, writer: &mut W) -> impl Future<Output = Result<(), W::Error>> + Send
    where
        W: NixWrite,
    {
        async move {
            writer.write_value(&self.len()).await?;
            for value in self.iter() {
                writer.write_value(value).await?;
            }
            Ok(())
        }
    }
}

impl<K, V> NixSerialize for BTreeMap<K, V>
where
    K: NixSerialize + Ord + Send + Sync,
    V: NixSerialize + Send + Sync,
{
    #[allow(clippy::manual_async_fn)]
    fn serialize<W>(&self, writer: &mut W) -> impl Future<Output = Result<(), W::Error>> + Send
    where
        W: NixWrite,
    {
        async move {
            writer.write_value(&self.len()).await?;
            for (key, value) in self.iter() {
                writer.write_value(key).await?;
                writer.write_value(value).await?;
            }
            Ok(())
        }
    }
}

impl<T> NixSerialize for &[T]
where
    T: NixSerialize + Send + Sync,
{
    #[allow(clippy::manual_async_fn)]
    fn serialize<W>(&self, writer: &mut W) -> impl Future<Output = Result<(), W::Error>> + Send
    where
        W: NixWrite,
    {
        async move {
            writer.write_value(&self.len()).await?;
            for value in self.iter() {
                writer.write_value(value).await?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod unittests {
    use std::collections::BTreeMap;
    use std::fmt;

    use hex_literal::hex;
    use rstest::rstest;
    use tokio::io::AsyncWriteExt as _;
    use tokio_test::io::Builder;

    use crate::daemon::ser::{NixSerialize, NixWrite, NixWriter};

    #[rstest]
    #[case::empty(vec![], &hex!("0000 0000 0000 0000"))]
    #[case::one(vec![0x29], &hex!("0100 0000 0000 0000 2900 0000 0000 0000"))]
    #[case::two(vec![0x7469, 10], &hex!("0200 0000 0000 0000 6974 0000 0000 0000 0A00 0000 0000 0000"))]
    #[tokio::test]
    async fn test_write_small_vec(#[case] value: Vec<usize>, #[case] data: &[u8]) {
        let mock = Builder::new().write(data).build();
        let mut writer = NixWriter::new(mock);
        writer.write_value(&value).await.unwrap();
        writer.flush().await.unwrap();
    }

    fn empty_map() -> BTreeMap<usize, u64> {
        BTreeMap::new()
    }
    macro_rules! map {
        ($($key:expr => $value:expr),*) => {{
            let mut ret = BTreeMap::new();
            $(ret.insert($key, $value);)*
            ret
        }};
    }

    #[rstest]
    #[case::empty(empty_map(), &hex!("0000 0000 0000 0000"))]
    #[case::one(map![0x7469usize => 10u64], &hex!("0100 0000 0000 0000 6974 0000 0000 0000 0A00 0000 0000 0000"))]
    #[tokio::test]
    async fn test_write_small_btree_map<E>(#[case] value: E, #[case] data: &[u8])
    where
        E: NixSerialize + Send + PartialEq + fmt::Debug,
    {
        let mock = Builder::new().write(data).build();
        let mut writer = NixWriter::new(mock);
        writer.write_value(&value).await.unwrap();
        writer.flush().await.unwrap();
    }
}
