use std::fs;
use std::io::Write;
use std::process::Command;
use tempfile::{NamedTempFile, TempDir};

mod daemon;

use daemon::{
    pick_unused_port, start_harmonia_cache, Daemon, DaemonConfig, DaemonInstance, HarmoniaDaemon,
    NixDaemon,
};

// Compile in the test keys from the repo
const SIGNING_KEY_1: &str = include_str!("../../tests/cache.sk");
const PUBLIC_KEY_1: &str = include_str!("../../tests/cache.pk");
const SIGNING_KEY_2: &str = include_str!("../../tests/cache2.sk");
const PUBLIC_KEY_2: &str = include_str!("../../tests/cache2.pk");

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// Helper function to write a string to a temporary file
fn write_temp_file(content: &str) -> Result<NamedTempFile> {
    let mut file = NamedTempFile::new()?;
    write!(file, "{content}")?; // Use write! instead of writeln! to preserve exact content
    file.flush()?;
    Ok(file)
}

// Helper function to test signing with a specific daemon
async fn test_signing_with_daemon(daemon: &DaemonInstance) -> Result<()> {
    println!(
        "Starting signing test with daemon at {}...",
        daemon.socket_path.display()
    );

    // Create temporary directory for harmonia's working files
    let temp_dir = TempDir::new()?;

    // Create log directory
    fs::create_dir_all(daemon.state_dir.join("log"))?;

    // Write signing keys to temp files
    let key_file1 = write_temp_file(SIGNING_KEY_1.trim())?;
    let key_file2 = write_temp_file(SIGNING_KEY_2.trim())?;

    // Find an available port
    let port = pick_unused_port().ok_or("No available ports")?;

    // Start harmonia-cache with the daemon socket
    let cache_config = format!(
        r#"
bind = "127.0.0.1:{}"
daemon_socket = "{}"
sign_key_paths = ["{}", "{}"]
priority = 30
virtual_nix_store = "{}"
real_nix_store = "{}"
"#,
        port,
        daemon.socket_path.display(),
        key_file1.path().display(),
        key_file2.path().display(),
        daemon.store_dir.display(),
        daemon.store_dir.display(),
    );

    let _cache_guard = start_harmonia_cache(&cache_config, port).await?;

    // Build test packages with references using builtins.derivation
    let expr = r#"
    let
      # First derivation - a library/dependency
      libfoo = builtins.derivation {
        name = "test-libfoo";
        builder = "/bin/sh";
        args = ["-c" "echo 'I am libfoo' > $out"];
        system = builtins.currentSystem;
      };
      
      # Second derivation that references the first
      hello = builtins.derivation {
        name = "test-hello";
        builder = "/bin/sh";
        args = ["-c" "cat ${libfoo} > $out; echo 'hello world for signing test' >> $out"];
        system = builtins.currentSystem;
      };
    in hello
    "#;

    let expr_file = write_temp_file(expr)?;

    let output = Command::new("nix-build")
        .args([
            "--no-out-link",
            "--store",
            &format!(
                "local?store={}&state={}",
                daemon.store_dir.display(),
                daemon.state_dir.display()
            ),
            "--extra-experimental-features",
            "nix-command",
            "--option",
            "substitute",
            "false",
            "--option",
            "builders",
            "", // Disable remote builders
            "--substituters",
            "",
            expr_file.path().to_str().unwrap(),
        ])
        .env_remove("NIX_REMOTE") // Disable remote store
        .env("NIX_LOG_DIR", daemon.state_dir.join("log"))
        .env("NIX_STATE_DIR", &daemon.state_dir)
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to build test package: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let hello_path = String::from_utf8(output.stdout)?.trim().to_string();

    println!("Using test package: {hello_path}");

    // Create temporary stores for clients
    let client1_store = temp_dir.path().join("client1");
    let client2_store = temp_dir.path().join("client2");

    // Create empty config directory to avoid inheriting system config
    let empty_conf_dir = temp_dir.path().join("empty-conf");
    fs::create_dir_all(&empty_conf_dir)?;

    // Test 1: Copy with first key trusted
    println!("Testing copy with first key trusted...");
    println!("Using public key: {}", PUBLIC_KEY_1.trim());

    let output = Command::new("nix")
        .args([
            "copy",
            "--from",
            &format!("http://127.0.0.1:{port}"),
            "--to",
            &format!("{}", client1_store.display(),),
            "--option",
            "trusted-public-keys",
            PUBLIC_KEY_1.trim(),
            "--extra-experimental-features",
            "nix-command flakes",
            &hello_path,
        ])
        .env("NIX_STORE_DIR", &daemon.store_dir)
        .env("NIX_CACHE_HOME", temp_dir.path().join("cache1"))
        .env("NIX_CONFIG", "")
        .env("NIX_CONF_DIR", &empty_conf_dir)
        .env_remove("NIX_USER_CONF_FILES")
        .env_remove("NIX_REMOTE")
        .status()?;

    assert!(output.success(), "First copy failed");

    // Test 2: Copy with second key trusted
    println!("Testing copy with second key trusted...");
    println!("Using public key: {}", PUBLIC_KEY_2.trim());

    let output2 = Command::new("nix")
        .args([
            "copy",
            "--from",
            &format!("http://127.0.0.1:{port}"),
            "--to",
            &format!("{}", client2_store.display(),),
            "--option",
            "trusted-public-keys",
            PUBLIC_KEY_2.trim(),
            "--extra-experimental-features",
            "nix-command flakes",
            &hello_path,
        ])
        .env("NIX_STORE_DIR", &daemon.store_dir)
        .env("NIX_CACHE_HOME", temp_dir.path().join("cache2"))
        .env("NIX_CONFIG", "")
        .env("NIX_CONF_DIR", &empty_conf_dir)
        .env_remove("NIX_USER_CONF_FILES")
        .env_remove("NIX_REMOTE")
        .status()
        .unwrap();

    assert!(output2.success(), "Second copy failed");

    Ok(())
}

#[tokio::test]
async fn test_signing_with_nix_daemon() -> Result<()> {
    // Skip if we don't have nix-daemon available
    if Command::new("nix-daemon")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        println!("Skipping nix-daemon test: nix-daemon not found in PATH");
        return Ok(());
    }

    let temp_dir = TempDir::new()?;

    let daemon_config = DaemonConfig {
        socket_path: temp_dir.path().join("nix-daemon.sock"),
        store_dir: temp_dir.path().join("store"),
        state_dir: temp_dir.path().join("var"),
    };

    let daemon = NixDaemon::start(daemon_config).await?;
    test_signing_with_daemon(&daemon).await
}

#[tokio::test]
async fn test_signing_with_harmonia_daemon() -> Result<()> {
    let temp_dir = TempDir::new()?;

    let daemon_config = DaemonConfig {
        socket_path: temp_dir.path().join("harmonia-daemon.sock"),
        store_dir: temp_dir.path().join("store"),
        state_dir: temp_dir.path().join("var"),
    };

    let daemon = HarmoniaDaemon::start(daemon_config).await?;
    test_signing_with_daemon(&daemon).await
}
