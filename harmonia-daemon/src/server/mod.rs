// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This module is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Daemon server implementation.
//!
//! This module provides a server implementation for the Nix daemon protocol,
//! allowing harmonia-daemon to serve store operations over Unix sockets.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::ops::Deref;
use std::pin::{Pin, pin};

use futures::future::TryFutureExt;
use futures::{FutureExt, Stream, StreamExt as _};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, AsyncWriteExt, copy_buf};
use tokio::net::UnixListener;
use tracing::{Instrument, debug, error, info, instrument, trace};

use harmonia_protocol::ProtocolVersion;
use harmonia_protocol::daemon::{
    DaemonError, DaemonErrorKind, DaemonResult, DaemonResultExt, DaemonStore, HandshakeDaemonStore,
    ResultLog, TrustLevel,
    wire::{
        CLIENT_MAGIC, FramedReader, IgnoredOne, SERVER_MAGIC,
        logger::RawLogMessage,
        parse_add_multiple_to_store,
        types2::{BaseStorePath, CollectGarbageResponse, GCAction, Request, ValidPathInfo},
    },
};
use harmonia_protocol::de::{NixRead, NixReader};
use harmonia_protocol::ser::{NixWrite, NixWriter};
use harmonia_protocol::types::{AddToStoreItem, DaemonPath};
use harmonia_store_core::derivation::BasicDerivation;
use harmonia_store_core::derived_path::{DerivedPath, OutputName};
use harmonia_store_core::log::LogMessage;
use harmonia_store_core::realisation::{DrvOutput, Realisation};
use harmonia_store_core::signature::Signature;
use harmonia_store_core::store_path::{
    ContentAddressMethodAlgorithm, StorePath, StorePathHash, StorePathSet,
};
use harmonia_utils_io::{AsyncBufReadCompat, BytesReader};

const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::from_parts(1, 37);
const NIX_VERSION: &str = "2.24.0 (Harmonia)";

pub struct RecoverableError {
    pub can_recover: bool,
    pub source: DaemonError,
}

trait RecoverExt<T> {
    fn recover(self) -> Result<T, RecoverableError>;
}

impl<T, E> RecoverExt<T> for Result<T, E>
where
    E: Into<DaemonError>,
{
    fn recover(self) -> Result<T, RecoverableError> {
        self.map_err(|source| RecoverableError {
            can_recover: true,
            source: source.into(),
        })
    }
}

impl<T> From<T> for RecoverableError
where
    T: Into<DaemonError>,
{
    fn from(source: T) -> Self {
        RecoverableError {
            can_recover: false,
            source: source.into(),
        }
    }
}

/// Builder for daemon server connections.
pub struct Builder {
    store_trust: TrustLevel,
    store_dir: harmonia_store_core::store_path::StoreDir,
    min_version: ProtocolVersion,
    max_version: ProtocolVersion,
    nix_version: Option<String>,
}

impl Builder {
    pub fn new() -> Builder {
        Default::default()
    }

    pub fn set_store_dir(mut self, store_dir: harmonia_store_core::store_path::StoreDir) -> Self {
        self.store_dir = store_dir;
        self
    }

    pub fn set_min_version<V: Into<ProtocolVersion>>(&mut self, version: V) -> &mut Self {
        let version = version.into();
        assert!(
            version >= ProtocolVersion::min(),
            "min version must be at least {}",
            ProtocolVersion::min()
        );
        self.min_version = version;
        self
    }

    pub fn set_max_version<V: Into<ProtocolVersion>>(&mut self, version: V) -> &mut Self {
        let version = version.into();
        assert!(
            version <= ProtocolVersion::max(),
            "max version must not be after {}",
            ProtocolVersion::max()
        );
        self.max_version = version;
        self
    }

    pub async fn serve_connection<'s, R, W, S>(
        &'s self,
        reader: R,
        writer: W,
        store: S,
    ) -> DaemonResult<()>
    where
        R: AsyncRead + Debug + Send + Unpin + 's,
        W: AsyncWrite + Debug + Send + Unpin + 's,
        S: HandshakeDaemonStore + Send + 's,
    {
        let reader = NixReader::builder()
            .set_store_dir(&self.store_dir)
            .build_buffered(reader);
        let writer = NixWriter::builder()
            .set_store_dir(&self.store_dir)
            .build(writer);
        let mut conn = DaemonConnection {
            store_trust: self.store_trust,
            reader,
            writer,
        };
        let nix_version = self.nix_version.as_deref().unwrap_or(NIX_VERSION);
        conn.handshake(self.min_version, self.max_version, nix_version)
            .await?;
        trace!("Server handshake done!");
        let store_result = store.handshake();
        let store = conn
            .process_logs(store_result)
            .await
            .map_err(|e| e.source)?;
        conn.writer.flush().await?;
        trace!("Server handshake logs done!");
        conn.process_requests(store).await?;
        trace!("Server processed all requests!");
        Ok(())
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            store_trust: TrustLevel::NotTrusted,
            store_dir: Default::default(),
            min_version: ProtocolVersion::min(),
            max_version: ProtocolVersion::max(),
            nix_version: None,
        }
    }
}

async fn write_log<W>(writer: &mut NixWriter<W>, msg: LogMessage) -> Result<(), RecoverableError>
where
    W: AsyncWrite + Send + Unpin,
{
    match &msg {
        LogMessage::Message(raw_msg) => {
            let msg = String::from_utf8_lossy(&raw_msg.text);
            trace!("log_message: {}", msg);
        }
        LogMessage::StartActivity(activity) => {
            let text = String::from_utf8_lossy(&activity.text);
            trace!(id=activity.id, level=?activity.level, type=?activity.activity_type,
                ?text,
                parent=activity.parent,
                "start_activity: {:?} {:?}: {}", activity.activity_type, activity.fields, text);
        }
        LogMessage::StopActivity(activity) => {
            trace!(id = activity.id, "stop_activity: {}", activity.id);
        }
        LogMessage::Result(result) => {
            trace!(
                id = result.id,
                "log_result: {} {:?} {:?}", result.id, result.result_type, result.fields,
            );
        }
    }
    writer.write_value(&msg).await?;
    writer.flush().await?;
    Ok(())
}

async fn process_logs<'s, T, W>(
    writer: &'s mut NixWriter<W>,
    logs: impl ResultLog<Output = DaemonResult<T>> + 's,
) -> Result<T, RecoverableError>
where
    T: 's,
    W: AsyncWrite + Send + Unpin,
{
    let mut logs = pin!(logs);
    while let Some(msg) = logs.next().await {
        write_log(writer, msg).await?;
    }
    match logs.await {
        Err(source) => {
            error!("result_error: {:?}", source);
            Err(RecoverableError {
                can_recover: true,
                source,
            })
        }
        Ok(value) => Ok(value),
    }
}

/// Wrapper to box store method returns for size control.
struct BoxedStore<S>(S);

#[warn(clippy::missing_trait_methods)]
impl<S> DaemonStore for BoxedStore<S>
where
    S: DaemonStore,
{
    fn trust_level(&self) -> TrustLevel {
        self.0.trust_level()
    }

    fn set_options<'a>(
        &'a mut self,
        options: &'a harmonia_protocol::types::ClientOptions,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        Box::pin(self.0.set_options(options))
    }

    fn is_valid_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + 'a {
        let ret = Box::pin(self.0.is_valid_path(path));
        trace!("IsValidPath Size {}", size_of_val(&ret));
        ret
    }

    fn query_valid_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
        substitute: bool,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        let ret = Box::pin(self.0.query_valid_paths(paths, substitute));
        trace!("QueryValidPaths Size {}", size_of_val(&ret));
        ret
    }

    fn query_path_info<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<
        Output = DaemonResult<Option<harmonia_protocol::types::UnkeyedValidPathInfo>>,
    > + Send
    + 'a {
        let ret = Box::pin(self.0.query_path_info(path));
        trace!("QueryPathInfo Size {}", size_of_val(&ret));
        ret
    }

    fn nar_from_path<'s>(
        &'s mut self,
        path: &'s StorePath,
    ) -> impl ResultLog<Output = DaemonResult<impl AsyncBufRead + use<S>>> + Send + 's {
        let ret = Box::pin(self.0.nar_from_path(path));
        trace!("NarFromPath Size {}", size_of_val(&ret));
        ret
    }

    fn build_paths<'a>(
        &'a mut self,
        paths: &'a [DerivedPath],
        mode: harmonia_protocol::daemon_wire::types2::BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        let ret = Box::pin(self.0.build_paths(paths, mode));
        trace!("BuildPaths Size {}", size_of_val(&ret));
        ret
    }

    fn build_derivation<'a>(
        &'a mut self,
        drv_path: &'a StorePath,
        drv: &'a BasicDerivation,
        mode: harmonia_protocol::daemon_wire::types2::BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<harmonia_protocol::daemon_wire::types2::BuildResult>>
    + Send
    + 'a {
        let ret = Box::pin(self.0.build_derivation(drv_path, drv, mode));
        trace!("BuildDerivation Size {}", size_of_val(&ret));
        ret
    }

    fn query_missing<'a>(
        &'a mut self,
        paths: &'a [DerivedPath],
    ) -> impl ResultLog<
        Output = DaemonResult<harmonia_protocol::daemon_wire::types2::QueryMissingResult>,
    > + Send
    + 'a {
        let ret = Box::pin(self.0.query_missing(paths));
        trace!("QueryMissing Size {}", size_of_val(&ret));
        ret
    }

    fn add_to_store_nar<'s, 'r, 'i, R>(
        &'s mut self,
        info: &'i ValidPathInfo,
        source: R,
        repair: bool,
        dont_check_sigs: bool,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        R: AsyncBufRead + Send + Unpin + 'r,
        's: 'r,
        'i: 'r,
    {
        let ret = Box::pin(
            self.0
                .add_to_store_nar(info, source, repair, dont_check_sigs),
        );
        trace!("AddToStoreNar Size {}", size_of_val(ret.deref()));
        ret
    }

    fn add_multiple_to_store<'s, 'i, 'r, ST, STR>(
        &'s mut self,
        repair: bool,
        dont_check_sigs: bool,
        stream: ST,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        ST: Stream<Item = Result<AddToStoreItem<STR>, DaemonError>> + Send + 'i,
        STR: AsyncBufRead + Send + Unpin + 'i,
        's: 'r,
        'i: 'r,
    {
        let ret = self
            .0
            .add_multiple_to_store(repair, dont_check_sigs, stream);
        trace!("AddMultipleToStore Size {}", size_of_val(ret.deref()));
        ret
    }

    fn build_paths_with_results<'a>(
        &'a mut self,
        drvs: &'a [DerivedPath],
        mode: harmonia_protocol::daemon_wire::types2::BuildMode,
    ) -> impl ResultLog<
        Output = DaemonResult<Vec<harmonia_protocol::daemon_wire::types2::KeyedBuildResult>>,
    > + Send
    + 'a {
        let ret = Box::pin(self.0.build_paths_with_results(drvs, mode));
        trace!("BuildPathsWithResults Size {}", size_of_val(ret.deref()));
        ret
    }

    fn query_all_valid_paths(
        &mut self,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + '_ {
        let ret = Box::pin(self.0.query_all_valid_paths());
        trace!("QueryAllValidPaths Size {}", size_of_val(&ret));
        ret
    }

    fn query_referrers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<BTreeSet<StorePath>>> + Send + 'a {
        let ret = Box::pin(self.0.query_referrers(path));
        trace!("QueryReferrers Size {}", size_of_val(ret.deref()));
        ret
    }

    fn ensure_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        let ret = Box::pin(self.0.ensure_path(path));
        trace!("EnsurePath Size {}", size_of_val(ret.deref()));
        ret
    }

    fn add_temp_root<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        let ret = Box::pin(self.0.add_temp_root(path));
        trace!("AddTempRoot Size {}", size_of_val(ret.deref()));
        ret
    }

    fn add_indirect_root<'a>(
        &'a mut self,
        path: &'a DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        let ret = Box::pin(self.0.add_indirect_root(path));
        trace!("AddIndirectRoot Size {}", size_of_val(ret.deref()));
        ret
    }

    fn find_roots(
        &mut self,
    ) -> impl ResultLog<Output = DaemonResult<BTreeMap<DaemonPath, StorePath>>> + Send + '_ {
        let ret = Box::pin(self.0.find_roots());
        trace!("FindRoots Size {}", size_of_val(ret.deref()));
        ret
    }

    fn collect_garbage<'a>(
        &'a mut self,
        action: GCAction,
        paths_to_delete: &'a StorePathSet,
        ignore_liveness: bool,
        max_freed: u64,
    ) -> impl ResultLog<Output = DaemonResult<CollectGarbageResponse>> + Send + 'a {
        let ret =
            Box::pin(
                self.0
                    .collect_garbage(action, paths_to_delete, ignore_liveness, max_freed),
            );
        trace!("CollectGarbage Size {}", size_of_val(ret.deref()));
        ret
    }

    fn query_path_from_hash_part<'a>(
        &'a mut self,
        hash: &'a StorePathHash,
    ) -> impl ResultLog<Output = DaemonResult<Option<StorePath>>> + Send + 'a {
        let ret = Box::pin(self.0.query_path_from_hash_part(hash));
        trace!("QueryPathFromHashPart Size {}", size_of_val(ret.deref()));
        ret
    }

    fn query_substitutable_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        let ret = Box::pin(self.0.query_substitutable_paths(paths));
        trace!("QuerySubstitutablePaths Size {}", size_of_val(ret.deref()));
        ret
    }

    fn query_valid_derivers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        let ret = Box::pin(self.0.query_valid_derivers(path));
        trace!("QueryValidDerivers Size {}", size_of_val(ret.deref()));
        ret
    }

    fn optimise_store(&mut self) -> impl ResultLog<Output = DaemonResult<()>> + Send + '_ {
        let ret = Box::pin(self.0.optimise_store());
        trace!("OptimiseStore Size {}", size_of_val(ret.deref()));
        ret
    }

    fn verify_store(
        &mut self,
        check_contents: bool,
        repair: bool,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + '_ {
        let ret = Box::pin(self.0.verify_store(check_contents, repair));
        trace!("VerifyStore Size {}", size_of_val(ret.deref()));
        ret
    }

    fn add_signatures<'a>(
        &'a mut self,
        path: &'a StorePath,
        signatures: &'a [Signature],
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        let ret = Box::pin(self.0.add_signatures(path, signatures));
        trace!("AddSignatures Size {}", size_of_val(ret.deref()));
        ret
    }

    fn query_derivation_output_map<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<BTreeMap<OutputName, Option<StorePath>>>> + Send + 'a
    {
        let ret = Box::pin(self.0.query_derivation_output_map(path));
        trace!("QueryDerivationOutputMap Size {}", size_of_val(ret.deref()));
        ret
    }

    fn register_drv_output<'a>(
        &'a mut self,
        realisation: &'a Realisation,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        let ret = Box::pin(self.0.register_drv_output(realisation));
        trace!("RegisterDrvOutput Size {}", size_of_val(ret.deref()));
        ret
    }

    fn query_realisation<'a>(
        &'a mut self,
        output_id: &'a DrvOutput,
    ) -> impl ResultLog<Output = DaemonResult<BTreeSet<Realisation>>> + Send + 'a {
        let ret = Box::pin(self.0.query_realisation(output_id));
        trace!("QueryRealisation Size {}", size_of_val(ret.deref()));
        ret
    }

    fn add_build_log<'s, 'r, 'p, R>(
        &'s mut self,
        path: &'p StorePath,
        source: R,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        R: AsyncBufRead + Send + Unpin + 'r,
        's: 'r,
        'p: 'r,
    {
        let ret = self.0.add_build_log(path, source);
        trace!("AddBuildLog Size {}", size_of_val(ret.deref()));
        ret
    }

    fn add_perm_root<'a>(
        &'a mut self,
        path: &'a StorePath,
        gc_root: &'a DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<DaemonPath>> + Send + 'a {
        let ret = Box::pin(self.0.add_perm_root(path, gc_root));
        trace!("AddPermRoot Size {}", size_of_val(ret.deref()));
        ret
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
        let ret = self.0.add_ca_to_store(name, cam, refs, repair, source);
        trace!("AddToStore Size {}", size_of_val(ret.deref()));
        ret
    }

    fn shutdown(&mut self) -> impl std::future::Future<Output = DaemonResult<()>> + Send + '_ {
        let ret = Box::pin(self.0.shutdown());
        trace!("Shutdown Size {}", size_of_val(&ret));
        ret
    }
}

pub struct DaemonConnection<R, W> {
    store_trust: TrustLevel,
    reader: NixReader<BytesReader<R>>,
    writer: NixWriter<W>,
}

impl<R, W> DaemonConnection<R, W>
where
    R: AsyncRead + Send + Unpin + Debug,
    W: AsyncWrite + Send + Unpin + Debug,
{
    #[instrument(skip(self))]
    pub async fn handshake<'s>(
        &'s mut self,
        min_version: ProtocolVersion,
        max_version: ProtocolVersion,
        nix_version: &'s str,
    ) -> Result<ProtocolVersion, DaemonError> {
        assert!(
            min_version.major() == 1 && min_version.minor() >= 37,
            "Only Nix 2.24 and later is supported (protocol 1.37+)"
        );
        assert!(
            max_version <= PROTOCOL_VERSION,
            "Only protocols up to {} is supported",
            PROTOCOL_VERSION
        );

        let client_magic = self.reader.read_number().await.with_field("clientMagic")?;
        if client_magic != CLIENT_MAGIC {
            return Err(DaemonErrorKind::WrongMagic(client_magic)).with_field("clientMagic");
        }

        self.writer
            .write_number(SERVER_MAGIC)
            .await
            .with_field("serverMagic")?;
        self.writer
            .write_value(&max_version)
            .await
            .with_field("protocolVersion")?;
        self.writer.flush().await?;

        let client_version: ProtocolVersion =
            self.reader.read_value().await.with_field("clientVersion")?;
        let version = client_version.min(max_version);
        if version < min_version {
            return Err(DaemonErrorKind::UnsupportedVersion(version)).with_field("clientVersion");
        }
        self.reader.set_version(version);
        self.writer.set_version(version);
        debug!(
            ?version,
            ?client_version,
            "Server Version is {}, Client version is {}",
            version,
            client_version
        );

        // Obsolete CPU Affinity (protocol >= 14)
        if self.reader.read_value().await.with_field("sendCpu")? {
            let _cpu_affinity = self.reader.read_number().await.with_field("cpuAffinity")?;
        }

        // Obsolete reserved space (protocol >= 11)
        let _reserve_space: bool = self.reader.read_value().await.with_field("reserveSpace")?;

        // Send nix version (protocol >= 33)
        self.writer
            .write_value(nix_version)
            .await
            .with_field("nixVersion")?;

        // Send trust level (protocol >= 35)
        self.writer
            .write_value(&self.store_trust)
            .await
            .with_field("trusted")?;

        self.writer.flush().await?;
        Ok(version)
    }

    #[instrument(level = "trace", skip_all)]
    pub async fn process_logs<'s, T: Send + 's>(
        &'s mut self,
        logs: impl ResultLog<Output = DaemonResult<T>> + Send + 's,
    ) -> Result<T, RecoverableError> {
        let value = process_logs(&mut self.writer, logs).await?;
        self.writer.write_value(&RawLogMessage::Last).await?;
        Ok(value)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn process_requests<'s, S>(&'s mut self, store: S) -> Result<(), DaemonError>
    where
        S: DaemonStore + 's,
    {
        let mut store = BoxedStore(store);
        loop {
            trace!("server buffer is {:?}", self.reader.get_ref().filled());
            let fut = self.reader.try_read_value::<Request>().boxed();
            trace!("Request Size {}", size_of_val(fut.deref()));
            let res = fut.await?;
            let Some(request) = res else {
                break;
            };
            let op = request.operation();
            let span = request.span();
            async {
                debug!("Server got operation {}", op);
                let req = self.process_request(&mut store, request);
                if let Err(mut err) = req.await {
                    error!(error = ?err.source, recover=err.can_recover, "Error processing request");
                    err.source = err.source.fill_operation(op);
                    if err.can_recover {
                        self.writer
                            .write_value(&RawLogMessage::Error(err.source.into()))
                            .await?;
                    } else {
                        return Err(err.source);
                    }
                }
                trace!("Server flush");
                self.writer.flush().await?;
                Ok(())
            }
            .instrument(span).await?;
        }
        debug!("Server handled all requests");
        store.shutdown().await
    }

    fn add_ca_to_store<'s, 'p, 'r, NW, S>(
        store: &'s mut S,
        name: &'p str,
        cam: ContentAddressMethodAlgorithm,
        refs: &'p StorePathSet,
        repair: bool,
        source: NW,
    ) -> impl ResultLog<Output = DaemonResult<ValidPathInfo>> + Send + 'r
    where
        S: DaemonStore + 's,
        NW: AsyncBufRead + Unpin + Send + 'r,
        's: 'r,
        'p: 'r,
    {
        store.add_ca_to_store(name, cam, refs, repair, source)
    }

    fn add_to_store_nar<'s, 'p, 'r, NW, S>(
        store: &'s mut S,
        info: &'p ValidPathInfo,
        source: NW,
        repair: bool,
        dont_check_sigs: bool,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'r
    where
        S: DaemonStore + 's,
        NW: AsyncBufRead + Unpin + Send + 'r,
        's: 'r,
        'p: 'r,
    {
        store.add_to_store_nar(info, source, repair, dont_check_sigs)
    }

    fn add_multiple_to_store<'s, 'r, S, ST, STR>(
        store: &'s mut S,
        repair: bool,
        dont_check_sigs: bool,
        stream: ST,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'r
    where
        S: DaemonStore + 's,
        ST: Stream<Item = Result<AddToStoreItem<STR>, DaemonError>> + Send + 'r,
        STR: AsyncBufRead + Unpin + Send + 'r,
        's: 'r,
    {
        store.add_multiple_to_store(repair, dont_check_sigs, stream)
    }

    fn store_nar_from_path<'s, S>(
        store: &'s mut S,
        path: &'s StorePath,
    ) -> impl ResultLog<Output = DaemonResult<impl AsyncBufRead + 's>> + Send + 's
    where
        S: DaemonStore + 's,
    {
        store.nar_from_path(path)
    }

    async fn nar_from_path<'s, 't, S>(
        &'s mut self,
        store: &'t mut S,
        path: StorePath,
    ) -> Result<(), RecoverableError>
    where
        S: DaemonStore + 't,
    {
        let logs = Self::store_nar_from_path(store, &path);

        let mut logs = pin!(logs);
        while let Some(msg) = logs.next().await {
            write_log(&mut self.writer, msg).await?;
        }

        let mut reader = pin!(logs.await?);
        self.writer.write_value(&RawLogMessage::Last).await?;
        let ret = copy_buf(&mut reader, &mut self.writer)
            .map_err(DaemonError::from)
            .await;
        match ret {
            Err(err) => {
                error!("NAR Copy failed {:?}", err);
                Err(err.into())
            }
            Ok(bytes) => {
                info!(bytes, "Copied {} bytes", bytes);
                Ok(())
            }
        }
    }

    fn add_build_log<'s, 'p, 'r, NW, S>(
        store: &'s mut S,
        path: &'p StorePath,
        source: NW,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'r
    where
        S: DaemonStore + 's,
        NW: AsyncBufRead + Unpin + Send + 'r,
        's: 'r,
        'p: 'r,
    {
        store.add_build_log(path, source)
    }

    pub async fn process_request<'s, S>(
        &'s mut self,
        mut store: S,
        request: Request,
    ) -> Result<(), RecoverableError>
    where
        S: DaemonStore + 's,
    {
        use Request::*;
        match request {
            SetOptions(options) => {
                let logs = store.set_options(&options);
                self.process_logs(logs).await?;
            }
            IsValidPath(path) => {
                let logs = store.is_valid_path(&path);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            QueryValidPaths(req) => {
                let logs = store.query_valid_paths(&req.paths, req.substitute);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            QueryPathInfo(path) => {
                let logs = store.query_path_info(&path);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            NarFromPath(path) => {
                self.nar_from_path(&mut store, path).await?;
            }
            QueryReferrers(path) => {
                let logs = store.query_referrers(&path);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            AddToStore(req) => {
                let buf_reader = AsyncBufReadCompat::new(&mut self.reader);
                let mut framed = FramedReader::new(buf_reader);
                let logs = Self::add_ca_to_store(
                    &mut store,
                    &req.name,
                    req.cam,
                    &req.refs,
                    req.repair,
                    &mut framed,
                );
                let res = process_logs(&mut self.writer, logs).await;
                let err = framed.drain_all().await;
                let value = res?;
                err?;
                self.writer.write_value(&RawLogMessage::Last).await?;
                self.writer.write_value(&value).await?;
            }
            BuildPaths(req) => {
                let logs = store.build_paths(&req.paths, req.mode);
                self.process_logs(logs).await?;
                self.writer.write_value(&IgnoredOne).await?;
            }
            EnsurePath(path) => {
                let logs = store.ensure_path(&path);
                self.process_logs(logs).await?;
                self.writer.write_value(&IgnoredOne).await?;
            }
            AddTempRoot(path) => {
                let logs = store.add_temp_root(&path);
                self.process_logs(logs).await?;
                self.writer.write_value(&IgnoredOne).await?;
            }
            AddIndirectRoot(path) => {
                let logs = store.add_indirect_root(&path);
                self.process_logs(logs).await?;
                self.writer.write_value(&IgnoredOne).await?;
            }
            FindRoots => {
                let logs = store.find_roots();
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            CollectGarbage(req) => {
                let logs = store.collect_garbage(
                    req.action,
                    &req.paths_to_delete,
                    req.ignore_liveness,
                    req.max_freed,
                );
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            QueryAllValidPaths => {
                let logs = store.query_all_valid_paths();
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            QueryPathFromHashPart(hash) => {
                let logs = store.query_path_from_hash_part(&hash);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            QuerySubstitutablePaths(paths) => {
                let logs = store.query_substitutable_paths(&paths);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            QueryValidDerivers(path) => {
                let logs = store.query_valid_derivers(&path);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            OptimiseStore => {
                let logs = store.optimise_store();
                self.process_logs(logs).await?;
                self.writer.write_value(&IgnoredOne).await?;
            }
            VerifyStore(req) => {
                let logs = store.verify_store(req.check_contents, req.repair);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            BuildDerivation(req) => {
                let (drv_path, drv) = &req.drv;
                let logs = store.build_derivation(drv_path, drv, req.mode);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            AddSignatures(req) => {
                let logs = store.add_signatures(&req.path, &req.signatures);
                self.process_logs(logs).await?;
                self.writer.write_value(&IgnoredOne).await?;
            }
            AddToStoreNar(req) => {
                trace!("DaemonConnection: Add to store");
                let buf_reader = AsyncBufReadCompat::new(&mut self.reader);
                let mut framed = FramedReader::new(buf_reader);
                trace!("DaemonConnection: Add to store: Framed");
                let logs = Self::add_to_store_nar(
                    &mut store,
                    &req.path_info,
                    &mut framed,
                    req.repair,
                    req.dont_check_sigs,
                );
                trace!("DaemonConnection: Add to store: Logs");
                let res: Result<(), RecoverableError> = async {
                    let mut logs = pin!(logs);
                    trace!("DaemonConnection: Add to store: get log");
                    while let Some(msg) = logs.next().await {
                        trace!("DaemonConnection: Add to store: got log");
                        write_log(&mut self.writer, msg).await?;
                    }
                    trace!("DaemonConnection: Add to store: get result");
                    logs.await.recover()?;
                    Ok(())
                }
                .await;
                trace!("DaemonConnection: Add to store: drain reader");
                let err = framed.drain_all().await;
                trace!("DaemonConnection: Add to store: done");
                res?;
                err?;
                self.writer.write_value(&RawLogMessage::Last).await?;
            }
            QueryMissing(paths) => {
                let logs = store.query_missing(&paths);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            QueryDerivationOutputMap(path) => {
                let logs = store.query_derivation_output_map(&path);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            RegisterDrvOutput(realisation) => {
                let logs = store.register_drv_output(&realisation);
                self.process_logs(logs).await?;
            }
            QueryRealisation(output_id) => {
                let logs = store.query_realisation(&output_id);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            AddMultipleToStore(req) => {
                let builder = NixReader::builder().set_version(self.reader.version());
                let buf_reader = AsyncBufReadCompat::new(&mut self.reader);
                let mut framed = FramedReader::new(buf_reader);
                let source = builder.build_buffered(&mut framed);
                let stream = parse_add_multiple_to_store(source).await?;
                trace!("DaemonConnection: Add multiple to store: call store");
                let logs = Self::add_multiple_to_store(
                    &mut store,
                    req.repair,
                    req.dont_check_sigs,
                    stream,
                );
                trace!("DaemonConnection: Add multiple to store: Logs");
                let res: Result<(), RecoverableError> = async {
                    let mut logs = pin!(logs);
                    trace!("DaemonConnection: Add to store: get log");
                    while let Some(msg) = logs.next().await {
                        trace!("DaemonConnection: Add multiple to store: got log {:?}", msg);
                        write_log(&mut self.writer, msg).await?;
                    }
                    trace!("DaemonConnection: Add multiple to store: get result");
                    logs.await.recover()?;
                    trace!("DaemonConnection: Add multiple to store: write result");
                    self.writer.write_value(&RawLogMessage::Last).await?;
                    Ok(())
                }
                .await;
                trace!("DaemonConnection: Add to store: drain reader");
                let err = framed.drain_all().await;
                trace!("DaemonConnection: Add multiple to store: done");
                res?;
                err?;
            }
            AddBuildLog(BaseStorePath(path)) => {
                let buf_reader = AsyncBufReadCompat::new(&mut self.reader);
                let mut framed = FramedReader::new(buf_reader);
                let logs = Self::add_build_log(&mut store, &path, &mut framed);
                let res = process_logs(&mut self.writer, logs).await;
                let err = framed.drain_all().await;
                res?;
                err?;
                self.writer.write_value(&RawLogMessage::Last).await?;
                self.writer.write_value(&IgnoredOne).await?;
            }
            BuildPathsWithResults(req) => {
                let logs = store.build_paths_with_results(&req.paths, req.mode);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
            AddPermRoot(req) => {
                let logs = store.add_perm_root(&req.store_path, &req.gc_root);
                let value = self.process_logs(logs).await?;
                self.writer.write_value(&value).await?;
            }
        }
        Ok(())
    }
}

/// Simple daemon server that listens on a Unix socket.
pub struct DaemonServer<H> {
    handler: H,
    socket_path: std::path::PathBuf,
    store_dir: harmonia_store_core::store_path::StoreDir,
}

impl<H> DaemonServer<H>
where
    H: HandshakeDaemonStore + Clone + Send + Sync + 'static,
{
    pub fn new(
        handler: H,
        socket_path: std::path::PathBuf,
        store_dir: harmonia_store_core::store_path::StoreDir,
    ) -> Self {
        Self {
            handler,
            socket_path,
            store_dir,
        }
    }

    pub async fn serve(&self) -> Result<(), std::io::Error> {
        // Remove existing socket file if present
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        // Create parent directory if needed
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;

        // Make socket world-accessible so other users can connect
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o666);
            std::fs::set_permissions(&self.socket_path, perms)?;
        }

        info!("Listening on {:?}", self.socket_path);

        loop {
            let (stream, _addr) = listener.accept().await?;
            let handler = self.handler.clone();
            let store_dir = self.store_dir.clone();

            tokio::spawn(async move {
                let (reader, writer) = stream.into_split();
                let builder = Builder::new().set_store_dir(store_dir);
                if let Err(e) = builder.serve_connection(reader, writer, handler).await {
                    error!("Connection error: {:?}", e);
                }
            });
        }
    }
}
