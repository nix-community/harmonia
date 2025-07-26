use harmonia_store_core::{FileIngestionMethod, HashAlgo, NarSignature, StorePath};
use std::collections::BTreeSet;

/// Request for AddTextToStore operation (OpCode 8)
/// Note: This operation is obsolete since protocol 1.25, use AddToStore instead
#[derive(Debug)]
pub struct AddTextToStoreRequest<'a> {
    pub name: &'a [u8],
    pub content: &'a [u8],
    pub references: &'a BTreeSet<StorePath>,
    pub repair: bool,
}

/// Request for AddSignatures operation (OpCode 37)
#[derive(Debug)]
pub struct AddSignaturesRequest<'a> {
    pub path: &'a StorePath,
    pub signatures: &'a [NarSignature],
}

/// Request for AddToStore operation (OpCode 7)
/// This is the complex streaming operation
#[derive(Debug)]
pub struct AddToStoreRequest<'a> {
    pub name: &'a [u8],
    pub method: FileIngestionMethod,
    pub hash_algo: HashAlgo,
    pub references: &'a BTreeSet<StorePath>,
    pub repair: bool,
}

/// Request for AddToStoreNar operation (OpCode 39)
/// This is for adding a NAR with known metadata
#[derive(Debug)]
pub struct AddToStoreNarRequest<'a> {
    pub info: &'a crate::protocol::ValidPathInfo,
    pub repair: bool,
    pub check_sigs: bool,
}

/// Request for VerifyStore operation (OpCode 35)
#[derive(Debug)]
pub struct VerifyStoreRequest {
    pub check_contents: bool,
    pub repair: bool,
}
