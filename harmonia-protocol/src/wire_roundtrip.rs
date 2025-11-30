//! Property-based roundtrip tests for the worker-protocol wire serializers.
//!
//! Many types in this crate already derive `Arbitrary`; this module turns
//! those into actual `NixWriter -> bytes -> NixReader` roundtrip checks so
//! field-order or padding bugs in manual `NixSerialize`/`NixDeserialize` impls
//! are caught.

use std::io::Cursor;

use proptest::prelude::*;
use tokio::io::AsyncWriteExt as _;

use crate::de::{NixDeserialize, NixRead, NixReader};
use crate::ser::{NixSerialize, NixWrite, NixWriter};
use crate::version::{FeatureSet, supported_features};

async fn roundtrip<T>(features: FeatureSet, v: &T) -> T
where
    T: NixSerialize + NixDeserialize + Send,
{
    let mut buf = Vec::new();
    let mut w = NixWriter::builder()
        .set_features(features.clone())
        .build(&mut buf);
    w.write_value(v).await.expect("write");
    w.flush().await.expect("flush");
    drop(w);

    let mut r = NixReader::builder()
        .set_features(features)
        .build_buffered(Cursor::new(buf));
    let back: T = r.read_value().await.expect("read");
    assert!(
        r.read_value::<u64>().await.is_err(),
        "trailing bytes after read"
    );
    back
}

/// Declare a `NixWriter`/`NixReader` proptest roundtrip for `$ty`.
///
/// Runs with `supported_features()` so feature-gated serializers (e.g.
/// `Realisation`) are exercised. For lossy back-compat paths use a fixed-input
/// test instead.
macro_rules! wire_roundtrip {
    ($name:ident, $ty:ty) => {
        #[test]
        fn $name() {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            proptest!(|(v in any::<$ty>())| {
                let back = rt.block_on(roundtrip::<$ty>(supported_features(), &v));
                prop_assert_eq!(back, v);
            });
        }
    };
}

use crate::build_result::BuildResult;
use crate::daemon_wire::types2::QueryMissingResult;
use harmonia_store_core::derived_path::DerivedPath;
use harmonia_store_core::realisation::{DrvOutput, Realisation, UnkeyedRealisation};

wire_roundtrip!(roundtrip_drv_output, DrvOutput);
wire_roundtrip!(roundtrip_unkeyed_realisation, UnkeyedRealisation);
wire_roundtrip!(
    roundtrip_option_unkeyed_realisation,
    Option<UnkeyedRealisation>
);
wire_roundtrip!(roundtrip_realisation, Realisation);
wire_roundtrip!(roundtrip_build_result, BuildResult);
wire_roundtrip!(roundtrip_query_missing_result, QueryMissingResult);
wire_roundtrip!(roundtrip_derived_path, DerivedPath);
