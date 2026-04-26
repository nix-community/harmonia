//! Regression tests for HTTP error-response hygiene: handlers must not leak
//! server filesystem paths, OS error strings or internal error-type names to
//! clients, and lookup misses must map to 4xx rather than 5xx.

use std::fs;
use std::process::Command;

mod common;

use common::{CanonicalTempDir, LocalStore, pick_unused_port, start_harmonia_cache};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Spin up a cache with one directory store path and return (port, guard,
/// store_dir, dir_hash).
async fn cache_with_dir() -> Result<(u16, Box<dyn Send>, std::path::PathBuf, String)> {
    let temp_dir = CanonicalTempDir::new()?;
    let store_dir = temp_dir.path().join("store");
    let state_dir = temp_dir.path().join("var/nix");

    let test_dir = temp_dir.path().join("my-dir");
    fs::create_dir_all(&test_dir)?;
    fs::write(test_dir.join("my-file"), "test contents")?;

    let out = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "store",
            "add-path",
            "--store",
            &format!(
                "local?store={}&state={}",
                store_dir.display(),
                state_dir.display()
            ),
            test_dir.to_str().unwrap(),
        ])
        .env_remove("NIX_REMOTE")
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "nix store add-path failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    let dir_path = String::from_utf8(out.stdout)?.trim().to_string();
    let dir_hash = dir_path
        .rsplit('/')
        .next()
        .and_then(|n| n.split('-').next())
        .ok_or("bad store path")?
        .to_string();

    let store = LocalStore::init_at(store_dir.clone(), state_dir)?;
    let port = pick_unused_port().ok_or("no free port")?;
    let cfg = format!(
        r#"
bind = "127.0.0.1:{port}"
nix_db_path = "{}"
virtual_nix_store = "{}"
real_nix_store = "{}"
"#,
        store.db_path().display(),
        store_dir.display(),
        store_dir.display(),
    );
    let guard = start_harmonia_cache(&cfg, port).await?;
    // Keep temp_dir alive by leaking it into the guard tuple via Box.
    let guard: Box<dyn Send> = Box::new((guard, temp_dir));
    Ok((port, guard, store_dir, dir_hash))
}

/// Requesting a non-existent file under a valid store path used to bubble the
/// `canonicalize()` ENOENT up as a 500 whose body contained the on-disk store
/// path and libc error string. It must be a plain 404 with no internal detail.
#[tokio::test]
async fn serve_missing_file_does_not_leak_fs_path() -> Result<()> {
    let (port, _guard, store_dir, dir_hash) = cache_with_dir().await?;

    let out = Command::new("curl")
        .args([
            "--silent",
            "--max-time",
            "5",
            "--write-out",
            "\n%{http_code}",
            &format!("http://127.0.0.1:{port}/serve/{dir_hash}/does-not-exist"),
        ])
        .output()?;
    assert!(out.status.success(), "curl invocation failed");
    let resp = String::from_utf8(out.stdout)?;
    let (body, status) = resp.rsplit_once('\n').ok_or("no status line")?;

    // A missing sub-path is a client lookup miss, not a server fault.
    assert_eq!(
        status, "404",
        "expected 404 for missing sub-path, got {status} (body: {body:?})"
    );

    // The on-disk store directory must never appear in a response body.
    let store_dir = store_dir.display().to_string();
    assert!(
        !body.contains(&store_dir),
        "response body leaks server filesystem path {store_dir:?}: {body:?}"
    );
    // Generic guard against any io::Error detail leaking through.
    assert!(
        !body.to_ascii_lowercase().contains("no such file"),
        "response body leaks io error detail: {body:?}"
    );

    Ok(())
}
