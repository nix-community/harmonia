// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT

//! Prometheus metrics for daemon connection pool monitoring.

use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry};

/// Metrics for monitoring connection pool health and performance.
#[derive(Clone, Debug)]
pub struct PoolMetrics {
    /// Number of currently active (in-use) connections
    pub active_connections: IntGauge,
    /// Number of idle connections available in the pool
    pub idle_connections: IntGauge,
    /// Total connections created, labeled by status ("success" or "error")
    pub total_connections_created: IntCounterVec,
    /// Time spent acquiring connections, labeled by outcome ("reused", "created", "timeout")
    pub connection_acquire_duration: HistogramVec,
    /// Connection errors, labeled by error type ("timeout", "broken", "creation_failed")
    pub connection_errors: IntCounterVec,
    /// Number of operations executed, labeled by operation name and status
    pub operations_total: IntCounterVec,
    /// Operation duration in seconds, labeled by operation name
    pub operation_duration: HistogramVec,
}

impl PoolMetrics {
    /// Create new metrics and register them with the given Prometheus registry.
    ///
    /// # Arguments
    /// * `prefix` - Prefix for metric names (e.g., "harmonia")
    /// * `registry` - Prometheus registry to register metrics with
    pub fn new(prefix: &str, registry: &Registry) -> Result<Self, prometheus::Error> {
        let active_connections = IntGauge::with_opts(Opts::new(
            format!("{prefix}_daemon_active_connections"),
            "Number of active connections to the Nix daemon",
        ))?;

        let idle_connections = IntGauge::with_opts(Opts::new(
            format!("{prefix}_daemon_idle_connections"),
            "Number of idle connections to the Nix daemon",
        ))?;

        let total_connections_created = IntCounterVec::new(
            Opts::new(
                format!("{prefix}_daemon_connections_created_total"),
                "Total number of Nix daemon connections created",
            ),
            &["status"], // "success" or "error"
        )?;

        let connection_acquire_duration = HistogramVec::new(
            HistogramOpts::new(
                format!("{prefix}_daemon_connection_acquire_duration_seconds"),
                "Time spent acquiring a connection to the Nix daemon",
            )
            .buckets(vec![
                0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0,
            ]),
            &["outcome"], // "reused", "created", "timeout", "error"
        )?;

        let connection_errors = IntCounterVec::new(
            Opts::new(
                format!("{prefix}_daemon_connection_errors_total"),
                "Total number of Nix daemon connection errors",
            ),
            &["error_type"], // "timeout", "broken", "creation_failed"
        )?;

        let operations_total = IntCounterVec::new(
            Opts::new(
                format!("{prefix}_daemon_operations_total"),
                "Total number of daemon operations executed",
            ),
            &["operation", "status"], // operation name, "success" or "error"
        )?;

        let operation_duration = HistogramVec::new(
            HistogramOpts::new(
                format!("{prefix}_daemon_operation_duration_seconds"),
                "Duration of daemon operations",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
            ]),
            &["operation"],
        )?;

        registry.register(Box::new(active_connections.clone()))?;
        registry.register(Box::new(idle_connections.clone()))?;
        registry.register(Box::new(total_connections_created.clone()))?;
        registry.register(Box::new(connection_acquire_duration.clone()))?;
        registry.register(Box::new(connection_errors.clone()))?;
        registry.register(Box::new(operations_total.clone()))?;
        registry.register(Box::new(operation_duration.clone()))?;

        Ok(PoolMetrics {
            active_connections,
            idle_connections,
            total_connections_created,
            connection_acquire_duration,
            connection_errors,
            operations_total,
            operation_duration,
        })
    }
}
