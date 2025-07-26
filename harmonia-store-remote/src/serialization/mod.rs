pub mod compound;
pub mod gc;
pub mod primitives;
pub mod store_requests;
pub mod store_types;

use crate::error::ProtocolError;
use crate::protocol::ProtocolVersion;
use tokio::io::{AsyncRead, AsyncWrite};

#[allow(async_fn_in_trait)]
pub trait Serialize {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError>;
}

#[allow(async_fn_in_trait)]
pub trait Deserialize: Sized {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError>;
}
