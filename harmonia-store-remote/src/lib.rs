// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Nix daemon client library for remote store access.
//!
//! This crate provides a client implementation for connecting to the Nix daemon
//! and performing store operations over the daemon protocol.
//!
//! **Architecture**: This is the Remote Store Layer in Harmonia's store architecture.
//! See `docs/architecture/harmonia-store-structure.md` for details.
//!
//! # Basic Example
//!
//! ```ignore
//! use harmonia_store_remote::DaemonClientBuilder;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Connect to the default daemon socket
//!     let client = DaemonClientBuilder::new()
//!         .build_daemon()
//!         .await?
//!         .handshake()
//!         .await?;
//!
//!     // Check if a path is valid
//!     let is_valid = client.is_valid_path(&path).await?;
//!     Ok(())
//! }
//! ```
//!
//! # Connection Pool Example
//!
//! For applications that need to make many concurrent requests, use the connection pool:
//!
//! ```ignore
//! use harmonia_store_remote::pool::{ConnectionPool, PoolConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let pool = ConnectionPool::new(
//!         "/nix/var/nix/daemon-socket/socket",
//!         PoolConfig::default(),
//!     );
//!
//!     // Acquire a connection from the pool
//!     let mut guard = pool.acquire().await?;
//!     let is_valid = guard.client().is_valid_path(&path).await?;
//!     // Connection automatically returned when guard is dropped
//!     Ok(())
//! }
//! ```

mod client;
pub mod metrics;
pub mod pool;

pub use client::{DaemonClient, DaemonClientBuilder, DaemonHandshakeClient};
pub use metrics::PoolMetrics;
pub use pool::{ConnectionPool, PoolConfig, PooledConnectionGuard};

// Re-export commonly used types from harmonia-protocol
pub use harmonia_protocol::ProtocolVersion;
pub use harmonia_protocol::types::{
    DaemonError, DaemonErrorKind, DaemonResult, DaemonStore, HandshakeDaemonStore, TrustLevel,
};
pub use harmonia_protocol::valid_path_info::UnkeyedValidPathInfo;
