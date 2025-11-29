use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::{Future, ready};
use std::pin::Pin;

use bstr::ByteSlice;
use bytes::Bytes;
use futures::Stream;
use num_enum::{IntoPrimitive, TryFromPrimitive};
#[cfg(test)]
use test_strategy::Arbitrary;
use thiserror::Error;
use tokio::io::AsyncBufRead;

use crate::ProtocolVersion;
use crate::daemon_wire::logger::{FutureResultExt, LogError, ResultLog, ResultLogExt, TraceLine};
use crate::daemon_wire::types::Operation;
use crate::daemon_wire::types2::{
    BuildMode, BuildResult, CollectGarbageResponse, GCAction, KeyedBuildResult, QueryMissingResult,
    ValidPathInfo,
};
use crate::daemon_wire::{IgnoredTrue, IgnoredZero};
use crate::log::Verbosity;
use harmonia_protocol_derive::{NixDeserialize, NixSerialize};
use harmonia_store_core::derivation::BasicDerivation;
use harmonia_store_core::derived_path::{DerivedPath, OutputName};
use harmonia_store_core::realisation::{DrvOutput, Realisation};
use harmonia_store_core::signature::Signature;
#[cfg(test)]
use harmonia_store_core::signature::proptests::arb_signatures;
use harmonia_store_core::store_path::{
    ContentAddress, ContentAddressMethodAlgorithm, StorePath, StorePathHash, StorePathSet,
};
use harmonia_utils_hash::NarHash;

pub type DaemonString = Bytes;
pub type DaemonPath = Bytes;
pub type DaemonInt = libc::c_uint;
pub type DaemonTime = libc::time_t;

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct ClientOptions {
    pub keep_failed: bool,
    pub keep_going: bool,
    pub try_fallback: bool,
    pub verbosity: Verbosity,
    pub max_build_jobs: DaemonInt,
    pub max_silent_time: DaemonTime,
    _use_build_hook: IgnoredTrue,
    pub verbose_build: Verbosity,
    _log_type: IgnoredZero,
    _print_build_trace: IgnoredZero,
    pub build_cores: DaemonInt,
    pub use_substitutes: bool,
    pub other_settings: BTreeMap<String, DaemonString>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            keep_failed: Default::default(),
            keep_going: Default::default(),
            try_fallback: Default::default(),
            verbosity: Default::default(),
            max_build_jobs: 1,
            max_silent_time: Default::default(),
            _use_build_hook: Default::default(),
            verbose_build: Default::default(),
            _log_type: Default::default(),
            _print_build_trace: Default::default(),
            build_cores: 1,
            use_substitutes: true,
            other_settings: Default::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct UnkeyedValidPathInfo {
    pub deriver: Option<StorePath>,
    pub nar_hash: NarHash,
    pub references: BTreeSet<StorePath>,
    pub registration_time: DaemonTime,
    pub nar_size: u64,
    pub ultimate: bool,
    #[cfg_attr(test, strategy(arb_signatures()))]
    pub signatures: BTreeSet<Signature>,
    pub ca: Option<ContentAddress>,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    TryFromPrimitive,
    IntoPrimitive,
    NixDeserialize,
    NixSerialize,
)]
#[nix(try_from = "u64", into = "u64")]
#[repr(u64)]
pub enum TrustLevel {
    Unknown = 0,
    Trusted = 1,
    NotTrusted = 2,
}

pub type DaemonResult<T> = Result<T, DaemonError>;
pub trait DaemonResultExt<T> {
    fn with_operation(self, op: Operation) -> DaemonResult<T>;
    fn with_field(self, field: &'static str) -> DaemonResult<T>;
}
impl<T, E> DaemonResultExt<T> for Result<T, E>
where
    E: Into<DaemonError>,
{
    fn with_operation(self, op: Operation) -> DaemonResult<T> {
        self.map_err(|err| err.into().fill_operation(op))
    }

    fn with_field(self, field: &'static str) -> DaemonResult<T> {
        self.map_err(|err| {
            let mut err = err.into();
            err.context.fields.push(field);
            err
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct DaemonErrorContext {
    operation: Option<Operation>,
    fields: Vec<&'static str>,
}

impl fmt::Display for DaemonErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(op) = self.operation.as_ref() {
            write!(f, "{op}")?;
            for field in self.fields.iter() {
                write!(f, ".{field}")?;
            }
        } else {
            let mut it = self.fields.iter();
            if let Some(field) = it.next() {
                f.write_str(field)?;
                for field in it {
                    write!(f, ".{field}")?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DaemonError {
    context: DaemonErrorContext,
    kind: DaemonErrorKind,
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only show context if it has meaningful content
        let has_context = self.context.operation.is_some() || !self.context.fields.is_empty();
        if has_context {
            write!(f, "{}: {}", self.context, self.kind)
        } else {
            write!(f, "{}", self.kind)
        }
    }
}

impl std::error::Error for DaemonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.kind.source()
    }
}

impl DaemonError {
    pub fn custom<D: fmt::Display>(source: D) -> Self {
        DaemonErrorKind::Custom(source.to_string()).into()
    }
    pub fn unimplemented(op: Operation) -> Self {
        DaemonError {
            kind: DaemonErrorKind::UnimplementedOperation(op),
            context: DaemonErrorContext {
                operation: Some(op),
                ..Default::default()
            },
        }
    }
    pub fn fill_operation(mut self, op: Operation) -> Self {
        if self.context.operation.is_none() {
            self.context.operation = Some(op);
        }
        self
    }
    pub fn kind(&self) -> &DaemonErrorKind {
        &self.kind
    }

    pub fn operation(&self) -> Option<&Operation> {
        self.context.operation.as_ref()
    }

    pub fn fields(&self) -> &[&'static str] {
        &self.context.fields
    }
}

#[derive(Error, Debug)]
pub enum DaemonErrorKind {
    #[error("wrong magic 0x{0:x}")]
    WrongMagic(u64),
    #[error("unsupported version {0}")]
    UnsupportedVersion(ProtocolVersion),
    #[error("unimplemented operation '{0:?}'")]
    UnimplementedOperation(Operation),
    #[error("no source for logger write")]
    NoSinkForLoggerWrite,
    #[error("no sink for logger read")]
    NoSourceForLoggerRead,
    #[error("io error {0}")]
    IO(
        #[from]
        #[source]
        std::io::Error,
    ),
    #[error("remote error: {0}")]
    Remote(
        #[from]
        #[source]
        RemoteError,
    ),
    #[error("{0}")]
    Custom(String),
}

impl Clone for DaemonErrorKind {
    fn clone(&self) -> Self {
        match self {
            Self::WrongMagic(arg0) => Self::WrongMagic(*arg0),
            Self::UnsupportedVersion(arg0) => Self::UnsupportedVersion(*arg0),
            Self::UnimplementedOperation(arg0) => Self::UnimplementedOperation(*arg0),
            Self::NoSinkForLoggerWrite => Self::NoSinkForLoggerWrite,
            Self::NoSourceForLoggerRead => Self::NoSourceForLoggerRead,
            Self::IO(arg0) => Self::IO(std::io::Error::new(arg0.kind(), arg0.to_string())),
            Self::Remote(arg0) => Self::Remote(arg0.clone()),
            Self::Custom(arg0) => Self::Custom(arg0.clone()),
        }
    }
}

impl From<LogError> for DaemonError {
    fn from(value: LogError) -> Self {
        DaemonError {
            context: DaemonErrorContext::default(),
            kind: DaemonErrorKind::Remote(value.into()),
        }
    }
}

impl From<std::io::Error> for DaemonError {
    fn from(value: std::io::Error) -> Self {
        DaemonError {
            context: DaemonErrorContext::default(),
            kind: DaemonErrorKind::IO(value),
        }
    }
}

impl From<RemoteError> for DaemonError {
    fn from(value: RemoteError) -> Self {
        DaemonError {
            context: DaemonErrorContext::default(),
            kind: DaemonErrorKind::Remote(value),
        }
    }
}

impl From<DaemonErrorKind> for DaemonError {
    fn from(kind: DaemonErrorKind) -> Self {
        DaemonError {
            context: DaemonErrorContext::default(),
            kind,
        }
    }
}

#[derive(Clone, Error, Debug, PartialEq, Eq, Hash)]
#[error("{}", msg.as_bstr())]
pub struct RemoteError {
    pub level: Verbosity,
    pub msg: DaemonString,
    pub exit_status: DaemonInt,
    pub traces: Vec<TraceLine>,
}

pub struct AddToStoreItem<R> {
    pub info: ValidPathInfo,
    pub reader: R,
}

pub trait HandshakeDaemonStore {
    type Store: DaemonStore + Send;
    fn handshake(self) -> impl ResultLog<Output = DaemonResult<Self::Store>> + Send;
}

#[allow(unused_variables)]
pub trait DaemonStore: Send {
    fn trust_level(&self) -> TrustLevel;

    /// Sets options on server.
    /// This is usually called by the client just after the handshake to set
    /// options for the rest of the session.
    fn set_options<'a>(
        &'a mut self,
        options: &'a ClientOptions,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::SetOptions))).empty_logs()
    }

    fn is_valid_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::IsValidPath))).empty_logs()
    }

    fn query_valid_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
        substitute: bool,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::QueryValidPaths))).empty_logs()
    }

    fn query_path_info<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<Option<UnkeyedValidPathInfo>>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::QueryPathInfo))).empty_logs()
    }

    fn nar_from_path<'s>(
        &'s mut self,
        path: &'s StorePath,
    ) -> impl ResultLog<Output = DaemonResult<impl AsyncBufRead + Send + use<Self>>> + Send + 's
    {
        ready(Err(DaemonError::unimplemented(Operation::NarFromPath)) as Result<&[u8], DaemonError>)
            .empty_logs()
    }

    fn build_paths<'a>(
        &'a mut self,
        drvs: &'a [DerivedPath],
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::BuildPaths))).empty_logs()
    }

    fn build_paths_with_results<'a>(
        &'a mut self,
        drvs: &'a [DerivedPath],
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<Vec<KeyedBuildResult>>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(
            Operation::BuildPathsWithResults,
        )))
        .empty_logs()
    }

    fn build_derivation<'a>(
        &'a mut self,
        drv_path: &'a StorePath,
        drv: &'a BasicDerivation,
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<BuildResult>> + Send + 'a {
        let _ = (drv_path, drv, mode);
        ready(Err(DaemonError::unimplemented(Operation::BuildDerivation))).empty_logs()
    }

    fn query_missing<'a>(
        &'a mut self,
        paths: &'a [DerivedPath],
    ) -> impl ResultLog<Output = DaemonResult<QueryMissingResult>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::QueryMissing))).empty_logs()
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
        ready(Err(DaemonError::unimplemented(Operation::AddToStoreNar)))
            .empty_logs()
            .boxed_result()
    }

    fn add_multiple_to_store<'s, 'i, 'r, S, R>(
        &'s mut self,
        repair: bool,
        dont_check_sigs: bool,
        stream: S,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        S: Stream<Item = Result<AddToStoreItem<R>, DaemonError>> + Send + 'i,
        R: AsyncBufRead + Send + Unpin + 'i,
        's: 'r,
        'i: 'r,
    {
        ready(Err(DaemonError::unimplemented(
            Operation::AddMultipleToStore,
        )))
        .empty_logs()
        .boxed_result()
    }

    fn query_all_valid_paths(
        &mut self,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + '_ {
        ready(Err(DaemonError::unimplemented(
            Operation::QueryAllValidPaths,
        )))
        .empty_logs()
    }

    fn query_referrers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::QueryReferrers))).empty_logs()
    }

    fn ensure_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::EnsurePath))).empty_logs()
    }

    fn add_temp_root<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::AddTempRoot))).empty_logs()
    }

    fn add_indirect_root<'a>(
        &'a mut self,
        path: &'a DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::AddIndirectRoot))).empty_logs()
    }

    fn find_roots(
        &mut self,
    ) -> impl ResultLog<Output = DaemonResult<BTreeMap<DaemonPath, StorePath>>> + Send + '_ {
        ready(Err(DaemonError::unimplemented(Operation::FindRoots))).empty_logs()
    }

    fn collect_garbage<'a>(
        &'a mut self,
        action: GCAction,
        paths_to_delete: &'a StorePathSet,
        ignore_liveness: bool,
        max_freed: u64,
    ) -> impl ResultLog<Output = DaemonResult<CollectGarbageResponse>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::CollectGarbage))).empty_logs()
    }

    fn query_path_from_hash_part<'a>(
        &'a mut self,
        hash: &'a StorePathHash,
    ) -> impl ResultLog<Output = DaemonResult<Option<StorePath>>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(
            Operation::QueryPathFromHashPart,
        )))
        .empty_logs()
    }

    fn query_substitutable_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(
            Operation::QuerySubstitutablePaths,
        )))
        .empty_logs()
    }

    fn query_valid_derivers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(
            Operation::QueryValidDerivers,
        )))
        .empty_logs()
    }

    fn optimise_store(&mut self) -> impl ResultLog<Output = DaemonResult<()>> + Send + '_ {
        ready(Err(DaemonError::unimplemented(Operation::OptimiseStore))).empty_logs()
    }

    fn verify_store(
        &mut self,
        check_contents: bool,
        repair: bool,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + '_ {
        ready(Err(DaemonError::unimplemented(Operation::VerifyStore))).empty_logs()
    }

    fn add_signatures<'a>(
        &'a mut self,
        path: &'a StorePath,
        signatures: &'a [Signature],
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::AddSignatures))).empty_logs()
    }

    fn query_derivation_output_map<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<BTreeMap<OutputName, Option<StorePath>>>> + Send + 'a
    {
        ready(Err(DaemonError::unimplemented(
            Operation::QueryDerivationOutputMap,
        )))
        .empty_logs()
    }

    fn register_drv_output<'a>(
        &'a mut self,
        realisation: &'a Realisation,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(
            Operation::RegisterDrvOutput,
        )))
        .empty_logs()
    }

    fn query_realisation<'a>(
        &'a mut self,
        output_id: &'a DrvOutput,
    ) -> impl ResultLog<Output = DaemonResult<BTreeSet<Realisation>>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::QueryRealisation))).empty_logs()
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
        ready(Err(DaemonError::unimplemented(Operation::AddBuildLog)))
            .empty_logs()
            .boxed_result()
    }

    fn add_perm_root<'a>(
        &'a mut self,
        path: &'a StorePath,
        gc_root: &'a DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<DaemonPath>> + Send + 'a {
        ready(Err(DaemonError::unimplemented(Operation::AddPermRoot))).empty_logs()
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
        ready(Err(DaemonError::unimplemented(Operation::AddToStore)))
            .empty_logs()
            .boxed_result()
    }

    fn shutdown(&mut self) -> impl Future<Output = DaemonResult<()>> + Send + '_;
}

#[warn(clippy::missing_trait_methods)]
impl<'os, S> DaemonStore for &'os mut S
where
    S: DaemonStore,
{
    fn trust_level(&self) -> TrustLevel {
        (**self).trust_level()
    }

    fn set_options<'a>(
        &'a mut self,
        options: &'a ClientOptions,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        (**self).set_options(options)
    }

    fn is_valid_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + 'a {
        (**self).is_valid_path(path)
    }

    fn query_valid_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
        substitute: bool,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        (**self).query_valid_paths(paths, substitute)
    }

    fn query_path_info<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<Option<UnkeyedValidPathInfo>>> + Send + 'a {
        (**self).query_path_info(path)
    }

    fn nar_from_path<'s>(
        &'s mut self,
        path: &'s StorePath,
    ) -> impl ResultLog<Output = DaemonResult<impl AsyncBufRead + use<'os, S>>> + Send + 's {
        (**self).nar_from_path(path)
    }

    fn build_paths<'a>(
        &'a mut self,
        paths: &'a [DerivedPath],
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        (**self).build_paths(paths, mode)
    }

    fn build_derivation<'a>(
        &'a mut self,
        drv_path: &'a StorePath,
        drv: &'a BasicDerivation,
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<BuildResult>> + 'a {
        (**self).build_derivation(drv_path, drv, mode)
    }

    fn query_missing<'a>(
        &'a mut self,
        paths: &'a [DerivedPath],
    ) -> impl ResultLog<Output = DaemonResult<QueryMissingResult>> + 'a {
        (**self).query_missing(paths)
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
        (**self).add_to_store_nar(info, source, repair, dont_check_sigs)
    }

    fn add_multiple_to_store<'s, 'i, 'r, I, R>(
        &'s mut self,
        repair: bool,
        dont_check_sigs: bool,
        stream: I,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        I: Stream<Item = Result<AddToStoreItem<R>, DaemonError>> + Send + 'i,
        R: AsyncBufRead + Send + Unpin + 'i,
        's: 'r,
        'i: 'r,
    {
        (**self).add_multiple_to_store(repair, dont_check_sigs, stream)
    }

    fn build_paths_with_results<'a>(
        &'a mut self,
        drvs: &'a [DerivedPath],
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<Vec<KeyedBuildResult>>> + Send + 'a {
        (**self).build_paths_with_results(drvs, mode)
    }

    fn query_all_valid_paths(
        &mut self,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + '_ {
        (**self).query_all_valid_paths()
    }

    fn query_referrers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        (**self).query_referrers(path)
    }

    fn ensure_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        (**self).ensure_path(path)
    }

    fn add_temp_root<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        (**self).add_temp_root(path)
    }

    fn add_indirect_root<'a>(
        &'a mut self,
        path: &'a DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        (**self).add_indirect_root(path)
    }

    fn find_roots(
        &mut self,
    ) -> impl ResultLog<Output = DaemonResult<BTreeMap<DaemonPath, StorePath>>> + Send + '_ {
        (**self).find_roots()
    }

    fn collect_garbage<'a>(
        &'a mut self,
        action: GCAction,
        paths_to_delete: &'a StorePathSet,
        ignore_liveness: bool,
        max_freed: u64,
    ) -> impl ResultLog<Output = DaemonResult<CollectGarbageResponse>> + Send + 'a {
        (**self).collect_garbage(action, paths_to_delete, ignore_liveness, max_freed)
    }

    fn query_path_from_hash_part<'a>(
        &'a mut self,
        hash: &'a StorePathHash,
    ) -> impl ResultLog<Output = DaemonResult<Option<StorePath>>> + Send + 'a {
        (**self).query_path_from_hash_part(hash)
    }

    fn query_substitutable_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        (**self).query_substitutable_paths(paths)
    }

    fn query_valid_derivers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        (**self).query_valid_derivers(path)
    }

    fn optimise_store(&mut self) -> impl ResultLog<Output = DaemonResult<()>> + Send + '_ {
        (**self).optimise_store()
    }

    fn verify_store(
        &mut self,
        check_contents: bool,
        repair: bool,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + '_ {
        (**self).verify_store(check_contents, repair)
    }

    fn add_signatures<'a>(
        &'a mut self,
        path: &'a StorePath,
        signatures: &'a [Signature],
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        (**self).add_signatures(path, signatures)
    }

    fn query_derivation_output_map<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<BTreeMap<OutputName, Option<StorePath>>>> + Send + 'a
    {
        (**self).query_derivation_output_map(path)
    }

    fn register_drv_output<'a>(
        &'a mut self,
        realisation: &'a Realisation,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        (**self).register_drv_output(realisation)
    }

    fn query_realisation<'a>(
        &'a mut self,
        output_id: &'a DrvOutput,
    ) -> impl ResultLog<Output = DaemonResult<BTreeSet<Realisation>>> + Send + 'a {
        (**self).query_realisation(output_id)
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
        (**self).add_build_log(path, source)
    }

    fn add_perm_root<'a>(
        &'a mut self,
        path: &'a StorePath,
        gc_root: &'a DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<DaemonPath>> + Send + 'a {
        (**self).add_perm_root(path, gc_root)
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
        (**self).add_ca_to_store(name, cam, refs, repair, source)
    }

    fn shutdown(&mut self) -> impl Future<Output = DaemonResult<()>> + Send + '_ {
        (**self).shutdown()
    }
}

#[cfg(test)]
mod proptests {
    use ::proptest::collection::btree_map;
    use ::proptest::prelude::*;
    use ::proptest::sample::SizeRange;

    use super::*;

    fn arb_client_settings(
        size: impl Into<SizeRange>,
    ) -> impl Strategy<Value = BTreeMap<String, DaemonString>> {
        let key = any::<String>();
        let value = any::<Vec<u8>>().prop_map(DaemonString::from);
        btree_map(key, value, size)
    }

    impl Arbitrary for ClientOptions {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            (
                any::<bool>(),
                any::<bool>(),
                any::<bool>(),
                any::<Verbosity>(),
                any::<DaemonInt>(),
                any::<DaemonTime>(),
                any::<Verbosity>(),
                any::<DaemonInt>(),
                any::<bool>(),
                arb_client_settings(..30),
            )
                .prop_map(
                    |(
                        keep_failed,
                        keep_going,
                        try_fallback,
                        verbosity,
                        max_build_jobs,
                        max_silent_time,
                        verbose_build,
                        build_cores,
                        use_substitutes,
                        other_settings,
                    )| {
                        ClientOptions {
                            keep_failed,
                            keep_going,
                            try_fallback,
                            verbosity,
                            max_build_jobs,
                            max_silent_time,
                            verbose_build,
                            build_cores,
                            use_substitutes,
                            other_settings,
                            _use_build_hook: IgnoredTrue,
                            _log_type: IgnoredZero,
                            _print_build_trace: IgnoredZero,
                        }
                    },
                )
                .boxed()
        }
    }
}
