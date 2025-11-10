pub mod client;
pub mod error;
pub mod protocol;
pub mod serialization;
pub mod server;

#[cfg(test)]
mod tests;

pub use client::{DaemonClient, pool::PoolConfig};
pub use error::ProtocolError;
pub use protocol::{CURRENT_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION, ProtocolVersion};
pub use server::{DaemonServer, RequestHandler};
