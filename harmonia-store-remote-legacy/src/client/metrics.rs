use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry};

#[derive(Clone)]
pub struct ClientMetrics {
    pub active_connections: IntGauge,
    pub idle_connections: IntGauge,
    pub total_connections_created: IntCounterVec,
    pub connection_acquire_duration: HistogramVec,
    pub connection_errors: IntCounterVec,
}

impl ClientMetrics {
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
            .buckets(vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5]),
            &["outcome"], // "reused", "created", "timeout", "error"
        )?;

        let connection_errors = IntCounterVec::new(
            Opts::new(
                format!("{prefix}_daemon_connection_errors_total"),
                "Total number of Nix daemon connection errors",
            ),
            &["error_type"], // "timeout", "broken", "creation_failed"
        )?;

        registry.register(Box::new(active_connections.clone()))?;
        registry.register(Box::new(idle_connections.clone()))?;
        registry.register(Box::new(total_connections_created.clone()))?;
        registry.register(Box::new(connection_acquire_duration.clone()))?;
        registry.register(Box::new(connection_errors.clone()))?;

        Ok(ClientMetrics {
            active_connections,
            idle_connections,
            total_connections_created,
            connection_acquire_duration,
            connection_errors,
        })
    }
}
