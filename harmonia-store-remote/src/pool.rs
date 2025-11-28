// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT

//! Connection pool for Nix daemon clients.
//!
//! This module provides a connection pool with formally verified safety properties:
//! - **Invariant**: `active + idle ≤ capacity` (proven in Dafny)
//! - **Resource safety**: Connections are always returned via RAII guards
//! - **Observability**: Full Prometheus metrics integration
//!
//! # Example
//!
//! ```ignore
//! use harmonia_store_remote::pool::{ConnectionPool, PoolConfig};
//!
//! let pool = ConnectionPool::new("/nix/var/nix/daemon-socket/socket", PoolConfig::default());
//! let guard = pool.acquire().await?;
//! let result = guard.client().query_path_info(&path).await?;
//! // Connection automatically returned when guard is dropped
//! ```

use crate::metrics::PoolMetrics;
use crate::{DaemonClient, DaemonClientBuilder};
use harmonia_protocol::types::{DaemonError, DaemonErrorKind, DaemonResult, HandshakeDaemonStore};
use harmonia_store_core::store_path::StoreDir;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, Notify};
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
    /// Timeout for acquiring a connection from the pool
    pub acquire_timeout: Duration,
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
            acquire_timeout: Duration::from_secs(30),
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

/// Pool state with formally verified invariant
struct PoolState {
    /// Idle connections available for reuse
    idle: VecDeque<PooledConnection>,
    /// Count of connections currently in use
    active_count: usize,
    /// Maximum pool capacity
    capacity: usize,
    /// Count of tasks waiting for a connection
    waiting_count: usize,
}

impl PoolState {
    /// Core invariant from Dafny proof: active + idle ≤ capacity
    fn invariant(&self) -> bool {
        self.active_count + self.idle.len() <= self.capacity && self.capacity > 0
    }

    /// Update metrics to reflect current state
    fn update_metrics(&self, metrics: &PoolMetrics) {
        metrics.idle_connections.set(self.idle.len() as i64);
        metrics.active_connections.set(self.active_count as i64);
    }
}

/// Result of an acquire attempt
enum AcquireResult {
    /// Successfully acquired a connection
    Success(PooledConnection),
    /// Must wait for a connection to become available
    WaitRequired,
}

/// A connection pool for Nix daemon clients.
///
/// The pool maintains the invariant `active + idle ≤ capacity` at all times,
/// ensuring bounded resource usage.
#[derive(Clone)]
pub struct ConnectionPool {
    state: Arc<Mutex<PoolState>>,
    socket_path: PathBuf,
    store_dir: StoreDir,
    config: PoolConfig,
    available_notify: Arc<Notify>,
}

impl ConnectionPool {
    /// Create a new connection pool.
    ///
    /// # Arguments
    /// * `socket_path` - Path to the Nix daemon Unix socket
    /// * `config` - Pool configuration
    ///
    /// # Panics
    /// Panics if `config.max_size` is 0.
    pub fn new<P: AsRef<Path>>(socket_path: P, config: PoolConfig) -> Self {
        assert!(config.max_size > 0, "Pool capacity must be positive");

        let state = PoolState {
            idle: VecDeque::new(),
            active_count: 0,
            capacity: config.max_size,
            waiting_count: 0,
        };

        debug_assert!(state.invariant());

        Self {
            state: Arc::new(Mutex::new(state)),
            socket_path: socket_path.as_ref().to_path_buf(),
            store_dir: StoreDir::default(),
            config,
            available_notify: Arc::new(Notify::new()),
        }
    }

    /// Create a new connection pool with a custom store directory.
    pub fn with_store_dir<P: AsRef<Path>>(socket_path: P, store_dir: StoreDir, config: PoolConfig) -> Self {
        let mut pool = Self::new(socket_path, config);
        pool.store_dir = store_dir;
        pool
    }

    /// Acquire a connection from the pool.
    ///
    /// Returns an RAII guard that automatically returns the connection
    /// when dropped.
    pub async fn acquire(&self) -> DaemonResult<PooledConnectionGuard> {
        let start = Instant::now();

        loop {
            let result = self.try_acquire(start).await?;

            match result {
                AcquireResult::Success(conn) => {
                    return Ok(PooledConnectionGuard {
                        conn: Some(conn),
                        pool: Arc::clone(&self.state),
                        metrics: self.config.metrics.clone(),
                        notify: Arc::clone(&self.available_notify),
                    });
                }
                AcquireResult::WaitRequired => {
                    // Wait for notification with timeout
                    match tokio::time::timeout(
                        self.config.acquire_timeout,
                        self.available_notify.notified(),
                    )
                    .await
                    {
                        Ok(_) => {
                            // Decrement waiting count and retry
                            let mut state = self.state.lock().await;
                            state.waiting_count = state.waiting_count.saturating_sub(1);
                            continue;
                        }
                        Err(_) => {
                            // Timeout waiting for connection
                            let mut state = self.state.lock().await;
                            state.waiting_count = state.waiting_count.saturating_sub(1);

                            if let Some(ref metrics) = self.config.metrics {
                                metrics
                                    .connection_errors
                                    .with_label_values(&["timeout"])
                                    .inc();
                                metrics
                                    .connection_acquire_duration
                                    .with_label_values(&["timeout"])
                                    .observe(start.elapsed().as_secs_f64());
                            }

                            return Err(timeout_error("acquiring connection from pool"));
                        }
                    }
                }
            }
        }
    }

    /// Try to acquire a connection without blocking.
    async fn try_acquire(&self, start_time: Instant) -> DaemonResult<AcquireResult> {
        let mut state = self.state.lock().await;
        debug_assert!(state.invariant());

        // Update metrics
        if let Some(ref metrics) = self.config.metrics {
            state.update_metrics(metrics);
        }

        // Remove expired connections
        let max_idle = self.config.max_idle_time;
        state.idle.retain(|conn| !conn.is_expired(max_idle));

        // Try to reuse an idle connection
        if let Some(mut conn) = state.idle.pop_front() {
            state.active_count += 1;
            conn.last_used = Instant::now();
            debug_assert!(state.invariant());

            if let Some(ref metrics) = self.config.metrics {
                state.update_metrics(metrics);
                metrics
                    .connection_acquire_duration
                    .with_label_values(&["reused"])
                    .observe(start_time.elapsed().as_secs_f64());
            }

            trace!("Reusing idle connection");
            return Ok(AcquireResult::Success(conn));
        }

        // Try to create a new connection if under capacity
        if state.active_count < state.capacity {
            state.active_count += 1;
            debug_assert!(state.invariant());

            if let Some(ref metrics) = self.config.metrics {
                state.update_metrics(metrics);
            }

            // Release lock before creating connection
            drop(state);

            match self.create_connection().await {
                Ok(conn) => {
                    if let Some(ref metrics) = self.config.metrics {
                        metrics
                            .total_connections_created
                            .with_label_values(&["success"])
                            .inc();
                        metrics
                            .connection_acquire_duration
                            .with_label_values(&["created"])
                            .observe(start_time.elapsed().as_secs_f64());
                    }
                    debug!("Created new connection");
                    return Ok(AcquireResult::Success(conn));
                }
                Err(e) => {
                    // Decrement active count on failure
                    let mut state = self.state.lock().await;
                    state.active_count = state.active_count.saturating_sub(1);
                    debug_assert!(state.invariant());

                    if let Some(ref metrics) = self.config.metrics {
                        state.update_metrics(metrics);
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
                    return Err(e);
                }
            }
        }

        // Pool is at capacity, must wait
        state.waiting_count += 1;
        trace!(
            "Pool at capacity ({}/{}), waiting",
            state.active_count,
            state.capacity
        );
        Ok(AcquireResult::WaitRequired)
    }

    /// Create a new connection to the daemon.
    async fn create_connection(&self) -> DaemonResult<PooledConnection> {
        let connect_fut = async {
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

    /// Get current pool statistics.
    ///
    /// Returns (idle_count, active_count, capacity).
    pub async fn stats(&self) -> (usize, usize, usize) {
        let state = self.state.lock().await;
        (state.idle.len(), state.active_count, state.capacity)
    }
}

/// RAII guard that ensures connections are returned to the pool.
pub struct PooledConnectionGuard {
    conn: Option<PooledConnection>,
    pool: Arc<Mutex<PoolState>>,
    metrics: Option<Arc<PoolMetrics>>,
    notify: Arc<Notify>,
}

impl PooledConnectionGuard {
    /// Get a reference to the underlying client.
    pub fn client(&mut self) -> &mut UnixDaemonClient {
        &mut self.conn.as_mut().expect("Connection already taken").client
    }

    /// Mark the connection as broken.
    ///
    /// A broken connection will not be returned to the pool.
    /// Call this when an operation fails with a connection-level error.
    pub fn mark_broken(mut self) {
        self.conn = None;
        if let Some(ref metrics) = self.metrics {
            metrics
                .connection_errors
                .with_label_values(&["broken"])
                .inc();
        }
    }
}

impl Drop for PooledConnectionGuard {
    fn drop(&mut self) {
        let conn = self.conn.take();
        let pool = Arc::clone(&self.pool);
        let metrics = self.metrics.clone();
        let notify = Arc::clone(&self.notify);

        // Use tokio::spawn to avoid blocking in drop
        tokio::spawn(async move {
            let mut state = pool.lock().await;
            debug_assert!(state.invariant());

            if let Some(mut conn) = conn {
                // Return healthy connection to pool
                if state.active_count > 0 {
                    state.active_count -= 1;
                    conn.last_used = Instant::now();
                    state.idle.push_back(conn);

                    if let Some(ref metrics) = metrics {
                        state.update_metrics(metrics);
                    }
                }
            } else if state.active_count > 0 {
                // Broken connection, just decrement count
                state.active_count -= 1;

                if let Some(ref metrics) = metrics {
                    state.update_metrics(metrics);
                }
            }

            debug_assert!(state.invariant());

            // Notify waiters if any
            if state.waiting_count > 0 {
                drop(state);
                notify.notify_one();
            }
        });
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
        assert!(config.acquire_timeout > Duration::ZERO);
        assert!(config.connection_timeout > Duration::ZERO);
    }

    #[test]
    #[should_panic(expected = "Pool capacity must be positive")]
    fn test_pool_zero_capacity() {
        let mut config = PoolConfig::default();
        config.max_size = 0;
        let _ = ConnectionPool::new("/tmp/test.sock", config);
    }

    #[test]
    fn test_pool_state_invariant() {
        let state = PoolState {
            idle: VecDeque::new(),
            active_count: 0,
            capacity: 5,
            waiting_count: 0,
        };
        assert!(state.invariant());
    }
}
