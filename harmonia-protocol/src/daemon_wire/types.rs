use derive_more::Display;
use harmonia_protocol_derive::{NixDeserialize, NixSerialize};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::de::{NixDeserialize as NixDeserializeTrait, NixRead};
use crate::ser::{NixSerialize as NixSerializeTrait, NixWrite};
use crate::version::ProtocolRange;
use harmonia_store_core::store_path::StorePath;

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
    Display,
    NixDeserialize,
    NixSerialize,
)]
#[nix(try_from = "u64", into = "u64")]
#[repr(u64)]
pub enum Operation {
    IsValidPath = 1,
    QueryReferrers = 6,
    AddToStore = 7,
    BuildPaths = 9,
    EnsurePath = 10,
    AddTempRoot = 11,
    AddIndirectRoot = 12,
    FindRoots = 14,
    SetOptions = 19,
    CollectGarbage = 20,
    QueryAllValidPaths = 23,
    QueryPathInfo = 26,
    QueryPathFromHashPart = 29,
    QueryValidPaths = 31,
    QuerySubstitutablePaths = 32,
    QueryValidDerivers = 33,
    OptimiseStore = 34,
    VerifyStore = 35,
    BuildDerivation = 36,
    AddSignatures = 37,
    NarFromPath = 38,
    AddToStoreNar = 39,
    QueryMissing = 40,
    QueryDerivationOutputMap = 41,
    RegisterDrvOutput = 42,
    QueryRealisation = 43,
    AddMultipleToStore = 44,
    AddBuildLog = 45,
    BuildPathsWithResults = 46,
    AddPermRoot = 47,

    /// Obsolete Nix 2.5.0 Protocol 1.32
    SyncWithGC = 13,
    /// Obsolete Nix 2.4 Protocol 1.25
    AddTextToStore = 8,
    /// Obsolete Nix 2.4 Protocol 1.22*
    QueryDerivationOutputs = 22,
    /// Obsolete Nix 2.4 Protocol 1.21
    QueryDerivationOutputNames = 28,
    /// Obsolete Nix 2.0, Protocol 1.19*
    QuerySubstitutablePathInfos = 30,
    /// Obsolete Nix 2.0 Protocol 1.17
    ExportPath = 16,
    /// Obsolete Nix 2.0 Protocol 1.17
    ImportPaths = 27,
    /// Obsolete Nix 2.0 Protocol 1.16
    QueryPathHash = 4,
    /// Obsolete Nix 2.0 Protocol 1.16
    QueryReferences = 5,
    /// Obsolete Nix 2.0 Protocol 1.16
    QueryDeriver = 18,
    /// Obsolete Nix 1.2 Protocol 1.12
    HasSubstitutes = 3,
    /// Obsolete Nix 1.2 Protocol 1.12
    QuerySubstitutablePathInfo = 21,
    // Removed Nix 2.0 Protocol 1.16
    // QueryFailedPaths = 24,
    // Removed Nix 2.0 Protocol 1.16
    // ClearFailedPaths = 25,
    // Removed Nix 1.0 Protocol 1.09
    // ImportPath = 17,
    // Became dead code in Nix 0.11 and removed in Nix 1.8
    // Quit = 0,
    // Removed Nix 0.12 Protocol 1.02
    // RemovedCollectGarbage = 15,
}

impl Operation {
    pub fn versions(&self) -> ProtocolRange {
        match self {
            Operation::IsValidPath => (..).into(),
            Operation::HasSubstitutes => (..12).into(),
            Operation::QueryPathHash => (..16).into(),
            Operation::QueryReferences => (..16).into(),
            Operation::QueryReferrers => (..).into(),
            Operation::AddToStore => (..).into(),
            Operation::AddTextToStore => (..25).into(),
            Operation::BuildPaths => (..).into(),
            Operation::EnsurePath => (..).into(),
            Operation::AddTempRoot => (..).into(),
            Operation::AddIndirectRoot => (..).into(),
            Operation::SyncWithGC => (..32).into(),
            Operation::FindRoots => (..).into(),
            Operation::ExportPath => (..17).into(),
            Operation::QueryDeriver => (..16).into(),
            Operation::SetOptions => (..).into(),
            Operation::CollectGarbage => (2..).into(),
            Operation::QuerySubstitutablePathInfo => (2..12).into(),
            Operation::QueryDerivationOutputs => (5..22).into(),
            Operation::QueryAllValidPaths => (5..).into(),
            Operation::QueryPathInfo => (6..).into(),
            Operation::ImportPaths => (9..17).into(),
            Operation::QueryDerivationOutputNames => (8..21).into(),
            Operation::QueryPathFromHashPart => (11..).into(),
            Operation::QuerySubstitutablePathInfos => (12..19).into(),
            Operation::QueryValidPaths => (12..).into(),
            Operation::QuerySubstitutablePaths => (12..).into(),
            Operation::QueryValidDerivers => (13..).into(),
            Operation::OptimiseStore => (14..).into(),
            Operation::VerifyStore => (14..).into(),
            Operation::BuildDerivation => (14..).into(),
            Operation::AddSignatures => (16..).into(),
            Operation::NarFromPath => (17..).into(),
            Operation::AddToStoreNar => (17..).into(),
            Operation::QueryMissing => (19..).into(),
            Operation::QueryDerivationOutputMap => (22..).into(),
            Operation::RegisterDrvOutput => (27..).into(),
            Operation::QueryRealisation => (27..).into(),
            Operation::AddMultipleToStore => (32..).into(),
            Operation::AddBuildLog => (32..).into(),
            Operation::BuildPathsWithResults => (34..).into(),
            Operation::AddPermRoot => (36..).into(),
        }
    }
}

macro_rules! optional_from_store_dir_str {
    ($sub:ty) => {
        impl NixDeserializeTrait for Option<$sub> {
            async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
            where
                R: ?Sized + NixRead + Send,
            {
                use crate::de::Error;
                use harmonia_store_core::store_path::FromStoreDirStr;
                if let Some(buf) = reader.try_read_bytes().await? {
                    let s = ::std::str::from_utf8(&buf).map_err(Error::invalid_data)?;
                    if s == "" {
                        Ok(Some(None))
                    } else {
                        let dir = reader.store_dir();
                        <$sub as FromStoreDirStr>::from_store_dir_str(dir, s)
                            .map_err(Error::invalid_data)
                            .map(|v| Some(Some(v)))
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
optional_from_store_dir_str!(StorePath);
