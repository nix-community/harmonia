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

use crate::daemon_wire::types2::QueryMissingResult;
use harmonia_store_build_result::BuildResult;
use harmonia_store_derivation::derived_path::DerivedPath;
use harmonia_store_derivation::realisation::{DrvOutput, Realisation, UnkeyedRealisation};

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

#[test]
fn single_derived_path_rejects_invalid_tag() {
    use harmonia_store_derivation::derived_path::SingleDerivedPath;

    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();

    rt.block_on(async {
        let mut buf = Vec::new();
        let mut w = NixWriter::builder().build(&mut buf);
        w.write_value(&2u64).await.expect("write");
        w.flush().await.expect("flush");

        let mut r = NixReader::builder().build_buffered(Cursor::new(buf));
        r.read_value::<SingleDerivedPath>().await.unwrap_err();
    });
}

/// Pins opcode 1000 and the tagged `SingleDerivedPath` encoding.
#[test]
fn roundtrip_submit_output_request() {
    use std::sync::Arc;

    use crate::daemon_wire::types2::{Request, SubmitOutputRequest};
    use harmonia_store_derivation::derived_path::SingleDerivedPath;
    use harmonia_store_path::StorePath;

    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();

    let path = SingleDerivedPath::Built {
        drv_path: Arc::new(SingleDerivedPath::Opaque(
            StorePath::from_bytes(b"00000000000000000000000000000000-x.drv").unwrap(),
        )),
        output: "lib".parse().unwrap(),
    };
    let req = Request::SubmitOutput(SubmitOutputRequest {
        path,
        output: "out".parse().unwrap(),
    });

    let buf = rt.block_on(async {
        let mut buf = Vec::new();
        let mut w = NixWriter::builder().build(&mut buf);
        w.write_value(&req).await.expect("write");
        w.flush().await.expect("flush");
        buf
    });
    let word = |i: usize| u64::from_le_bytes(buf[i * 8..(i + 1) * 8].try_into().unwrap());
    assert_eq!(word(0), 1000);
    assert_eq!(word(1), 1); // Built tag
    assert_eq!(word(2), 0); // inner Opaque tag

    let back: Request = rt.block_on(async {
        let mut r = NixReader::builder().build_buffered(Cursor::new(buf));
        r.read_value().await.expect("read")
    });
    assert_eq!(back, req);
}

#[test]
fn roundtrip_add_to_store_scanning_request() {
    use crate::daemon_wire::types2::{AddToStoreScanningRequest, Request};
    use harmonia_store_content_address::ContentAddressMethodAlgorithm;
    use harmonia_utils_hash::Algorithm;

    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();

    let req = Request::AddToStoreScanning(AddToStoreScanningRequest {
        name: "example".into(),
        cam: ContentAddressMethodAlgorithm::NixArchive(Algorithm::SHA256),
    });

    let buf = rt.block_on(async {
        let mut buf = Vec::new();
        let mut w = NixWriter::builder().build(&mut buf);
        w.write_value(&req).await.expect("write");
        w.flush().await.expect("flush");
        buf
    });
    assert_eq!(u64::from_le_bytes(buf[..8].try_into().unwrap()), 1001);
    assert!(
        buf.windows(14).any(|w| w == b"fixed:r:sha256"),
        "cam must be transmitted in renderWithAlgo form"
    );

    let back: Request = rt.block_on(async {
        let mut r = NixReader::builder().build_buffered(Cursor::new(buf));
        r.read_value().await.expect("read")
    });
    assert_eq!(back, req);
}
