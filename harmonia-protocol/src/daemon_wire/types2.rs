pub use crate::build_result::{BuildResult, BuildStatus, KeyedBuildResult, KeyedBuildResults};

use std::fmt;
use std::str::FromStr;
use std::str::from_utf8;
use std::time::Duration;

use num_enum::{IntoPrimitive, TryFromPrimitive};
#[cfg(test)]
use test_strategy::Arbitrary;
use tracing::{Span, debug_span};

use crate::de::{Error as _, NixDeserialize as NixDeserializeTrait, NixRead};
use crate::ser::{NixSerialize as NixSerializeTrait, NixWrite};
use crate::types::{ClientOptions, DaemonPath, DaemonString};
use crate::valid_path_info::{UnkeyedValidPathInfo, ValidPathInfo};
use harmonia_protocol_derive::{NixDeserialize, NixSerialize};
use harmonia_store_core::derivation::BasicDerivation;
use harmonia_store_core::derived_path::DerivedPath;
use harmonia_store_core::realisation::{DrvOutput, Realisation};
use harmonia_store_core::signature::Signature;
use harmonia_store_core::store_path::{
    ContentAddress, ContentAddressMethodAlgorithm, StorePath, StorePathHash, StorePathSet,
};

use super::IgnoredZero;
use super::types::Operation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[nix(from = "i64", into = "i64")]
pub struct Microseconds(i64);

impl From<i64> for Microseconds {
    fn from(value: i64) -> Self {
        Microseconds(value)
    }
}

impl From<Microseconds> for Duration {
    fn from(value: Microseconds) -> Self {
        Duration::from_micros(value.0.unsigned_abs())
    }
}

impl TryFrom<Duration> for Microseconds {
    type Error = std::num::TryFromIntError;
    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        Ok(Microseconds(value.as_micros().try_into()?))
    }
}

impl From<Microseconds> for i64 {
    fn from(value: Microseconds) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[nix(from_str, display)]
pub struct BaseStorePath(pub StorePath);
impl FromStr for BaseStorePath {
    type Err = crate::store_path::ParseStorePathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(BaseStorePath(StorePath::from_str(s)?))
    }
}
impl fmt::Display for BaseStorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
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
#[nix(try_from = "u16", into = "u16")]
#[repr(u16)]
pub enum FileIngestionMethod {
    Flat = 0,
    Recursive = 1,
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
#[nix(try_from = "u16", into = "u16")]
#[repr(u16)]
pub enum BuildMode {
    Normal = 0,
    Repair = 1,
    Check = 2,
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
    Default,
    NixDeserialize,
    NixSerialize,
)]
#[nix(try_from = "u16", into = "u16")]
#[repr(u16)]
pub enum GCAction {
    #[default]
    ReturnLive = 0,
    ReturnDead = 1,
    DeleteDead = 2,
    DeleteSpecific = 3,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[nix(tag = "Operation")]
pub enum Request {
    IsValidPath(StorePath),
    QueryReferrers(StorePath),
    AddToStore(AddToStoreRequest),
    BuildPaths(BuildPathsRequest),
    EnsurePath(StorePath),
    AddTempRoot(StorePath),
    AddIndirectRoot(DaemonPath),
    FindRoots,
    SetOptions(ClientOptions),
    CollectGarbage(CollectGarbageRequest),
    QueryAllValidPaths,
    QueryPathInfo(StorePath),
    QueryPathFromHashPart(StorePathHash),
    QueryValidPaths(QueryValidPathsRequest),
    QuerySubstitutablePaths(StorePathSet),
    QueryValidDerivers(StorePath),
    OptimiseStore,
    VerifyStore(VerifyStoreRequest),
    BuildDerivation(BuildDerivationRequest),
    AddSignatures(AddSignaturesRequest),
    NarFromPath(StorePath),
    AddToStoreNar(AddToStoreNarRequest),
    QueryMissing(Vec<DerivedPath>),
    QueryDerivationOutputMap(StorePath),
    RegisterDrvOutput(Realisation),
    QueryRealisation(DrvOutput),
    AddMultipleToStore(AddMultipleToStoreRequest),
    AddBuildLog(BaseStorePath),
    BuildPathsWithResults(BuildPathsRequest),
    AddPermRoot(AddPermRootRequest),
}

impl Request {
    pub fn operation(&self) -> Operation {
        match self {
            Request::IsValidPath(_) => Operation::IsValidPath,
            Request::QueryReferrers(_) => Operation::QueryReferrers,
            Request::AddToStore(_) => Operation::AddToStore,
            Request::BuildPaths(_) => Operation::BuildPaths,
            Request::EnsurePath(_) => Operation::EnsurePath,
            Request::AddTempRoot(_) => Operation::AddTempRoot,
            Request::AddIndirectRoot(_) => Operation::AddIndirectRoot,
            Request::FindRoots => Operation::FindRoots,
            Request::SetOptions(_) => Operation::SetOptions,
            Request::CollectGarbage(_) => Operation::CollectGarbage,
            Request::QueryAllValidPaths => Operation::QueryAllValidPaths,
            Request::QueryPathInfo(_) => Operation::QueryPathInfo,
            Request::QueryPathFromHashPart(_) => Operation::QueryPathFromHashPart,
            Request::QueryValidPaths(_) => Operation::QueryValidPaths,
            Request::QuerySubstitutablePaths(_) => Operation::QuerySubstitutablePaths,
            Request::QueryValidDerivers(_) => Operation::QueryValidDerivers,
            Request::OptimiseStore => Operation::OptimiseStore,
            Request::VerifyStore(_) => Operation::VerifyStore,
            Request::BuildDerivation(_) => Operation::BuildDerivation,
            Request::AddSignatures(_) => Operation::AddSignatures,
            Request::NarFromPath(_) => Operation::NarFromPath,
            Request::AddToStoreNar(_) => Operation::AddToStoreNar,
            Request::QueryMissing(_) => Operation::QueryMissing,
            Request::QueryDerivationOutputMap(_) => Operation::QueryDerivationOutputMap,
            Request::RegisterDrvOutput(_) => Operation::RegisterDrvOutput,
            Request::QueryRealisation(_) => Operation::QueryRealisation,
            Request::AddMultipleToStore(_) => Operation::AddMultipleToStore,
            Request::AddBuildLog(_) => Operation::AddBuildLog,
            Request::BuildPathsWithResults(_) => Operation::BuildPathsWithResults,
            Request::AddPermRoot(_) => Operation::AddPermRoot,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Request::IsValidPath(path) => debug_span!("IsValidPath", ?path),
            Request::QueryReferrers(path) => debug_span!("QueryReferrers", ?path),
            Request::AddToStore(req) => {
                debug_span!("AddToStore", name=?req.name, cam=?req.cam, refs=req.refs.len(), repair=req.repair)
            }
            Request::BuildPaths(req) => {
                debug_span!("BuildPaths", paths=req.paths.len(), mode=?req.mode)
            }
            Request::EnsurePath(path) => debug_span!("EnsurePath", ?path),
            Request::AddTempRoot(path) => debug_span!("AddTempRoot", ?path),
            Request::AddIndirectRoot(raw_path) => {
                let path = String::from_utf8_lossy(raw_path);
                debug_span!("AddIndirectRoot", ?path)
            }
            Request::FindRoots => debug_span!("FindRoots"),
            Request::SetOptions(_options) => debug_span!("SetOptions"),
            Request::CollectGarbage(req) => {
                debug_span!("CollectGarbage",
                    action=?req.action,
                    paths_to_delete=req.paths_to_delete.len(),
                    ignore_liveness=req.ignore_liveness,
                    max_freed=req.max_freed)
            }
            Request::QueryAllValidPaths => debug_span!("QueryAllValidPaths"),
            Request::QueryPathInfo(path) => debug_span!("QueryPathInfo", ?path),
            Request::QueryPathFromHashPart(hash) => debug_span!("QueryPathFromHashPart", ?hash),
            Request::QueryValidPaths(req) => {
                debug_span!(
                    "QueryValidPaths",
                    paths = req.paths.len(),
                    substitute = req.substitute
                )
            }
            Request::QuerySubstitutablePaths(paths) => {
                debug_span!("QuerySubstitutablePaths", paths = paths.len())
            }
            Request::QueryValidDerivers(path) => debug_span!("QueryValidDerivers", ?path),
            Request::OptimiseStore => debug_span!("OptimiseStore"),
            Request::VerifyStore(req) => {
                debug_span!(
                    "VerifyStore",
                    check_contents = req.check_contents,
                    repair = req.repair
                )
            }
            Request::BuildDerivation(req) => {
                debug_span!("BuildDerivation",
                    drv_path=?req.drv.0,
                    drv_name=?req.drv.1.name,
                    mode=?req.mode)
            }
            Request::AddSignatures(req) => {
                debug_span!("AddSignatures", path=?req.path, signatures=?req.signatures)
            }
            Request::NarFromPath(path) => debug_span!("NarFromPath", ?path),
            Request::AddToStoreNar(req) => {
                let path = &req.path_info.path;
                let info = &req.path_info.info;
                debug_span!(
                    "AddToStoreNar",
                    ?path,
                    ?info,
                    repair = req.repair,
                    dont_check_sigs = req.dont_check_sigs
                )
            }
            Request::QueryMissing(paths) => debug_span!("QueryMissing", paths = paths.len()),
            Request::QueryDerivationOutputMap(path) => {
                debug_span!("QueryDerivationOutputMap", ?path)
            }
            Request::RegisterDrvOutput(realisation) => {
                debug_span!("RegisterDrvOutput", ?realisation)
            }
            Request::QueryRealisation(drv_output) => {
                debug_span!("QueryRealisation", ?drv_output)
            }
            Request::AddMultipleToStore(req) => {
                debug_span!(
                    "AddMultipleToStore",
                    repair = req.repair,
                    dont_check_sigs = req.dont_check_sigs
                )
            }
            Request::AddBuildLog(path) => debug_span!("AddBuildLog", ?path),
            Request::BuildPathsWithResults(req) => {
                debug_span!("BuildPathsWithResults", paths=?req.paths.len(), mode=?req.mode)
            }
            Request::AddPermRoot(req) => {
                let gc_root = String::from_utf8_lossy(&req.gc_root);
                debug_span!("AddPermRoot", path=?req.store_path, ?gc_root)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct UnkeyedSubstitutablePathInfo {
    pub deriver: Option<StorePath>,
    pub references: StorePathSet,
    pub download_size: u64,
    pub nar_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubstitutablePathInfo {
    pub path: StorePath,
    pub info: UnkeyedSubstitutablePathInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct AddToStoreRequest {
    pub name: String,
    pub cam: ContentAddressMethodAlgorithm,
    pub refs: StorePathSet,
    pub repair: bool,
    // Framed NAR dump
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct BuildPathsRequest {
    pub paths: Vec<DerivedPath>,
    pub mode: BuildMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, NixDeserialize, NixSerialize)]
pub struct CollectGarbageRequest {
    pub action: GCAction,
    pub paths_to_delete: StorePathSet,
    pub ignore_liveness: bool,
    pub max_freed: u64,
    _removed1: IgnoredZero,
    _removed2: IgnoredZero,
    _removed3: IgnoredZero,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, NixDeserialize, NixSerialize)]
pub struct CollectGarbageResponse {
    pub paths_deleted: Vec<DaemonString>,
    pub bytes_freed: u64,
    _obsolete: IgnoredZero,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct QueryValidPathsRequest {
    pub paths: StorePathSet,
    pub substitute: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct VerifyStoreRequest {
    pub check_contents: bool,
    pub repair: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct BuildDerivationRequest {
    pub drv: (StorePath, BasicDerivation),
    pub mode: BuildMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct AddSignaturesRequest {
    pub path: StorePath,
    pub signatures: Vec<Signature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct AddToStoreNarRequest {
    pub path_info: ValidPathInfo,
    pub repair: bool,
    pub dont_check_sigs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct QueryMissingResult {
    pub will_build: StorePathSet,
    pub will_substitute: StorePathSet,
    pub unknown: StorePathSet,
    pub download_size: u64,
    pub nar_size: u64,
}

pub type QueryRealisationResponse = Vec<Realisation>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct AddMultipleToStoreRequest {
    pub repair: bool,
    pub dont_check_sigs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct AddPermRootRequest {
    pub store_path: StorePath,
    pub gc_root: DaemonPath,
}

macro_rules! optional_info {
    ($sub:ty) => {
        impl NixDeserializeTrait for Option<$sub> {
            async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
            where
                R: ?Sized + NixRead + Send,
            {
                if let Some(found) = reader.try_read_value::<bool>().await? {
                    if found {
                        Ok(Some(Some(reader.read_value().await?)))
                    } else {
                        Ok(Some(None))
                    }
                } else {
                    Ok(None)
                }
            }
        }
        impl NixSerializeTrait for Option<$sub> {
            async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
            where
                W: NixWrite,
            {
                if let Some(value) = self.as_ref() {
                    writer.write_value(&true).await?;
                    writer.write_value(value).await
                } else {
                    writer.write_value(&false).await
                }
            }
        }
    };
}
optional_info!(UnkeyedSubstitutablePathInfo);
optional_info!(UnkeyedValidPathInfo);

macro_rules! optional_from_str {
    ($sub:ty) => {
        impl NixDeserializeTrait for Option<$sub> {
            async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
            where
                R: ?Sized + NixRead + Send,
            {
                if let Some(buf) = reader.try_read_bytes().await? {
                    let s = from_utf8(&buf).map_err(R::Error::invalid_data)?;
                    if s == "" {
                        Ok(Some(None))
                    } else {
                        Ok(Some(Some(s.parse().map_err(R::Error::invalid_data)?)))
                    }
                } else {
                    Ok(None)
                }
            }
        }
        impl NixSerializeTrait for Option<$sub> {
            async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
            where
                W: NixWrite,
            {
                if let Some(value) = self.as_ref() {
                    writer.write_value(value).await
                } else {
                    writer.write_slice(b"").await
                }
            }
        }
    };
}
optional_from_str!(String);
optional_from_str!(ContentAddress);
optional_from_str!(ContentAddressMethodAlgorithm);

impl NixDeserializeTrait for Option<Microseconds> {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        if let Some(tag) = reader.try_read_value::<u8>().await? {
            match tag {
                0 => Ok(Some(None)),
                1 => Ok(Some(Some(reader.read_value::<Microseconds>().await?))),
                _ => Err(R::Error::invalid_data("invalid optional tag from remote")),
            }
        } else {
            Ok(None)
        }
    }
}

impl NixSerializeTrait for Option<Microseconds> {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        if let Some(value) = self.as_ref() {
            writer.write_number(1).await?;
            writer.write_value(value).await
        } else {
            writer.write_number(0).await
        }
    }
}

#[cfg(test)]
pub mod arbitrary {
    use super::*;
    use ::proptest::prelude::*;

    impl Arbitrary for BuildMode {
        type Parameters = ();
        type Strategy = BoxedStrategy<BuildMode>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            use BuildMode::*;
            prop_oneof![
                50 => Just(Normal),
                5 => Just(Repair),
                5 => Just(Check),
            ]
            .boxed()
        }
    }

    impl Arbitrary for Microseconds {
        type Parameters = ();
        type Strategy = BoxedStrategy<Microseconds>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_microseconds().boxed()
        }
    }
    prop_compose! {
        fn arb_microseconds()(ms in 0i64..i64::MAX) -> Microseconds {
            Microseconds(ms)
        }
    }
}
