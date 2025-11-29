// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::AtomicU64;

use async_stream::stream;
use futures::Stream;
use futures::future::Either;
use futures::io::Cursor;
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, AsyncWriteExt as _, copy_buf};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::oneshot;
use tracing::{debug, info, trace};

// From harmonia-protocol
use harmonia_protocol::ProtocolVersion;
use harmonia_protocol::daemon_wire::logger::{
    FutureResultExt, ProcessStderr, RawLogMessage, ResultLog, ResultLogExt,
};
use harmonia_protocol::daemon_wire::types::Operation;
use harmonia_protocol::daemon_wire::types2::{
    BuildMode, CollectGarbageResponse, GCAction, KeyedBuildResult, QueryMissingResult,
    ValidPathInfo,
};
use harmonia_protocol::daemon_wire::{
    CLIENT_MAGIC, FramedWriter, IgnoredOne, SERVER_MAGIC, write_add_multiple_to_store_stream,
};
use harmonia_protocol::de::{NixDeserialize, NixRead as _, NixReader, NixReaderBuilder};
use harmonia_protocol::ser::{NixWrite, NixWriter, NixWriterBuilder};
use harmonia_protocol::types::{
    AddToStoreItem, ClientOptions, DaemonError, DaemonErrorKind, DaemonPath, DaemonResult,
    DaemonResultExt as _, DaemonStore, HandshakeDaemonStore, TrustLevel, UnkeyedValidPathInfo,
};

// From harmonia-store-core
use harmonia_store_core::derivation::BasicDerivation;
use harmonia_store_core::derived_path::{DerivedPath, OutputName};
use harmonia_store_core::log::{LogMessage, Message, Verbosity};
use harmonia_store_core::realisation::{DrvOutput, Realisation};
use harmonia_store_core::signature::Signature;
use harmonia_store_core::store_path::{
    ContentAddressMethodAlgorithm, StoreDir, StorePath, StorePathHash, StorePathSet,
};
use harmonia_utils_io::{AsyncBufReadCompat, BytesReader, Lending};

// From harmonia-nar
use harmonia_nar::archive::{NarBytesReader, NarReader};

pub struct DaemonClientBuilder {
    store_dir: StoreDir,
    host: Option<String>,
    min_version: ProtocolVersion,
    max_version: ProtocolVersion,
    reader_builder: NixReaderBuilder,
    writer_builder: NixWriterBuilder,
}

impl Default for DaemonClientBuilder {
    fn default() -> Self {
        Self {
            store_dir: Default::default(),
            host: Default::default(),
            min_version: ProtocolVersion::min(),
            max_version: ProtocolVersion::max(),
            reader_builder: Default::default(),
            writer_builder: Default::default(),
        }
    }
}

impl DaemonClientBuilder {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn set_store_dir(mut self, store_dir: &StoreDir) -> Self {
        self.store_dir = store_dir.clone();
        self
    }

    pub fn set_host(mut self, host: String) -> Self {
        self.host = Some(host);
        self
    }

    pub fn clear_host(mut self) -> Self {
        self.host = None;
        self
    }

    pub fn set_min_version<P: Into<ProtocolVersion>>(mut self, min_version: P) -> Self {
        let min_version = min_version.into();
        assert!(
            min_version >= ProtocolVersion::min(),
            "Only protocols later than {} are supported",
            ProtocolVersion::min()
        );
        self.min_version = min_version;
        self
    }

    pub fn set_max_version<P: Into<ProtocolVersion>>(mut self, max_version: P) -> Self {
        let max_version = max_version.into();
        assert!(
            max_version <= ProtocolVersion::max(),
            "Only protocols up to {} are supported",
            ProtocolVersion::max()
        );
        self.max_version = max_version;
        self
    }

    pub fn build<R, W>(self, reader: R, writer: W) -> DaemonHandshakeClient<R, W>
    where
        R: AsyncRead + Unpin,
    {
        let reader = BytesReader::builder().build(reader);
        let reader = self
            .reader_builder
            .set_store_dir(&self.store_dir)
            .build(Lending::new(reader));
        let writer = self
            .writer_builder
            .set_store_dir(&self.store_dir)
            .build(writer);
        let host = self.host.unwrap_or_else(|| "unknown".to_string());
        let min_version = self.min_version;
        let max_version = self.max_version;
        DaemonHandshakeClient {
            host,
            reader,
            writer,
            min_version,
            max_version,
        }
    }

    pub async fn build_unix<P>(
        self,
        path: P,
    ) -> DaemonResult<DaemonHandshakeClient<OwnedReadHalf, OwnedWriteHalf>>
    where
        P: AsRef<Path>,
    {
        let stream = UnixStream::connect(path).await?;
        let (reader, writer) = stream.into_split();
        Ok(self.build(reader, writer))
    }

    pub async fn build_daemon(
        self,
    ) -> DaemonResult<DaemonHandshakeClient<OwnedReadHalf, OwnedWriteHalf>> {
        self.build_unix("/nix/var/nix/daemon-socket/socket").await
    }

    pub fn connect<R, W>(
        self,
        reader: R,
        writer: W,
    ) -> impl ResultLog<Output = DaemonResult<DaemonClient<R, W>>>
    where
        R: AsyncRead + fmt::Debug + Unpin + Send + 'static,
        W: AsyncWrite + fmt::Debug + Unpin + Send + 'static,
    {
        self.build(reader, writer).handshake()
    }

    pub fn connect_unix<P>(
        self,
        path: P,
    ) -> impl ResultLog<Output = DaemonResult<DaemonClient<OwnedReadHalf, OwnedWriteHalf>>>
    where
        P: AsRef<Path> + Send,
    {
        async move { Ok(self.build_unix(path).await?.handshake()) }.future_result()
    }

    pub fn connect_daemon(
        self,
    ) -> impl ResultLog<Output = DaemonResult<DaemonClient<OwnedReadHalf, OwnedWriteHalf>>> {
        async move { Ok(self.build_daemon().await?.handshake()) }.future_result()
    }
}

#[derive(Debug)]
pub struct DaemonHandshakeClient<R, W> {
    host: String,
    min_version: ProtocolVersion,
    max_version: ProtocolVersion,
    reader: NixReader<Lending<BytesReader<R>, NarBytesReader<BytesReader<R>>>>,
    writer: NixWriter<W>,
}

impl<R, W> HandshakeDaemonStore for DaemonHandshakeClient<R, W>
where
    R: AsyncRead + fmt::Debug + Unpin + Send + 'static,
    W: AsyncWrite + fmt::Debug + Unpin + Send + 'static,
{
    type Store = DaemonClient<R, W>;

    fn handshake(self) -> impl ResultLog<Output = DaemonResult<Self::Store>> {
        async move {
            let mut reader = self.reader;
            let mut writer = self.writer;
            let mut daemon_nix_version = None;
            let mut remote_trusts_us = TrustLevel::Unknown;

            // Send the magic greeting, check for the reply.
            writer
                .write_number(CLIENT_MAGIC)
                .await
                .with_field("clientMagic")?;
            writer.flush().await.with_field("clientMagic")?;

            let magic = reader.read_number().await.with_field("serverMagic")?;
            if magic != SERVER_MAGIC {
                return Err(DaemonErrorKind::WrongMagic(magic)).with_field("serverMagic");
            }

            let server_version: ProtocolVersion =
                reader.read_value().await.with_field("protocolVersion")?;
            let version = server_version.min(self.max_version);
            if version < self.min_version {
                return Err(DaemonErrorKind::UnsupportedVersion(version))
                    .with_field("protocolVersion");
            }
            writer
                .write_value(&version)
                .await
                .with_field("clientVersion")?;
            reader.set_version(version);
            writer.set_version(version);
            info!(
                ?version,
                ?server_version,
                "Client Version is {}, server version is {}",
                version,
                server_version
            );

            if version.minor() >= 14 {
                // Obsolete CPU Affinity
                writer.write_value(&false).await.with_field("sendCpu")?;
            }

            if version.minor() >= 11 {
                // Obsolete reserved space
                writer
                    .write_value(&false)
                    .await
                    .with_field("reserveSpace")?;
            }

            if version.minor() >= 33 {
                writer.flush().await?;
                let version = reader.read_value().await.with_field("nixVersion")?;
                info!(version, "Nix Version {:?}", version);
                daemon_nix_version = Some(version);
            }

            if version.minor() >= 35 {
                remote_trusts_us = reader.read_value().await.with_field("trusted")?;
            }

            writer.flush().await?;

            let host = self.host;

            Ok(ProcessStderr::new(reader)
                .result_fn(move |result, reader, _, _, _| async move {
                    result?;
                    let id = CLIENT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(DaemonClient {
                        id,
                        host,
                        reader,
                        writer,
                        daemon_nix_version,
                        remote_trusts_us,
                    })
                })
                .stream())
        }
        .future_result()
    }
}

static CLIENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct DaemonClient<R, W> {
    id: u64,
    host: String,
    reader: NixReader<Lending<BytesReader<R>, NarBytesReader<BytesReader<R>>>>,
    writer: NixWriter<W>,
    daemon_nix_version: Option<String>,
    remote_trusts_us: TrustLevel,
}

impl DaemonClient<Cursor<Vec<u8>>, Cursor<Vec<u8>>> {
    pub fn builder() -> DaemonClientBuilder {
        DaemonClientBuilder::new()
    }
}

impl<R, W> DaemonClient<R, W> {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn daemon_nix_version(&self) -> Option<&str> {
        self.daemon_nix_version.as_deref()
    }
}

impl<R, W> DaemonClient<R, W>
where
    R: AsyncRead + fmt::Debug + Unpin + Send + 'static,
    W: AsyncWrite + fmt::Debug + Unpin + Send + 'static,
{
    pub async fn connect(
        store_dir: &StoreDir,
        host: String,
        reader: R,
        writer: W,
    ) -> impl ResultLog<Output = DaemonResult<Self>> {
        DaemonClient::builder()
            .set_store_dir(store_dir)
            .set_host(host)
            .build(reader, writer)
            .handshake()
    }

    pub fn version(&self) -> ProtocolVersion {
        self.writer.version()
    }

    fn process_stderr<T>(&mut self) -> impl ResultLog<Output = DaemonResult<T>> + '_
    where
        T: NixDeserialize + Send + 'static,
    {
        async {
            self.writer.flush().await?;
            Ok(ProcessStderr::new(&mut self.reader).stream())
        }
        .future_result()
    }
}

impl<R, W> DaemonStore for DaemonClient<R, W>
where
    R: AsyncRead + fmt::Debug + Unpin + Send + 'static,
    W: AsyncWrite + fmt::Debug + Unpin + Send + 'static,
{
    fn trust_level(&self) -> TrustLevel {
        self.remote_trusts_us
    }

    fn set_options<'a>(
        &'a mut self,
        options: &'a ClientOptions,
    ) -> impl ResultLog<Output = DaemonResult<()>> + 'a {
        async {
            self.writer.write_value(&Operation::SetOptions).await?;
            self.writer.write_value(options).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::SetOptions)
    }

    fn is_valid_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + 'a {
        async {
            self.writer.write_value(&Operation::IsValidPath).await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::IsValidPath)
    }

    fn query_valid_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
        substitute: bool,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + 'a {
        async move {
            self.writer.write_value(&Operation::QueryValidPaths).await?;
            self.writer.write_value(paths).await?;
            if (27..).contains(&self.writer.version().minor()) {
                self.writer.write_value(&substitute).await?;
            }
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryValidPaths)
    }

    fn query_path_info<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<Option<UnkeyedValidPathInfo>>> + 'a {
        async {
            self.writer.write_value(&Operation::QueryPathInfo).await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryPathInfo)
    }

    fn nar_from_path<'s>(
        &'s mut self,
        path: &'s StorePath,
    ) -> impl ResultLog<Output = DaemonResult<impl AsyncBufRead + use<R, W>>> + Send + 's {
        let reader = &mut self.reader;
        let writer = &mut self.writer;
        let (sender, receiver) = oneshot::channel();
        let logs = stream! {
            let w = async {
                writer.write_value(&Operation::NarFromPath).await?;
                writer.write_value(path).await?;
                writer.flush().await?;
                Ok(())
            }.await;
            let result;
            if w.is_err() {
                result = w;
            } else {
                loop {
                    let msg = reader.read_value::<RawLogMessage>().await;
                    match msg {
                        Ok(RawLogMessage::Next(text)) => {
                            yield LogMessage::Message(Message {
                                text,
                                level: Verbosity::Error
                            });
                        }
                        Ok(RawLogMessage::Result(result)) => {
                            yield LogMessage::Result(result);
                        }
                        Ok(RawLogMessage::StartActivity(act)) => {
                            yield LogMessage::StartActivity(act);
                        }
                        Ok(RawLogMessage::StopActivity(act)) => {
                            yield LogMessage::StopActivity(act);
                        }
                        Ok(RawLogMessage::Read(_len)) => {
                        }
                        Ok(RawLogMessage::Write(_buf)) => {
                        }
                        Ok(RawLogMessage::Last) => {
                            result = Ok(());
                            break;
                        }
                        Ok(RawLogMessage::Error(err)) => {
                            result = Err(err.into());
                            break;
                        }
                        Err(err) => {
                            result = Err(DaemonError::from(err));
                            break;
                        }
                    }
                }
            }
            let _ = sender.send((reader, result));
        };
        async {
            let (reader, result) = receiver.await.unwrap();
            let reader = reader.get_mut().lend(NarBytesReader::new);
            match result {
                Ok(_) => Ok(AsyncBufReadCompat::new(reader)),
                Err(err) => Err(err.fill_operation(Operation::NarFromPath)),
            }
        }
        .with_logs(logs)
    }

    fn build_paths<'a>(
        &'a mut self,
        paths: &'a [DerivedPath],
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<()>> + 'a {
        async move {
            self.writer.write_value(&Operation::BuildPaths).await?;
            self.writer.write_value(&paths).await?;
            self.writer.write_value(&mode).await?;
            Ok(self.process_stderr().map_ok(|_ignored: IgnoredOne| ()))
        }
        .future_result()
        .fill_operation(Operation::BuildPaths)
    }

    fn build_paths_with_results<'a>(
        &'a mut self,
        drvs: &'a [DerivedPath],
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<Vec<KeyedBuildResult>>> + Send + 'a {
        async move {
            self.writer
                .write_value(&Operation::BuildPathsWithResults)
                .await?;
            self.writer.write_value(&drvs).await?;
            self.writer.write_value(&mode).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::BuildPathsWithResults)
    }

    fn build_derivation<'a>(
        &'a mut self,
        drv_path: &'a StorePath,
        drv: &'a BasicDerivation,
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<harmonia_protocol::daemon_wire::types2::BuildResult>> + 'a
    {
        async move {
            self.writer.write_value(&Operation::BuildDerivation).await?;
            // Wire protocol expects (StorePath, BasicDerivation) tuple
            self.writer
                .write_value(&(drv_path.clone(), drv.clone()))
                .await?;
            self.writer.write_value(&mode).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::BuildDerivation)
    }

    fn query_missing<'a>(
        &'a mut self,
        paths: &'a [DerivedPath],
    ) -> impl ResultLog<Output = DaemonResult<QueryMissingResult>> + 'a {
        async move {
            trace!(paths = paths.len(), "Sending QueryMissing");
            self.writer.write_value(&Operation::QueryMissing).await?;
            self.writer.write_value(&paths).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryMissing)
    }

    fn add_to_store_nar<'s, 'r, 'i, AR>(
        &'s mut self,
        info: &'i ValidPathInfo,
        source: AR,
        repair: bool,
        dont_check_sigs: bool,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        AR: AsyncBufRead + Send + Unpin + 'r,
        's: 'r,
        'i: 'r,
    {
        async move {
            self.writer.write_value(&Operation::AddToStoreNar).await?;
            self.writer.write_value(info).await?;
            self.writer.write_value(&repair).await?;
            self.writer.write_value(&dont_check_sigs).await?;
            self.writer.flush().await?;
            if self.writer.version().minor() >= 23 {
                Ok(Either::Left(Either::Left(Box::pin(
                    ProcessStderr::new(&mut self.reader)
                        .stream()
                        .drive_result(async {
                            let mut source = source;
                            let mut framed = FramedWriter::new(&mut self.writer);
                            trace!("client:add_to_store_nar:driver: copy_buf");
                            copy_buf(&mut source, &mut framed).await?;
                            framed.shutdown().await?;
                            trace!("client:add_to_store_nar:driver: flush");
                            self.writer.flush().await?;
                            trace!("client:add_to_store_nar:driver: done");
                            Ok(()) as DaemonResult<()>
                        }),
                ))))
            } else if self.writer.version().minor() >= 21 {
                Ok(Either::Left(Either::Right(Box::pin(
                    ProcessStderr::new(&mut self.reader)
                        .with_source(&mut self.writer, source)
                        .stream(),
                ))))
            } else {
                Ok(Either::Right(Box::pin(
                    ProcessStderr::new(&mut self.reader)
                        .stream()
                        .drive_result(async {
                            let mut reader = NarReader::new(source);
                            copy_buf(&mut reader, &mut self.writer).await?;
                            self.writer.flush().await?;
                            Ok(()) as DaemonResult<()>
                        }),
                )))
            }
        }
        .future_result()
        .fill_operation(Operation::AddToStoreNar)
        .boxed_result()
    }

    fn add_multiple_to_store<'s, 'i, 'r, S, SR>(
        &'s mut self,
        repair: bool,
        dont_check_sigs: bool,
        stream: S,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        S: Stream<Item = Result<AddToStoreItem<SR>, DaemonError>> + Send + 'i,
        SR: AsyncBufRead + Send + Unpin + 'i,
        's: 'r,
        'i: 'r,
    {
        async move {
            self.writer
                .write_value(&Operation::AddMultipleToStore)
                .await?;
            self.writer.write_value(&repair).await?;
            self.writer.write_value(&dont_check_sigs).await?;

            info!(self.id, "add_multiple_to_store {}", self.id);
            let driver = async {
                let id = self.id;
                let version = self.writer.version();
                let mut writer = NixWriter::builder()
                    .set_version(version)
                    .build(FramedWriter::new(&mut self.writer));

                debug!(id, "Write write stream");
                write_add_multiple_to_store_stream(&mut writer, stream).await?;
                debug!(id, "Write writer shutdown");
                writer.shutdown().await?;
                debug!(id, "Write self.writer flush");
                self.writer.flush().await?;
                debug!(id, "Write done");
                Ok(()) as DaemonResult<()>
            };
            Ok(ProcessStderr::new(&mut self.reader)
                .stream()
                .drive_result(driver))
        }
        .future_result()
        .fill_operation(Operation::AddMultipleToStore)
        .boxed_result()
    }

    fn query_all_valid_paths(
        &mut self,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + '_ {
        async move {
            self.writer
                .write_value(&Operation::QueryAllValidPaths)
                .await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryAllValidPaths)
    }

    fn query_referrers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        async move {
            self.writer.write_value(&Operation::QueryReferrers).await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryReferrers)
    }

    fn ensure_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        async move {
            self.writer.write_value(&Operation::EnsurePath).await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::EnsurePath)
    }

    fn add_temp_root<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        async move {
            self.writer.write_value(&Operation::AddTempRoot).await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::AddTempRoot)
    }

    fn add_indirect_root<'a>(
        &'a mut self,
        path: &'a DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        async move {
            self.writer.write_value(&Operation::AddIndirectRoot).await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::AddIndirectRoot)
    }

    fn find_roots(
        &mut self,
    ) -> impl ResultLog<Output = DaemonResult<BTreeMap<DaemonPath, StorePath>>> + Send + '_ {
        async move {
            self.writer.write_value(&Operation::FindRoots).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::FindRoots)
    }

    fn collect_garbage<'a>(
        &'a mut self,
        action: GCAction,
        paths_to_delete: &'a StorePathSet,
        ignore_liveness: bool,
        max_freed: u64,
    ) -> impl ResultLog<Output = DaemonResult<CollectGarbageResponse>> + Send + 'a {
        async move {
            self.writer.write_value(&Operation::CollectGarbage).await?;
            self.writer.write_value(&action).await?;
            self.writer.write_value(paths_to_delete).await?;
            self.writer.write_value(&ignore_liveness).await?;
            self.writer.write_value(&max_freed).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::CollectGarbage)
    }

    fn query_path_from_hash_part<'a>(
        &'a mut self,
        hash: &'a StorePathHash,
    ) -> impl ResultLog<Output = DaemonResult<Option<StorePath>>> + Send + 'a {
        async move {
            self.writer
                .write_value(&Operation::QueryPathFromHashPart)
                .await?;
            self.writer.write_value(hash).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryPathFromHashPart)
    }

    fn query_substitutable_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        async move {
            self.writer
                .write_value(&Operation::QuerySubstitutablePaths)
                .await?;
            self.writer.write_value(paths).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QuerySubstitutablePaths)
    }

    fn query_valid_derivers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        async move {
            self.writer
                .write_value(&Operation::QueryValidDerivers)
                .await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryValidDerivers)
    }

    fn optimise_store(&mut self) -> impl ResultLog<Output = DaemonResult<()>> + Send + '_ {
        async move {
            self.writer.write_value(&Operation::OptimiseStore).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::OptimiseStore)
    }

    fn verify_store(
        &mut self,
        check_contents: bool,
        repair: bool,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + '_ {
        async move {
            self.writer.write_value(&Operation::VerifyStore).await?;
            self.writer.write_value(&check_contents).await?;
            self.writer.write_value(&repair).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::VerifyStore)
    }

    fn add_signatures<'a>(
        &'a mut self,
        path: &'a StorePath,
        signatures: &'a [Signature],
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        async move {
            self.writer.write_value(&Operation::AddSignatures).await?;
            self.writer.write_value(path).await?;
            self.writer.write_value(&signatures).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::AddSignatures)
    }

    fn query_derivation_output_map<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<BTreeMap<OutputName, Option<StorePath>>>> + Send + 'a
    {
        async move {
            self.writer
                .write_value(&Operation::QueryDerivationOutputMap)
                .await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryDerivationOutputMap)
    }

    fn register_drv_output<'a>(
        &'a mut self,
        realisation: &'a Realisation,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        async move {
            self.writer
                .write_value(&Operation::RegisterDrvOutput)
                .await?;
            self.writer.write_value(realisation).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::RegisterDrvOutput)
    }

    fn query_realisation<'a>(
        &'a mut self,
        output_id: &'a DrvOutput,
    ) -> impl ResultLog<Output = DaemonResult<BTreeSet<Realisation>>> + Send + 'a {
        async move {
            self.writer
                .write_value(&Operation::QueryRealisation)
                .await?;
            self.writer.write_value(output_id).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryRealisation)
    }

    fn add_build_log<'s, 'r, 'p, S>(
        &'s mut self,
        path: &'p StorePath,
        source: S,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        S: AsyncBufRead + Send + Unpin + 'r,
        's: 'r,
        'p: 'r,
    {
        async move {
            self.writer.write_value(&Operation::AddBuildLog).await?;
            self.writer.write_value(path).await?;
            self.writer.flush().await?;
            Ok(ProcessStderr::new(&mut self.reader)
                .stream()
                .drive_result(async {
                    let mut source = source;
                    let mut framed = FramedWriter::new(&mut self.writer);
                    copy_buf(&mut source, &mut framed).await?;
                    framed.shutdown().await?;
                    self.writer.flush().await?;
                    Ok(()) as DaemonResult<()>
                }))
        }
        .future_result()
        .fill_operation(Operation::AddBuildLog)
        .boxed_result()
    }

    fn add_perm_root<'a>(
        &'a mut self,
        path: &'a StorePath,
        gc_root: &'a DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<DaemonPath>> + Send + 'a {
        async move {
            self.writer.write_value(&Operation::AddPermRoot).await?;
            self.writer.write_value(path).await?;
            self.writer.write_value(gc_root).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::AddPermRoot)
    }

    fn sync_with_gc(&mut self) -> impl ResultLog<Output = DaemonResult<()>> + Send + '_ {
        async move {
            self.writer.write_value(&Operation::SyncWithGC).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::SyncWithGC)
    }

    fn query_derivation_outputs<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        async move {
            self.writer
                .write_value(&Operation::QueryDerivationOutputs)
                .await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryDerivationOutputs)
    }

    fn query_derivation_output_names<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<BTreeSet<OutputName>>> + Send + 'a {
        async move {
            self.writer
                .write_value(&Operation::QueryDerivationOutputNames)
                .await?;
            self.writer.write_value(path).await?;
            Ok(self.process_stderr())
        }
        .future_result()
        .fill_operation(Operation::QueryDerivationOutputNames)
    }

    fn add_ca_to_store<'a, 'r, S>(
        &'a mut self,
        name: &'a str,
        cam: ContentAddressMethodAlgorithm,
        refs: &'a StorePathSet,
        repair: bool,
        source: S,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<ValidPathInfo>> + Send + 'r>>
    where
        S: AsyncBufRead + Send + Unpin + 'r,
        'a: 'r,
    {
        async move {
            self.writer.write_value(&Operation::AddToStore).await?;
            self.writer.write_value(name).await?;
            self.writer.write_value(&cam).await?;
            self.writer.write_value(refs).await?;
            self.writer.write_value(&repair).await?;
            self.writer.flush().await?;
            Ok(ProcessStderr::new(&mut self.reader)
                .stream()
                .drive_result(async {
                    let mut source = source;
                    let mut framed = FramedWriter::new(&mut self.writer);
                    copy_buf(&mut source, &mut framed).await?;
                    framed.shutdown().await?;
                    self.writer.flush().await?;
                    Ok(()) as DaemonResult<()>
                }))
        }
        .future_result()
        .fill_operation(Operation::AddToStore)
        .boxed_result()
    }

    async fn shutdown(&mut self) -> DaemonResult<()> {
        self.writer.shutdown().await?;
        Ok(())
    }
}
