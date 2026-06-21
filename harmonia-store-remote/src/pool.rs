// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT

//! Connection pool for Nix daemon clients.
//!
//! Capacity is bounded by a [`tokio::sync::Semaphore`]: a connection slot
//! is a permit, so acquiring is cancellation-safe and a dropped acquire
//! future can never leak a slot. The Nix daemon protocol is stateful, so
//! a connection whose in-flight operation is cancelled would be left
//! mid-frame; such connections are dropped instead of returned to the
//! pool, where the next user would desync and hang. Use [`PooledConnectionGuard::execute`]
//! for operations that must survive cancellation.
//!
//! # Example
//!
//! ```ignore
//! use harmonia_store_remote::pool::{ConnectionPool, PoolConfig};
//!
//! let pool = ConnectionPool::new("/nix/var/nix/daemon-socket/socket", PoolConfig::default());
//! let mut guard = pool.acquire().await?;
//! let result = guard.execute(|c| c.query_path_info(&path)).await?;
//! // Connection returned to the pool when the guard is dropped, unless
//! // an operation was interrupted, in which case it is discarded.
//! ```

use crate::metrics::PoolMetrics;
use crate::{DaemonClient, DaemonClientBuilder};
use harmonia_protocol::types::{DaemonError, DaemonErrorKind, DaemonResult, HandshakeDaemonStore};
use harmonia_store_path::StoreDir;
use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, trace, warn};

/// Create a timeout error.
fn timeout_error(context: &str) -> DaemonError {
    DaemonError::from(DaemonErrorKind::Custom(format!("timeout: {context}")))
}

/// Configuration for the connection pool.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Maximum number of connections in the pool
    pub max_size: usize,
    /// Maximum time a connection can be idle before being closed
    pub max_idle_time: Duration,
    /// Timeout for establishing a new connection
    pub connection_timeout: Duration,
    /// Optional metrics for monitoring
    pub metrics: Option<Arc<PoolMetrics>>,
}

impl Default for PoolConfig {
    fn default() -> Self {
        // Default to number of CPU cores + 1 for some headroom
        let max_size = std::thread::available_parallelism()
            .map(|n| n.get() + 1)
            .unwrap_or(5);

        Self {
            max_size,
            max_idle_time: Duration::from_secs(300), // 5 minutes
            connection_timeout: Duration::from_secs(10),
            metrics: None,
        }
    }
}

/// Type alias for the daemon client with Unix socket transport
type UnixDaemonClient = DaemonClient<OwnedReadHalf, OwnedWriteHalf>;

/// Internal wrapper for pooled connections
struct PooledConnection {
    client: UnixDaemonClient,
    last_used: Instant,
}

impl PooledConnection {
    fn is_expired(&self, max_idle_time: Duration) -> bool {
        self.last_used.elapsed() > max_idle_time
    }
}

/// A connection pool for Nix daemon clients.
#[derive(Clone)]
pub struct ConnectionPool {
    /// One permit per connection slot.
    slots: Arc<Semaphore>,
    /// Idle connections available for reuse. A std mutex suffices: it is
    /// only held to push or pop, never across an await, so Drop can
    /// return a connection synchronously.
    idle: Arc<Mutex<VecDeque<PooledConnection>>>,
    socket_path: PathBuf,
    store_dir: StoreDir,
    config: PoolConfig,
}

impl ConnectionPool {
    /// Create a new connection pool.
    ///
    /// # Panics
    /// Panics if `config.max_size` is 0.
    pub fn new<P: AsRef<Path>>(socket_path: P, config: PoolConfig) -> Self {
        assert!(config.max_size > 0, "Pool capacity must be positive");

        Self {
            slots: Arc::new(Semaphore::new(config.max_size)),
            idle: Arc::new(Mutex::new(VecDeque::new())),
            socket_path: socket_path.as_ref().to_path_buf(),
            store_dir: StoreDir::default(),
            config,
        }
    }

    /// Create a new connection pool with a custom store directory.
    pub fn with_store_dir<P: AsRef<Path>>(
        socket_path: P,
        store_dir: StoreDir,
        config: PoolConfig,
    ) -> Self {
        let mut pool = Self::new(socket_path, config);
        pool.store_dir = store_dir;
        pool
    }

    /// Get the store directory for this pool.
    pub fn store_dir(&self) -> &StoreDir {
        &self.store_dir
    }

    /// Acquire a connection from the pool, returning an RAII guard that
    /// releases it on drop.
    ///
    /// Waits for a free slot indefinitely; a caller that needs a
    /// deadline wraps this in its own timeout (cancellation frees the
    /// slot at once).
    pub async fn acquire(&self) -> DaemonResult<PooledConnectionGuard> {
        let start = Instant::now();

        // Hold a slot for the connection's lifetime.
        let permit = Arc::clone(&self.slots)
            .acquire_owned()
            .await
            // The semaphore is never closed, but handle it rather than panic.
            .map_err(|_| {
                DaemonError::from(DaemonErrorKind::Custom("connection pool closed".into()))
            })?;

        // Reuse a live idle connection, discarding any that idled out.
        let reused = {
            let max_idle = self.config.max_idle_time;
            let mut idle = self.idle.lock().unwrap();
            loop {
                match idle.pop_front() {
                    Some(conn) if conn.is_expired(max_idle) => continue,
                    other => break other,
                }
            }
        };

        let (conn, kind) = match reused {
            Some(mut conn) => {
                conn.last_used = Instant::now();
                trace!("Reusing idle connection");
                (conn, "reused")
            }
            None => {
                // On error the permit drops here, releasing the slot.
                let conn = self.create_connection().await.inspect_err(|e| {
                    if let Some(metrics) = &self.config.metrics {
                        metrics
                            .total_connections_created
                            .with_label_values(&["error"])
                            .inc();
                        metrics
                            .connection_errors
                            .with_label_values(&["creation_failed"])
                            .inc();
                    }
                    warn!("Failed to create connection: {e}");
                })?;
                if let Some(metrics) = &self.config.metrics {
                    metrics
                        .total_connections_created
                        .with_label_values(&["success"])
                        .inc();
                }
                debug!("Created new connection");
                (conn, "created")
            }
        };

        if let Some(metrics) = &self.config.metrics {
            metrics
                .connection_acquire_duration
                .with_label_values(&[kind])
                .observe(start.elapsed().as_secs_f64());
            self.update_metrics(metrics);
        }

        Ok(PooledConnectionGuard {
            conn: Some(conn),
            idle: Arc::clone(&self.idle),
            metrics: self.config.metrics.clone(),
            dirty: false,
            _permit: permit,
        })
    }

    /// Create a new connection to the daemon.
    async fn create_connection(&self) -> DaemonResult<PooledConnection> {
        let connect_fut = async {
            debug!(
                "Connecting to daemon socket at {}",
                self.socket_path.display(),
            );
            let handshake_client = DaemonClientBuilder::new()
                .set_store_dir(&self.store_dir)
                .build_unix(&self.socket_path)
                .await?;

            // Perform handshake (ResultLog is both Stream and Future, so we can just .await it)
            let client = handshake_client.handshake().await?;
            Ok::<_, DaemonError>(client)
        };

        let client = tokio::time::timeout(self.config.connection_timeout, connect_fut)
            .await
            .map_err(|_| timeout_error("connecting to daemon"))??;

        Ok(PooledConnection {
            client,
            last_used: Instant::now(),
        })
    }

    fn update_metrics(&self, metrics: &PoolMetrics) {
        let idle = self.idle.lock().unwrap().len();
        // Held permits are the guards in use; idle connections hold none.
        let active = self.config.max_size - self.slots.available_permits();
        metrics.idle_connections.set(idle as i64);
        metrics.active_connections.set(active as i64);
    }

    /// Get current pool statistics.
    ///
    /// Returns (idle_count, active_count, capacity).
    pub async fn stats(&self) -> (usize, usize, usize) {
        let idle = self.idle.lock().unwrap().len();
        let capacity = self.config.max_size;
        // Held permits are the guards in use; idle connections hold none.
        let active = capacity - self.slots.available_permits();
        (idle, active, capacity)
    }
}

/// RAII guard that returns its connection to the pool when dropped,
/// unless the connection was poisoned (see [`Self::execute`]) or marked
/// broken via [`Self::mark_broken`], in which case it is discarded.
pub struct PooledConnectionGuard {
    conn: Option<PooledConnection>,
    idle: Arc<Mutex<VecDeque<PooledConnection>>>,
    metrics: Option<Arc<PoolMetrics>>,
    /// Set while an operation is in flight; poisons the connection if the
    /// guard drops while set.
    dirty: bool,
    /// Releases the connection slot when the guard drops.
    _permit: OwnedSemaphorePermit,
}

impl PooledConnectionGuard {
    /// Run a daemon operation, discarding the connection unless it
    /// completes successfully. The daemon protocol is a request/response
    /// stream on one socket, so an operation that is cancelled or errors
    /// mid-exchange leaves the connection in an unknown state that would
    /// desync the next user; only a clean `Ok` returns it to the pool.
    ///
    /// This is the only way to reach the client, so an operation cannot
    /// be run uncancellably. A multi-step exchange that must stay atomic
    /// goes in a single closure.
    pub async fn execute<'a, T, Fut>(
        &'a mut self,
        op: impl FnOnce(&'a mut UnixDaemonClient) -> Fut,
    ) -> DaemonResult<T>
    where
        Fut: Future<Output = DaemonResult<T>> + 'a,
    {
        self.dirty = true;
        // Borrow conn alone (not all of self) so dirty stays assignable.
        let client = &mut self
            .conn
            .as_mut()
            .expect("connection guard used after mark_broken()")
            .client;
        let out = op(client).await;
        // Leave the connection poisoned on error: its protocol state is
        // unknown, so Drop discards it rather than recirculating it.
        if out.is_ok() {
            self.dirty = false;
        }
        out
    }

    /// Mark the connection as broken so it is not returned to the pool.
    /// Call this when an operation fails with a connection-level error.
    pub fn mark_broken(mut self) {
        self.conn = None;
        if let Some(metrics) = &self.metrics {
            metrics
                .connection_errors
                .with_label_values(&["broken"])
                .inc();
        }
    }
}

impl Drop for PooledConnectionGuard {
    fn drop(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            if self.dirty {
                // Interrupted mid-operation: the connection may have an
                // unfinished request on the wire, so discard it.
                if let Some(metrics) = &self.metrics {
                    metrics
                        .connection_errors
                        .with_label_values(&["interrupted"])
                        .inc();
                }
                trace!("Discarding connection interrupted mid-operation");
            } else {
                conn.last_used = Instant::now();
                self.idle.lock().unwrap().push_back(conn);
            }
        }
        if let Some(metrics) = &self.metrics {
            let idle = self.idle.lock().unwrap().len() as i64;
            metrics.idle_connections.set(idle);
        }
        // _permit is released when this guard finishes dropping.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_default() {
        let config = PoolConfig::default();
        assert!(config.max_size > 0);
        assert!(config.max_idle_time > Duration::ZERO);
        assert!(config.connection_timeout > Duration::ZERO);
    }

    #[test]
    #[should_panic(expected = "Pool capacity must be positive")]
    fn test_pool_zero_capacity() {
        let config = PoolConfig {
            max_size: 0,
            ..Default::default()
        };
        let _ = ConnectionPool::new("/tmp/test.sock", config);
    }

    /// A pool pointed at a non-existent socket should fail to create a
    /// connection but must release the slot, so capacity is preserved
    /// across repeated (including cancelled) acquire attempts rather than
    /// draining to zero.
    #[tokio::test]
    async fn slot_released_on_failed_and_cancelled_acquire() {
        let config = PoolConfig {
            max_size: 2,
            connection_timeout: Duration::from_millis(50),
            ..Default::default()
        };
        let pool = ConnectionPool::new("/nonexistent/socket", config);

        // Failed creations must not consume slots permanently.
        for _ in 0..5 {
            assert!(pool.acquire().await.is_err());
        }
        assert_eq!(pool.slots.available_permits(), 2);

        // A cancelled acquire (future dropped) must also free its slot.
        for _ in 0..5 {
            let fut = pool.acquire();
            tokio::pin!(fut);
            // Poll once then drop without awaiting to completion.
            let _ = tokio::time::timeout(Duration::from_millis(1), &mut fut).await;
        }
        assert_eq!(pool.slots.available_permits(), 2);
    }
}
