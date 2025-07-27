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
        path: StorePath,
    ) -> impl Future<Output = Result<Option<ValidPathInfo>, ProtocolError>> + Send;

    fn handle_query_path_from_hash_part(
        &self,
        hash: &[u8],
    ) -> impl Future<Output = Result<Option<StorePath>, ProtocolError>> + Send;

    fn handle_is_valid_path(
        &self,
        path: StorePath,
    ) -> impl Future<Output = Result<bool, ProtocolError>> + Send;

    // Batch query operations
    fn handle_query_all_valid_paths(
        &self,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send;

    fn handle_query_valid_paths(
        &self,
        paths: BTreeSet<StorePath>,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send;

    // Substitution queries
    fn handle_query_substitutable_paths(
        &self,
        paths: BTreeSet<StorePath>,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send;

    fn handle_has_substitutes(
        &self,
        path: StorePath,
    ) -> impl Future<Output = Result<bool, ProtocolError>> + Send;

    fn handle_query_substitutable_path_info(
        &self,
        path: StorePath,
    ) -> impl Future<Output = Result<Option<SubstitutablePathInfo>, ProtocolError>> + Send;

    fn handle_query_substitutable_path_infos(
        &self,
        paths: BTreeSet<StorePath>,
    ) -> impl Future<Output = Result<SubstitutablePathInfos, ProtocolError>> + Send;

    // Reference queries
    fn handle_query_referrers(
        &self,
        path: StorePath,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send;

    // Derivation queries
    fn handle_query_valid_derivers(
        &self,
        path: StorePath,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send;

    fn handle_query_derivation_outputs(
        &self,
        drv_path: StorePath,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send;

    fn handle_query_derivation_output_names(
        &self,
        drv_path: StorePath,
    ) -> impl Future<Output = Result<Vec<Vec<u8>>, ProtocolError>> + Send;

    fn handle_query_derivation_output_map(
        &self,
        drv_path: StorePath,
    ) -> impl Future<Output = Result<BTreeMap<String, Option<StorePath>>, ProtocolError>> + Send;

    // Missing/dependency analysis
    fn handle_query_missing(
        &self,
        targets: Vec<DerivedPath>,
    ) -> impl Future<Output = Result<Missing, ProtocolError>> + Send;

    // Content-addressed store operations
    fn handle_query_realisation(
        &self,
        id: DrvOutputId,
    ) -> impl Future<Output = Result<Option<Realisation>, ProtocolError>> + Send;

    fn handle_query_failed_paths(
        &self,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, ProtocolError>> + Send;

    fn handle_clear_failed_paths(
        &self,
        paths: BTreeSet<StorePath>,
    ) -> impl Future<Output = Result<(), ProtocolError>> + Send;
}
