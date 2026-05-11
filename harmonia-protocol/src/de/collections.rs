use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use super::{NixDeserialize, NixRead};

/// Cap initial `Vec` preallocation when deserializing length-prefixed
/// collections. The length comes from an untrusted peer, so a huge value would
/// otherwise trigger an unbounded allocation and abort the process. The Vec
/// still grows beyond this if the peer actually sends that many elements.
const MAX_PREALLOC_ELEMS: usize = 64 * 1024;

impl<T> NixDeserialize for Vec<T>
where
    T: NixDeserialize + Send,
{
    #[allow(clippy::manual_async_fn)]
    fn try_deserialize<R>(
        reader: &mut R,
    ) -> impl Future<Output = Result<Option<Self>, R::Error>> + Send + '_
    where
        R: ?Sized + NixRead + Send,
    {
        async move {
            if let Some(len) = reader.try_read_value::<usize>().await? {
                let mut ret = Vec::with_capacity(len.min(MAX_PREALLOC_ELEMS));
                for _ in 0..len {
                    ret.push(reader.read_value().await?);
                }
                Ok(Some(ret))
            } else {
                Ok(None)
            }
        }
    }
}

impl<T> NixDeserialize for BTreeSet<T>
where
    T: NixDeserialize + Ord + Send,
{
    #[allow(clippy::manual_async_fn)]
    fn try_deserialize<R>(
        reader: &mut R,
    ) -> impl Future<Output = Result<Option<Self>, R::Error>> + Send + '_
    where
        R: ?Sized + NixRead + Send,
    {
        async move {
            if let Some(len) = reader.try_read_value::<usize>().await? {
                let mut ret = BTreeSet::new();
                for _ in 0..len {
                    ret.insert(reader.read_value().await?);
                }
                Ok(Some(ret))
            } else {
                Ok(None)
            }
        }
    }
}

impl<K, V> NixDeserialize for BTreeMap<K, V>
where
    K: NixDeserialize + Ord + Send,
    V: NixDeserialize + Send,
{
    #[allow(clippy::manual_async_fn)]
    fn try_deserialize<R>(
        reader: &mut R,
    ) -> impl Future<Output = Result<Option<Self>, R::Error>> + Send + '_
    where
        R: ?Sized + NixRead + Send,
    {
        async move {
            if let Some(len) = reader.try_read_value::<usize>().await? {
                let mut ret = BTreeMap::new();
                for _ in 0..len {
                    let key = reader.read_value().await?;
                    let value = reader.read_value().await?;
                    ret.insert(key, value);
                }
                Ok(Some(ret))
            } else {
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod unittests {
    use std::collections::BTreeMap;
    use std::fmt;

    use hex_literal::hex;
    use rstest::rstest;
    use tokio_test::io::Builder;

    use crate::daemon::de::{NixDeserialize, NixRead, NixReader};

    #[rstest]
    #[case::empty(vec![], &hex!("0000 0000 0000 0000"))]
    #[case::one(vec![0x29], &hex!("0100 0000 0000 0000 2900 0000 0000 0000"))]
    #[case::two(vec![0x7469, 10], &hex!("0200 0000 0000 0000 6974 0000 0000 0000 0A00 0000 0000 0000"))]
    #[tokio::test]
    async fn test_read_small_vec(#[case] expected: Vec<usize>, #[case] data: &[u8]) {
        let mock = Builder::new().read(data).build();
        let mut reader = NixReader::new(mock);
        let actual: Vec<usize> = reader.read_value().await.unwrap();
        assert_eq!(actual, expected);
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
    async fn test_read_small_btree_map<E>(#[case] expected: E, #[case] data: &[u8])
    where
        E: NixDeserialize + PartialEq + fmt::Debug,
    {
        let mock = Builder::new().read(data).build();
        let mut reader = NixReader::new(mock);
        let actual: E = reader.read_value().await.unwrap();
        assert_eq!(actual, expected);
    }

    /// A malicious peer can send a huge length prefix for a Vec. We must not
    /// preallocate based on the untrusted length, otherwise this aborts the
    /// process with an allocation failure (DoS). Found by fuzzing.
    #[tokio::test]
    async fn test_huge_vec_len_does_not_abort() {
        // len = u64::MAX, then EOF.
        let mock = Builder::new().read(&hex!("FFFF FFFF FFFF FFFF")).build();
        let mut reader = NixReader::new(mock);
        let result: Result<Vec<u64>, _> = reader.read_value().await;
        assert!(result.is_err());
    }
}
