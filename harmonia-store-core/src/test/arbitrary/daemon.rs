use std::fmt::Debug;

#[cfg(feature = "daemon")]
use harmonia_protocol::daemon::wire::types2::ValidPathInfo;
#[cfg(feature = "daemon")]
use bytes::Bytes;
use proptest::prelude::*;
#[cfg(feature = "daemon")]
use proptest::sample::SizeRange;

#[cfg(feature = "daemon")]
use crate::archive::test_data;
use harmonia_protocol::daemon::ProtocolVersion;
#[cfg(feature = "daemon")]
use crate::test::arbitrary::archive::{arb_nar_contents, arb_nar_events};

pub fn version_cut_off<B, A, V>(
    version: ProtocolVersion,
    cut_off: u8,
    before: B,
    after: A,
) -> BoxedStrategy<V>
where
    B: Strategy<Value = V> + 'static,
    A: Strategy<Value = V> + 'static,
{
    if version.minor() < cut_off {
        before.boxed()
    } else {
        after.boxed()
    }
}

pub fn field_after<A, V>(version: ProtocolVersion, cut_off: u8, after: A) -> BoxedStrategy<V>
where
    A: Strategy<Value = V> + 'static,
    V: Default + Clone + Debug + 'static,
{
    version_cut_off(version, cut_off, Just(V::default()), after)
}

#[cfg(feature = "daemon")]
pub fn arb_nar_item() -> impl Strategy<Value = (ValidPathInfo, test_data::TestNarEvents)> {
    (any::<ValidPathInfo>(), arb_nar_events(20, 20, 5))
}

#[cfg(feature = "daemon")]
pub fn arb_nar_contents_item() -> impl Strategy<Value = (ValidPathInfo, Bytes)> {
    (any::<ValidPathInfo>(), arb_nar_contents(20, 20, 5))
}

#[cfg(feature = "daemon")]
pub fn arb_nar_items() -> impl Strategy<Value = Vec<(ValidPathInfo, test_data::TestNarEvents)>> {
    proptest::collection::vec(arb_nar_item(), SizeRange::default())
}

#[cfg(feature = "daemon")]
pub fn arb_nar_contents_items() -> impl Strategy<Value = Vec<(ValidPathInfo, Bytes)>> {
    proptest::collection::vec(arb_nar_contents_item(), SizeRange::default())
}
