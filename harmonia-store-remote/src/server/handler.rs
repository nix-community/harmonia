use crate::error::ProtocolError;
use crate::protocol::{StorePath, ValidPathInfo};
use std::future::Future;

pub trait RequestHandler: Send + Sync {
    fn handle_query_path_info(
        &self,
        path: &StorePath,
    ) -> impl Future<Output = Result<Option<ValidPathInfo>, ProtocolError>> + Send;

    fn handle_query_path_from_hash_part(
        &self,
        hash: &[u8],
    ) -> impl Future<Output = Result<Option<StorePath>, ProtocolError>> + Send;

    fn handle_is_valid_path(
        &self,
        path: &StorePath,
    ) -> impl Future<Output = Result<bool, ProtocolError>> + Send;
}
