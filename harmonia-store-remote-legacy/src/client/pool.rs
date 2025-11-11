use crate::client::connection::Connection;
use crate::client::metrics::ClientMetrics;
use crate::error::ProtocolError;
use crate::protocol::ProtocolVersion;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
pub struct PoolConfig {
    pub max_size: usize,
    pub max_idle_time: Duration,
    pub connection_timeout: Duration,
    pub metrics: Option<Arc<ClientMetrics>>,
}

impl Default for PoolConfig {
    fn default() -> Self {
        // Default to number of CPU cores + 1 for some headroom
        // This aligns with typical worker thread counts
        let max_size = std::thread::available_parallelism()
            .map(|n| n.get() + 1)
            .unwrap_or(5);

        Self {
            max_size,
            max_idle_time: Duration::from_secs(300), // 5 minutes
            connection_timeout: Duration::from_secs(5),
            metrics: None,
        }
    }
}

/// Internal connection wrapper
struct PooledConnection {
    connection: Connection,
    last_used: Instant,
    version: ProtocolVersion,
}

impl PooledConnection {
    fn is_expired(&self, max_idle_time: Duration) -> bool {
        self.last_used.elapsed() > max_idle_time
    }
}

/// Pool state matching the Dafny proof
struct PoolState {
    idle: VecDeque<PooledConnection>,
    active_count: usize,
    capacity: usize,
    waiting_count: usize,
}

impl PoolState {
    /// Core invariant from Dafny proof
    fn invariant(&self) -> bool {
        self.active_count + self.idle.len() <= self.capacity && self.capacity > 0
    }

    /// Update metrics to reflect current state
    fn update_metrics(&self, metrics: &ClientMetrics) {
        metrics.idle_connections.set(self.idle.len() as i64);
        metrics.active_connections.set(self.active_count as i64);
    }
}

/// Result of acquire attempt matching Dafny
enum AcquireResult {
    Success(PooledConnection),
    WaitRequired,
}

/// RAII guard that ensures connections are returned
pub struct PooledConnectionGuard {
    conn: Option<PooledConnection>,
    pool: Arc<Mutex<PoolState>>,
    metrics: Option<Arc<ClientMetrics>>,
    notify: Arc<Notify>,
}

/// A connection pool with formally verified safety properties
/// This implementation maintains the invariant: active + idle ≤ capacity
#[derive(Clone)]
pub struct ConnectionPool {
    state: Arc<Mutex<PoolState>>,
    socket_path: PathBuf,
    store_dir: harmonia_store_core::store_path::StoreDir,
    max_idle_time: Duration,
    connection_timeout: Duration,
    metrics: Option<Arc<ClientMetrics>>,
    available_notify: Arc<Notify>,
}

impl ConnectionPool {
    /// Create a new pool with given configuration
    pub fn new(socket_path: PathBuf, store_dir: harmonia_store_core::store_path::StoreDir, config: PoolConfig) -> Self {
        assert!(config.max_size > 0, "Capacity must be positive");

        let state = PoolState {
            idle: VecDeque::new(),
            active_count: 0,
            capacity: config.max_size,
            waiting_count: 0,
        };

        debug_assert!(state.invariant());

        Self {
            state: Arc::new(Mutex::new(state)),
            socket_path,
            store_dir,
            max_idle_time: config.max_idle_time,
            connection_timeout: config.connection_timeout,
            metrics: config.metrics,
            available_notify: Arc::new(Notify::new()),
        }
    }

    /// Try to acquire a connection - internal helper matching Dafny TryAcquire
    async fn try_acquire(
        &self,
        start_time: Instant,
    ) -> Result<(AcquireResult, Option<PooledConnection>), ProtocolError> {
        let mut state = self.state.lock().await;
        debug_assert!(state.invariant());

        // Update metrics to show current state
        if let Some(ref metrics) = self.metrics {
            state.update_metrics(metrics);
        }

        // Remove expired connections
        state
            .idle
            .retain(|conn| !conn.is_expired(self.max_idle_time));

        if let Some(mut conn) = state.idle.pop_front() {
            // Reuse idle connection
            state.active_count += 1;
            conn.last_used = Instant::now();
            debug_assert!(state.invariant());

            // Update metrics after state change
            if let Some(ref metrics) = self.metrics {
                state.update_metrics(metrics);

                let duration = start_time.elapsed().as_secs_f64();
                metrics
                    .connection_acquire_duration
                    .with_label_values(&["reused"])
                    .observe(duration);
            }

            Ok((AcquireResult::Success(conn), None))
        } else if state.active_count < state.capacity {
            // Create new connection
            state.active_count += 1;
            debug_assert!(state.invariant());

            // Update metrics after incrementing active count
            if let Some(ref metrics) = self.metrics {
                state.update_metrics(metrics);
            }

            // Drop lock before creating connection
            drop(state);

            // Create connection outside lock
            match self.create_new_connection().await {
                Ok(conn) => {
                    // Track successful creation
                    if let Some(ref metrics) = self.metrics {
                        metrics
                            .total_connections_created
                            .with_label_values(&["success"])
                            .inc();
                        let duration = start_time.elapsed().as_secs_f64();
                        metrics
                            .connection_acquire_duration
                            .with_label_values(&["created"])
                            .observe(duration);
                    }
                    Ok((AcquireResult::Success(conn), None))
                }
                Err(e) => {
                    // Decrement on failure
                    let mut state = self.state.lock().await;
                    state.active_count -= 1;
                    debug_assert!(state.invariant());

                    // Update metrics after decrementing
                    if let Some(ref metrics) = self.metrics {
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

                    Err(e)
                }
            }
        } else {
            // Must wait - pool is full
            state.waiting_count += 1;
            Ok((AcquireResult::WaitRequired, None))
        }
    }

    /// Acquire a connection from the pool
    pub async fn acquire(&self) -> Result<PooledConnectionGuard, ProtocolError> {
        let start = Instant::now();

        loop {
            let (result, _conn_opt) = self.try_acquire(start).await?;

            match result {
                AcquireResult::Success(conn) => {
                    // Track metrics based on whether connection was reused or created
                    // This will be tracked in try_acquire based on the path taken

                    return Ok(PooledConnectionGuard {
                        conn: Some(conn),
                        pool: Arc::clone(&self.state),
                        metrics: self.metrics.clone(),
                        notify: Arc::clone(&self.available_notify),
                    });
                }
                AcquireResult::WaitRequired => {
                    // Wait for notification with timeout
                    match tokio::time::timeout(
                        self.connection_timeout,
                        self.available_notify.notified(),
                    )
                    .await
                    {
                        Ok(_) => {
                            // Try again after being notified
                            let mut state = self.state.lock().await;
                            state.waiting_count -= 1;
                            continue;
                        }
                        Err(_) => {
                            // Timeout waiting
                            let mut state = self.state.lock().await;
                            state.waiting_count -= 1;

                            if let Some(ref metrics) = self.metrics {
                                metrics
                                    .connection_errors
                                    .with_label_values(&["timeout"])
                                    .inc();
                                let duration = start.elapsed().as_secs_f64();
                                metrics
                                    .connection_acquire_duration
                                    .with_label_values(&["timeout"])
                                    .observe(duration);
                            }

                            return Err(ProtocolError::PoolTimeout);
                        }
                    }
                }
            }
        }
    }

    async fn create_new_connection(&self) -> Result<PooledConnection, ProtocolError> {
        let connect_fut = Connection::connect(&self.socket_path, self.store_dir.clone());
        let (connection, version, _features) =
            tokio::time::timeout(self.connection_timeout, connect_fut)
                .await
                .map_err(|_| ProtocolError::ConnectionTimeout)??;

        Ok(PooledConnection {
            connection,
            last_used: Instant::now(),
            version,
        })
    }

    /// Get pool statistics
    pub async fn stats(&self) -> (usize, usize, usize) {
        let state = self.state.lock().await;
        (state.idle.len(), state.active_count, state.capacity)
    }
}

impl PooledConnectionGuard {
    /// Get mutable access to the connection
    pub fn connection(&mut self) -> &mut Connection {
        &mut self.conn.as_mut().unwrap().connection
    }

    /// Get the protocol version
    pub fn version(&self) -> ProtocolVersion {
        self.conn.as_ref().unwrap().version
    }

    /// Get both connection and version
    pub fn connection_and_version(&mut self) -> (&mut Connection, ProtocolVersion) {
        let conn = self.conn.as_mut().unwrap();
        (&mut conn.connection, conn.version)
    }

    /// Mark connection as broken (won't be returned to pool)
    pub fn mark_broken(mut self) {
        self.conn = None;
    }
}

impl Drop for PooledConnectionGuard {
    fn drop(&mut self) {
        // Use tokio::spawn to avoid blocking in drop
        let conn = self.conn.take();
        let pool = Arc::clone(&self.pool);
        let metrics = self.metrics.clone();
        let notify = Arc::clone(&self.notify);

        tokio::spawn(async move {
            let mut state = pool.lock().await;

            // Precondition
            debug_assert!(state.invariant());

            if let Some(conn) = conn {
                if state.active_count > 0 {
                    state.active_count -= 1;

                    // Return healthy connection to pool
                    state.idle.push_back(conn);

                    // Update metrics
                    if let Some(ref metrics) = metrics {
                        state.update_metrics(metrics);
                    }
                }
            } else if state.active_count > 0 {
                // Broken connection
                state.active_count -= 1;

                // Update metrics
                if let Some(ref metrics) = metrics {
                    state.update_metrics(metrics);
                }
            }

            // Postcondition
            debug_assert!(state.invariant());

            // Notify waiters if any
            if state.waiting_count > 0 {
                drop(state);
                notify.notify_one();
            }
        });
    }
}
