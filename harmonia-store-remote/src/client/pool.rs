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
        Self {
            max_size: 5,
            max_idle_time: Duration::from_secs(300), // 5 minutes
            connection_timeout: Duration::from_secs(5),
            metrics: None,
        }
    }
}

pub struct ConnectionPool {
    // Pool of available connections
    idle_connections: Arc<Mutex<VecDeque<PooledConnection>>>,
    // Configuration
    socket_path: PathBuf,
    max_size: usize,
    max_idle_time: Duration,
    connection_timeout: Duration,
    // Statistics
    active_count: Arc<Mutex<usize>>,
    // Notify when a connection is returned to the pool
    available_notify: Arc<Notify>,
    // Metrics
    metrics: Option<Arc<ClientMetrics>>,
}

#[derive(Debug)]
struct PooledConnection {
    connection: Connection,
    #[allow(dead_code)]
    created_at: Instant,
    last_used: Instant,
    version: ProtocolVersion,
    #[allow(dead_code)]
    features: Vec<Vec<u8>>,
}

impl PooledConnection {
    fn is_expired(&self, max_idle_time: Duration) -> bool {
        self.last_used.elapsed() > max_idle_time
    }
}

impl ConnectionPool {
    pub fn new(socket_path: PathBuf, config: PoolConfig) -> Self {
        Self {
            idle_connections: Arc::new(Mutex::new(VecDeque::new())),
            socket_path,
            max_size: config.max_size,
            max_idle_time: config.max_idle_time,
            connection_timeout: config.connection_timeout,
            active_count: Arc::new(Mutex::new(0)),
            available_notify: Arc::new(Notify::new()),
            metrics: config.metrics,
        }
    }
    pub async fn acquire(&self) -> Result<PooledConnectionGuard, ProtocolError> {
        let start = Instant::now();

        // First, try to get an idle connection
        let mut idle = self.idle_connections.lock().await;

        // Update idle connections metric
        if let Some(ref metrics) = self.metrics {
            metrics.idle_connections.set(idle.len() as i64);
        }

        // Remove expired connections
        idle.retain(|conn| !conn.is_expired(self.max_idle_time));

        if let Some(mut conn) = idle.pop_front() {
            drop(idle); // Release lock early

            // Validate connection is still alive
            if self.validate_connection(&mut conn).await {
                conn.last_used = Instant::now();

                // Track successful reuse
                if let Some(ref metrics) = self.metrics {
                    let duration = start.elapsed().as_secs_f64();
                    metrics
                        .connection_acquire_duration
                        .with_label_values(&["reused"])
                        .observe(duration);
                }

                return Ok(PooledConnectionGuard::new(conn, self.clone()));
            } else {
                // Track broken connection
                if let Some(ref metrics) = self.metrics {
                    metrics
                        .connection_errors
                        .with_label_values(&["broken"])
                        .inc();
                }
            }
        } else {
            drop(idle); // Release lock early
        }

        // Create new connection if under limit
        let active_count = {
            let mut active = self.active_count.lock().await;
            if *active < self.max_size {
                *active += 1;
                Some(*active)
            } else {
                None
            }
        };

        if active_count.is_some() {
            // Update active connections metric
            if let Some(ref metrics) = self.metrics {
                metrics.active_connections.set(active_count.unwrap() as i64);
            }

            match self.create_new_connection().await {
                Ok(conn) => {
                    // Track successful creation
                    if let Some(ref metrics) = self.metrics {
                        metrics
                            .total_connections_created
                            .with_label_values(&["success"])
                            .inc();
                        let duration = start.elapsed().as_secs_f64();
                        metrics
                            .connection_acquire_duration
                            .with_label_values(&["created"])
                            .observe(duration);
                    }
                    Ok(conn)
                }
                Err(e) => {
                    // Decrement count on failure
                    *self.active_count.lock().await -= 1;
                    self.available_notify.notify_one();

                    // Track failed creation
                    if let Some(ref metrics) = self.metrics {
                        metrics
                            .total_connections_created
                            .with_label_values(&["error"])
                            .inc();
                        metrics
                            .connection_errors
                            .with_label_values(&["creation_failed"])
                            .inc();
                        let duration = start.elapsed().as_secs_f64();
                        metrics
                            .connection_acquire_duration
                            .with_label_values(&["error"])
                            .observe(duration);
                    }
                    Err(e)
                }
            }
        } else {
            // Pool is full, wait for a connection to become available
            loop {
                // Check once more if a connection became available
                let mut idle = self.idle_connections.lock().await;
                if let Some(mut conn) = idle.pop_front() {
                    drop(idle);
                    if self.validate_connection(&mut conn).await {
                        conn.last_used = Instant::now();
                        return Ok(PooledConnectionGuard::new(conn, self.clone()));
                    }
                    // Connection was invalid, try again
                    continue;
                }
                drop(idle);

                // Wait for notification with timeout
                match tokio::time::timeout(
                    self.connection_timeout,
                    self.available_notify.notified(),
                )
                .await
                {
                    Ok(_) => continue, // Try again
                    Err(_) => {
                        // Track timeout
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

    async fn create_new_connection(&self) -> Result<PooledConnectionGuard, ProtocolError> {
        // Use timeout for connection attempts
        let connect_fut = Connection::connect(&self.socket_path);
        let (connection, version, features) =
            tokio::time::timeout(self.connection_timeout, connect_fut)
                .await
                .map_err(|_| ProtocolError::ConnectionTimeout)??;

        let pooled = PooledConnection {
            connection,
            created_at: Instant::now(),
            last_used: Instant::now(),
            version,
            features,
        };

        Ok(PooledConnectionGuard::new(pooled, self.clone()))
    }

    async fn validate_connection(&self, _conn: &mut PooledConnection) -> bool {
        // Could implement a lightweight ping operation
        // For now, assume connections are valid
        true
    }

    fn return_connection(&self, mut conn: PooledConnection) {
        conn.last_used = Instant::now();

        // Return to pool asynchronously
        let idle = self.idle_connections.clone();
        let notify = self.available_notify.clone();
        let metrics = self.metrics.clone();
        tokio::spawn(async move {
            let mut idle_guard = idle.lock().await;
            idle_guard.push_back(conn);

            // Update idle connections metric
            if let Some(ref metrics) = metrics {
                metrics.idle_connections.set(idle_guard.len() as i64);
            }
            drop(idle_guard);

            notify.notify_one(); // Notify waiting threads
        });
    }
}

impl Clone for ConnectionPool {
    fn clone(&self) -> Self {
        Self {
            idle_connections: self.idle_connections.clone(),
            socket_path: self.socket_path.clone(),
            max_size: self.max_size,
            max_idle_time: self.max_idle_time,
            connection_timeout: self.connection_timeout,
            active_count: self.active_count.clone(),
            available_notify: self.available_notify.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

pub struct PooledConnectionGuard {
    conn: Option<PooledConnection>,
    pool: ConnectionPool,
}

impl PooledConnectionGuard {
    fn new(conn: PooledConnection, pool: ConnectionPool) -> Self {
        Self {
            conn: Some(conn),
            pool,
        }
    }

    pub fn connection(&mut self) -> &mut Connection {
        &mut self.conn.as_mut().unwrap().connection
    }

    pub fn version(&self) -> ProtocolVersion {
        self.conn.as_ref().unwrap().version
    }

    pub fn connection_and_version(&mut self) -> (&mut Connection, ProtocolVersion) {
        let conn = self.conn.as_mut().unwrap();
        (&mut conn.connection, conn.version)
    }

    pub fn mark_broken(mut self) {
        self.conn = None; // Don't return to pool
    }
}

impl Drop for PooledConnectionGuard {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.return_connection(conn);
        } else {
            // Connection was marked broken, decrement active count
            let active = self.pool.active_count.clone();
            let notify = self.pool.available_notify.clone();
            let metrics = self.pool.metrics.clone();
            tokio::spawn(async move {
                let new_count = {
                    let mut active_guard = active.lock().await;
                    *active_guard -= 1;
                    *active_guard
                };

                // Update active connections metric
                if let Some(ref metrics) = metrics {
                    metrics.active_connections.set(new_count as i64);
                }

                notify.notify_one(); // Notify waiting threads
            });
        }
    }
}
