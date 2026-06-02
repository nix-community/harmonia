//! Higher-level store operations built on top of the [`DaemonStore`] trait.

use harmonia_store_content_address::ContentAddressMethodAlgorithm;
use harmonia_store_derivation::derivation::BasicDerivation;
use harmonia_store_path::StoreDir;

use crate::types::{DaemonResult, DaemonStore};
use crate::valid_path_info::ValidPathInfo;

/// Serialize a derivation to ATerm and write it to the store as a
/// text-hashed content-addressed flat file.
///
/// The derivation's inputs become the references of the written path. Returns the
/// [`ValidPathInfo`] of the new `.drv`, whose `path` is the derivation's
/// store path.
pub async fn write_derivation<S>(
    store: &mut S,
    store_dir: &StoreDir,
    drv: &BasicDerivation,
    repair: bool,
) -> DaemonResult<ValidPathInfo>
where
    S: DaemonStore + ?Sized,
{
    let aterm = harmonia_store_aterm::print_derivation_aterm(store_dir, drv);
    let name = format!("{}.drv", drv.name);
    let source = std::io::Cursor::new(aterm);
    store
        .add_ca_to_store(
            &name,
            ContentAddressMethodAlgorithm::Text,
            &drv.inputs,
            repair,
            source,
        )
        .await
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use harmonia_store_derivation::derivation::{DerivationOutputs, DerivationT};
    use harmonia_store_path::{StorePath, StorePathSet};
    use tokio::io::AsyncBufRead;

    use super::*;
    use crate::daemon_wire::logger::{FutureResultExt as _, ResultLog, ResultLogExt as _};
    use crate::types::{DaemonError, TrustLevel};

    /// What [`add_ca_to_store`](`DaemonStore::add_ca_to_store`) was called with.
    struct Captured {
        name: String,
        cam: ContentAddressMethodAlgorithm,
        refs: StorePathSet,
        repair: bool,
        source: Vec<u8>,
    }

    /// A [`DaemonStore`] that records its [`add_ca_to_store`](`DaemonStore::add_ca_to_store`)
    /// arguments and then returns an error, so the test can assert on the call without
    /// having to fabricate a [`ValidPathInfo`].
    struct MockStore {
        captured: Arc<Mutex<Option<Captured>>>,
    }

    impl DaemonStore for MockStore {
        fn trust_level(&self) -> Option<TrustLevel> {
            None
        }

        fn add_ca_to_store<'a, 'r, R>(
            &'a mut self,
            name: &'a str,
            cam: ContentAddressMethodAlgorithm,
            refs: &'a StorePathSet,
            repair: bool,
            source: R,
        ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<ValidPathInfo>> + Send + 'r>>
        where
            R: AsyncBufRead + Send + Unpin + 'r,
            'a: 'r,
        {
            let captured = self.captured.clone();
            let name = name.to_owned();
            let refs = refs.clone();
            async move {
                use tokio::io::AsyncReadExt as _;
                let mut source = source;
                let mut buf = Vec::new();
                source
                    .read_to_end(&mut buf)
                    .await
                    .expect("read captured source");
                *captured.lock().unwrap() = Some(Captured {
                    name,
                    cam,
                    refs,
                    repair,
                    source: buf,
                });
                Err(DaemonError::custom("mock store: captured call"))
            }
            .empty_logs()
            .boxed_result()
        }

        async fn shutdown(&mut self) -> DaemonResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn forwards_aterm_name_and_refs_to_add_ca_to_store() {
        let store_dir = StoreDir::default();
        let input = "m1r53pnnm6hnjwyjmxska24y8amvlpjp-hello-2.12.1"
            .parse::<StorePath>()
            .unwrap();
        let mut inputs = StorePathSet::new();
        inputs.insert(input);

        let drv = DerivationT {
            name: "myprog".parse().unwrap(),
            outputs: DerivationOutputs::new(),
            inputs: inputs.clone(),
            platform: Bytes::from("x86_64-linux"),
            builder: Bytes::from("/bin/sh"),
            args: vec![],
            env: std::collections::BTreeMap::new(),
            structured_attrs: None,
        };

        let captured = Arc::new(Mutex::new(None));
        let mut store = MockStore {
            captured: captured.clone(),
        };

        // we only care about what `write_derivation` forwarded. repair is set to true
        // so the assertion proves the flag is forwarded, not hardcoded.
        let _ = write_derivation(&mut store, &store_dir, &drv, true).await;

        let call = captured
            .lock()
            .unwrap()
            .take()
            .expect("add_ca_to_store was called");
        assert_eq!(call.name, "myprog.drv");
        assert_eq!(call.cam, ContentAddressMethodAlgorithm::Text);
        assert_eq!(call.refs, inputs);
        assert!(call.repair);
        assert_eq!(
            call.source,
            harmonia_store_aterm::print_derivation_aterm(&store_dir, &drv),
        );
    }
}
