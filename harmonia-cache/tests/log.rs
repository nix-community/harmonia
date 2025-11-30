mod daemon;

use daemon::{
    CanonicalTempDir, Daemon, DaemonConfig, NixDaemon, Result, TestCache, build_hello_derivation,
    setup_nix_env,
};

#[tokio::test]
async fn test_build_log_endpoint() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;
    let root = temp_dir.path();

    let env_vars = setup_nix_env(root);

    // Build a derivation to create a build log
    let drv_name = build_hello_derivation(&env_vars)?;
    println!("Built derivation: {drv_name}");

    // Verify the log file exists (Nix compresses logs as .bz2)
    let drv_hash = &drv_name[0..2];
    let drv_rest = &drv_name[2..];
    let log_path = root.join("var/log/nix/drvs").join(drv_hash).join(drv_rest);
    let log_path_bz2 = log_path.with_extension("drv.bz2");

    let actual_log_path = if log_path.exists() {
        log_path
    } else if log_path_bz2.exists() {
        log_path_bz2
    } else {
        panic!(
            "Build log should exist at {} or {}",
            log_path.display(),
            log_path_bz2.display()
        );
    };
    println!("Log found at: {}", actual_log_path.display());

    // Now start a daemon and cache to test the HTTP endpoint
    let socket_path = root.join("daemon.sock");
    let daemon = NixDaemon::start(DaemonConfig {
        socket_path: socket_path.clone(),
        store_dir: root.join("store"),
        state_dir: root.join("var/nix"),
    })
    .await?;

    let cache = TestCache::builder().daemon(daemon).build().await?;

    // Fetch the build log via HTTP - use just the hash part (first 32 chars)
    let drv_hash_query = &drv_name[0..32];
    let body = cache.curl(&format!("/log/{drv_hash_query}"))?;
    println!("Build log from HTTP: {body}");

    assert!(
        body.contains("hello"),
        "HTTP response should contain 'hello', got: {body}"
    );

    Ok(())
}
