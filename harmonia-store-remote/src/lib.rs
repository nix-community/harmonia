pub mod client;
pub mod error;
pub mod operations;
pub mod protocol;
pub mod serialization;
pub mod server;

#[cfg(test)]
mod tests;

pub use client::{pool::PoolConfig, DaemonClient};
pub use error::ProtocolError;
pub use protocol::{ProtocolVersion, CURRENT_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION};
pub use server::{DaemonServer, RequestHandler};
