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
}

impl Operation {
    pub fn versions(&self) -> ProtocolRange {
        // All operations are valid for protocol version 37 (the only supported version)
        (..).into()
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
