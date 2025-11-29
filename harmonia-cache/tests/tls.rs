use std::fs;
use std::process::Command;

mod daemon;

use daemon::{
    CanonicalTempDir, Daemon, DaemonConfig, HarmoniaDaemon, pick_unused_port, start_harmonia_cache,
};

// Compile in the test TLS certificates from the repo
const TLS_CERT: &str = include_str!("../../tests/tls-cert.pem");
const TLS_KEY: &str = include_str!("../../tests/tls-key.pem");

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::test]
async fn test_tls() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;

    // Set up daemon
    let daemon_config = DaemonConfig {
        socket_path: temp_dir.path().join("harmonia-daemon.sock"),
        store_dir: temp_dir.path().join("store"),
        state_dir: temp_dir.path().join("var"),
    };

    let daemon = HarmoniaDaemon::start(daemon_config).await?;

    println!(
        "Starting TLS test with daemon at {}...",
        daemon.socket_path.display()
    );

    // Create log directory
    fs::create_dir_all(daemon.state_dir.join("log"))?;

    // Write TLS cert and key to temp files
    let cert_path = temp_dir.path().join("tls-cert.pem");
    let key_path = temp_dir.path().join("tls-key.pem");
    fs::write(&cert_path, TLS_CERT)?;
    fs::write(&key_path, TLS_KEY)?;

    // Find an available port
    let port = pick_unused_port().ok_or("No available ports")?;

    // Start harmonia-cache with TLS
    let cache_config = format!(
        r#"
bind = "127.0.0.1:{}"
daemon_socket = "{}"
priority = 30
virtual_nix_store = "{}"
real_nix_store = "{}"
tls_cert_path = "{}"
tls_key_path = "{}"
"#,
        port,
        daemon.socket_path.display(),
        daemon.store_dir.display(),
        daemon.store_dir.display(),
        cert_path.display(),
        key_path.display(),
    );

    let _cache_guard = start_harmonia_cache(&cache_config, port).await?;

    // Test HTTPS endpoint with curl
    println!("Testing HTTPS endpoint...");

    let output = Command::new("curl")
        .args([
            "--cacert",
            cert_path.to_str().unwrap(),
            "--fail",
            "--max-time",
            "5",
            "--insecure", // Allow self-signed certificates
            &format!("https://localhost:{port}/version"),
        ])
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to connect to HTTPS endpoint: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    println!("Successfully connected to HTTPS endpoint");
    println!("Response: {}", String::from_utf8_lossy(&output.stdout));

    // Also test nix-cache-info endpoint
    let output = Command::new("curl")
        .args([
            "--cacert",
            cert_path.to_str().unwrap(),
            "--fail",
            "--max-time",
            "5",
            "--insecure", // Allow self-signed certificates
            &format!("https://localhost:{port}/nix-cache-info"),
        ])
        .output()?;

    assert!(output.status.success(), "Failed to get nix-cache-info");

    let response = String::from_utf8_lossy(&output.stdout);
    assert!(
        response.contains("StoreDir:"),
        "Invalid nix-cache-info response"
    );
    assert!(
        response.contains("Priority: 30"),
        "Invalid priority in response"
    );

    Ok(())
}
