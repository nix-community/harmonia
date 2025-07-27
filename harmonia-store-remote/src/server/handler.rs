use crate::error::ProtocolError;
use crate::protocol::types::{
    DerivedPath, DrvOutputId, Missing, Realisation, SubstitutablePathInfo, SubstitutablePathInfos,
};
use crate::protocol::{StorePath, ValidPathInfo};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

pub trait RequestHandler: Send + Sync {
    // Basic query operations
    fn handle_query_path_info(
        &self,
        _path: StorePath,
    ) -> impl Future<Output = Result<Option<ValidPathInfo>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_path_info")) }
    }

    fn handle_query_path_from_hash_part(
        &self,
        _hash: &[u8],
    ) -> impl Future<Output = Result<Option<StorePath>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_path_from_hash_part")) }
    }

    fn handle_is_valid_path(
        &self,
        _path: StorePath,
    ) -> impl Future<Output = Result<bool, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("is_valid_path")) }
    }

    // Batch query operations
    fn handle_query_all_valid_paths(
        &self,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_all_valid_paths")) }
    }

    fn handle_query_valid_paths(
        &self,
        _paths: BTreeSet<StorePath>,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_valid_paths")) }
    }

    // Substitution queries
    fn handle_query_substitutable_paths(
        &self,
        _paths: BTreeSet<StorePath>,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_substitutable_paths")) }
    }

    fn handle_has_substitutes(
        &self,
        _path: StorePath,
    ) -> impl Future<Output = Result<bool, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("has_substitutes")) }
    }

    fn handle_query_substitutable_path_info(
        &self,
        _path: StorePath,
    ) -> impl Future<Output = Result<Option<SubstitutablePathInfo>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_substitutable_path_info")) }
    }

    fn handle_query_substitutable_path_infos(
        &self,
        _paths: BTreeSet<StorePath>,
    ) -> impl Future<Output = Result<SubstitutablePathInfos, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_substitutable_path_infos")) }
    }

    // Reference queries
    fn handle_query_referrers(
        &self,
        _path: StorePath,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_referrers")) }
    }

    // Derivation queries
    fn handle_query_valid_derivers(
        &self,
        _path: StorePath,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_valid_derivers")) }
    }

    fn handle_query_derivation_outputs(
        &self,
        _drv_path: StorePath,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_derivation_outputs")) }
    }

    fn handle_query_derivation_output_names(
        &self,
        _drv_path: StorePath,
    ) -> impl Future<Output = Result<Vec<Vec<u8>>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_derivation_output_names")) }
    }

    fn handle_query_derivation_output_map(
        &self,
        _drv_path: StorePath,
    ) -> impl Future<Output = Result<BTreeMap<String, Option<StorePath>>, ProtocolError>> + Send
    {
        async { Err(ProtocolError::Unsupported("query_derivation_output_map")) }
    }

    // Missing/dependency analysis
    fn handle_query_missing(
        &self,
        _targets: Vec<DerivedPath>,
    ) -> impl Future<Output = Result<Missing, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_missing")) }
    }

    // Content-addressed store operations
    fn handle_query_realisation(
        &self,
        _id: DrvOutputId,
    ) -> impl Future<Output = Result<Option<Realisation>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_realisation")) }
    }

    fn handle_query_failed_paths(
        &self,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("query_failed_paths")) }
    }

    fn handle_clear_failed_paths(
        &self,
        _paths: BTreeSet<StorePath>,
    ) -> impl Future<Output = Result<(), ProtocolError>> + Send {
        async { Err(ProtocolError::Unsupported("clear_failed_paths")) }
    }
}
