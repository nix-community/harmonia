pub mod compound;
pub mod primitives;

use crate::error::ProtocolError;
use crate::protocol::ProtocolVersion;
use harmonia_store_core::store_path::StoreDir;
use tokio::io::{AsyncRead, AsyncWrite};

#[allow(async_fn_in_trait)]
pub trait Serialize {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
        store_dir: &StoreDir,
    ) -> Result<(), ProtocolError>;
}

#[allow(async_fn_in_trait)]
pub trait Deserialize: Sized {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
        store_dir: &StoreDir,
    ) -> Result<Self, ProtocolError>;
}
