use crate::error::ProtocolError;

#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    IsValidPath = 1,
    HasSubstitutes = 3,
    QueryPathHash = 4,   // obsolete
    QueryReferences = 5, // obsolete
    QueryReferrers = 6,
    AddToStore = 7,
    AddTextToStore = 8, // obsolete since 1.25, Nix 3.0. Use WorkerProto::Op::AddToStore
    BuildPaths = 9,
    EnsurePath = 10,
    AddTempRoot = 11,
    AddIndirectRoot = 12,
    SyncWithGC = 13,
    FindRoots = 14,
    ExportPath = 16,   // obsolete
    QueryDeriver = 18, // obsolete
    SetOptions = 19,
    CollectGarbage = 20,
    QuerySubstitutablePathInfo = 21,
    QueryDerivationOutputs = 22, // obsolete
    QueryAllValidPaths = 23,
    QueryFailedPaths = 24,
    ClearFailedPaths = 25,
    QueryPathInfo = 26,
    ImportPaths = 27,                // obsolete
    QueryDerivationOutputNames = 28, // obsolete
    QueryPathFromHashPart = 29,
    QuerySubstitutablePathInfos = 30,
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

impl TryFrom<u64> for OpCode {
    type Error = ProtocolError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::IsValidPath),
            3 => Ok(Self::HasSubstitutes),
            4 => Ok(Self::QueryPathHash),
            5 => Ok(Self::QueryReferences),
            6 => Ok(Self::QueryReferrers),
            7 => Ok(Self::AddToStore),
            8 => Ok(Self::AddTextToStore),
            9 => Ok(Self::BuildPaths),
            10 => Ok(Self::EnsurePath),
            11 => Ok(Self::AddTempRoot),
            12 => Ok(Self::AddIndirectRoot),
            13 => Ok(Self::SyncWithGC),
            14 => Ok(Self::FindRoots),
            16 => Ok(Self::ExportPath),
            18 => Ok(Self::QueryDeriver),
            19 => Ok(Self::SetOptions),
            20 => Ok(Self::CollectGarbage),
            21 => Ok(Self::QuerySubstitutablePathInfo),
            22 => Ok(Self::QueryDerivationOutputs),
            23 => Ok(Self::QueryAllValidPaths),
            24 => Ok(Self::QueryFailedPaths),
            25 => Ok(Self::ClearFailedPaths),
            26 => Ok(Self::QueryPathInfo),
            27 => Ok(Self::ImportPaths),
            28 => Ok(Self::QueryDerivationOutputNames),
            29 => Ok(Self::QueryPathFromHashPart),
            30 => Ok(Self::QuerySubstitutablePathInfos),
            31 => Ok(Self::QueryValidPaths),
            32 => Ok(Self::QuerySubstitutablePaths),
            33 => Ok(Self::QueryValidDerivers),
            34 => Ok(Self::OptimiseStore),
            35 => Ok(Self::VerifyStore),
            36 => Ok(Self::BuildDerivation),
            37 => Ok(Self::AddSignatures),
            38 => Ok(Self::NarFromPath),
            39 => Ok(Self::AddToStoreNar),
            40 => Ok(Self::QueryMissing),
            41 => Ok(Self::QueryDerivationOutputMap),
            42 => Ok(Self::RegisterDrvOutput),
            43 => Ok(Self::QueryRealisation),
            44 => Ok(Self::AddMultipleToStore),
            45 => Ok(Self::AddBuildLog),
            46 => Ok(Self::BuildPathsWithResults),
            47 => Ok(Self::AddPermRoot),
            _ => Err(ProtocolError::InvalidOpCode(value)),
        }
    }
}
