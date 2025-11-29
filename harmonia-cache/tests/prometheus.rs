use std::process::Command;

mod daemon;

use daemon::{
    CanonicalTempDir, Daemon, DaemonConfig, HarmoniaDaemon, pick_unused_port, start_harmonia_cache,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::test]
async fn test_prometheus_metrics() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;

    // Set up daemon
    let daemon_config = DaemonConfig {
        socket_path: temp_dir.path().join("harmonia-daemon.sock"),
        store_dir: temp_dir.path().join("store"),
        state_dir: temp_dir.path().join("var"),
    };

    let daemon = HarmoniaDaemon::start(daemon_config).await?;

    println!(
        "Starting Prometheus metrics test with daemon at {}...",
        daemon.socket_path.display()
    );

    // Find an available port
    let port = pick_unused_port().ok_or("No available ports")?;

    // Start harmonia-cache
    let cache_config = format!(
        r#"
bind = "0.0.0.0:{}"
daemon_socket = "{}"
priority = 30
"#,
        port,
        daemon.socket_path.display(),
    );

    let _cache_guard = start_harmonia_cache(&cache_config, port).await?;

    // Make requests to registered routes
    println!("Making test requests...");

    // Request to a registered route
    let _ = Command::new("curl")
        .args([
            "--fail",
            "--max-time",
            "2",
            "--silent",
            &format!("http://localhost:{port}/nix-cache-info"),
        ])
        .output()?;

    // Get metrics
    println!("Fetching metrics...");
    let metrics_output = Command::new("curl")
        .args([
            "--fail",
            "--max-time",
            "2",
            "--silent",
            &format!("http://localhost:{port}/metrics"),
        ])
        .output()?;

    let metrics = String::from_utf8(metrics_output.stdout)?;
    println!("Metrics response:\n{metrics}");

    // Verify that registered routes are tracked
    assert!(
        metrics.contains(r#"path="/nix-cache-info""#),
        "Metrics should include /nix-cache-info path"
    );

    println!("Prometheus metrics filtering test completed successfully!");

    Ok(())
}
