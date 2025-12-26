use std::fs;
use std::process::Command;

mod daemon;

use daemon::{
    CanonicalTempDir, Daemon, DaemonConfig, NixDaemon, pick_unused_port, start_harmonia_cache,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::test]
async fn test_chroot() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;

    // Set up paths for guest store
    let guest_dir = temp_dir.path().join("guest");
    let guest_store = guest_dir.join("nix/store");
    let guest_state = guest_dir.join("nix/var");

    // Create test content
    let test_file = temp_dir.path().join("my-file");
    fs::write(&test_file, "test contents")?;

    let test_dir = temp_dir.path().join("my-dir");
    fs::create_dir_all(&test_dir)?;
    fs::copy(&test_file, test_dir.join("my-file"))?;

    // Add file and directory to the store (this will create the store structure)
    let file_output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "store",
            "add-file",
            "--store",
            &format!(
                "local?store={}&state={}",
                guest_store.display(),
                guest_state.display()
            ),
            test_file.to_str().unwrap(),
        ])
        .env_remove("NIX_REMOTE")
        .output()?;

    if !file_output.status.success() {
        return Err(format!(
            "Failed to add file to store: {}",
            String::from_utf8_lossy(&file_output.stderr)
        )
        .into());
    }

    let file_path = String::from_utf8(file_output.stdout)?.trim().to_string();
    println!("Added file to store: {file_path}");

    let dir_output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "store",
            "add-path",
            "--store",
            &format!(
                "local?store={}&state={}",
                guest_store.display(),
                guest_state.display()
            ),
            test_dir.to_str().unwrap(),
        ])
        .env_remove("NIX_REMOTE")
        .output()?;

    if !dir_output.status.success() {
        return Err(format!(
            "Failed to add directory to store: {}",
            String::from_utf8_lossy(&dir_output.stderr)
        )
        .into());
    }

    let dir_path = String::from_utf8(dir_output.stdout)?.trim().to_string();
    println!("Added directory to store: {dir_path}");

    // Start the Nix daemon with the guest store
    let daemon_config = DaemonConfig {
        socket_path: temp_dir.path().join("nix-daemon.sock"),
        store_dir: guest_store.clone(),
        state_dir: guest_state.clone(),
    };

    let daemon = NixDaemon::start(daemon_config).await?;

    println!(
        "Starting chroot test with daemon at {}...",
        daemon.socket_path.display()
    );

    // To test the chroot mapping logic, we use a different path for the real store.
    // In this test, we'll make real_nix_store a symlink to the actual guest_store.
    let real_store = temp_dir.path().join("real-store");
    std::os::unix::fs::symlink(&guest_store, &real_store)?;

    // Extract directory hash from the store path
    // The path will be something like /nix/store/hash-name
    let dir_hash = dir_path
        .split('/')
        .next_back()
        .ok_or("Invalid store path")?
        .split('-')
        .next()
        .ok_or("Invalid store path format")?;

    // Find an available port
    let port = pick_unused_port().ok_or("No available ports")?;

    // Start harmonia-cache with chroot configuration.
    // virtual_nix_store must match what the daemon uses for protocol communication.
    // real_nix_store is where Harmonia looks for files on the filesystem.
    let cache_config = format!(
        r#"
bind = "127.0.0.1:{}"
daemon_socket = "{}"
priority = 30
virtual_nix_store = "{}"
real_nix_store = "{}"
"#,
        port,
        daemon.socket_path.display(),
        guest_store.display(),
        real_store.display(),
    );

    let _cache_guard = start_harmonia_cache(&cache_config, port).await?;

    // Test basic endpoints
    println!("Testing basic endpoints...");

    let output = Command::new("curl")
        .args([
            "--fail",
            "--max-time",
            "5",
            &format!("http://127.0.0.1:{port}/version"),
        ])
        .output()?;

    assert!(output.status.success(), "Failed to get version");

    let output = Command::new("curl")
        .args([
            "--fail",
            "--max-time",
            "5",
            &format!("http://127.0.0.1:{port}/nix-cache-info"),
        ])
        .output()?;

    assert!(output.status.success(), "Failed to get nix-cache-info");

    // Test directory listing endpoint
    println!("Testing directory listing for hash: {dir_hash}");

    let output = Command::new("curl")
        .args([
            "--fail",
            "--max-time",
            "5",
            &format!("http://127.0.0.1:{port}/{dir_hash}.ls"),
        ])
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to get directory listing: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let listing = String::from_utf8(output.stdout)?;
    println!("Directory listing response: {listing}");

    // Parse JSON response
    let json: serde_json::Value = serde_json::from_str(&listing)?;
    assert_eq!(json["version"], 1, "Invalid listing version");
    assert!(
        json["root"]["entries"]["my-file"]["type"] == "regular",
        "Expected my-file to be regular file in listing"
    );

    // Test serve endpoint - directory listing
    let output = Command::new("curl")
        .args([
            "--fail",
            "--max-time",
            "5",
            &format!("http://127.0.0.1:{port}/serve/{dir_hash}/"),
        ])
        .output()?;

    assert!(
        output.status.success(),
        "Failed to get serve directory listing"
    );
    let response = String::from_utf8(output.stdout)?;
    assert!(
        response.contains("my-file"),
        "my-file not in directory listing"
    );

    // Test serve endpoint - file content
    let output = Command::new("curl")
        .args([
            "--fail",
            "--max-time",
            "5",
            &format!("http://127.0.0.1:{port}/serve/{dir_hash}/my-file"),
        ])
        .output()?;

    assert!(output.status.success(), "Failed to get file content");
    let content = String::from_utf8(output.stdout)?.trim().to_string();
    assert_eq!(
        content, "test contents",
        "Expected 'test contents', got '{content}'"
    );

    // Test that we can fetch narinfo for the file with the virtual path
    println!("Testing narinfo fetch with virtual path...");

    // Extract hash from the file path - note that file_path has the real store path
    // e.g., /tmp/.../guest/nix/store/hash-name
    let file_store_name = file_path
        .split('/')
        .next_back()
        .ok_or("Invalid store path")?;
    let file_hash = file_store_name
        .split('-')
        .next()
        .ok_or("Invalid store path format")?;

    let output = Command::new("curl")
        .args([
            "--verbose",
            "--max-time",
            "5",
            &format!("http://127.0.0.1:{port}/{file_hash}.narinfo"),
        ])
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to get narinfo: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let narinfo = String::from_utf8(output.stdout)?;
    println!("Narinfo response: {narinfo}");

    // Verify the narinfo contains the virtual store path
    assert!(
        narinfo.contains("/nix/store"),
        "Narinfo should contain virtual store path"
    );

    println!("Chroot test completed successfully!");

    Ok(())
}
