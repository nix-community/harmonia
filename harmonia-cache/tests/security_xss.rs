//! Regression tests for HTML/template injection: exercise the directory
//! listing (`/serve/...`) and the landing page (`/`) with attacker-controlled
//! inputs (store-path filenames, `Host` / `X-Forwarded-Proto` headers) and
//! assert that no active HTML leaks into the response.

use std::fs;
use std::process::Command;

mod common;

use common::{CanonicalTempDir, LocalStore, pick_unused_port, start_harmonia_cache};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A store path may contain files with arbitrary names. The directory listing
/// inserts the percent-encoded filename into an `href="..."` attribute; before
/// the fix only C0 controls were encoded, so a `"` in the name broke out of the
/// attribute and allowed stored XSS.
#[tokio::test]
async fn serve_directory_listing_escapes_filenames_in_href() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;
    let root = temp_dir.path().join("root");
    let store = LocalStore::init(&root)?;

    // Build a directory whose entry name would break out of href="...".
    let payload_dir = temp_dir.path().join("payload");
    fs::create_dir_all(&payload_dir)?;
    let evil_name = r#"x" onmouseover="alert(1)"#;
    fs::write(payload_dir.join(evil_name), b"hi")?;
    // Also probe template-placeholder injection through a filename.
    fs::write(payload_dir.join("[[css]]"), b"hi")?;

    let store_uri = format!(
        "local?store={}&state={}",
        store.store_dir.display(),
        store.state_dir.display()
    );
    let out = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "store",
            "add-path",
            "--store",
            &store_uri,
            payload_dir.to_str().unwrap(),
        ])
        .env_remove("NIX_REMOTE")
        .output()?;
    assert!(
        out.status.success(),
        "nix store add-path failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let store_path = String::from_utf8(out.stdout)?.trim().to_string();
    let hash = store_path
        .rsplit('/')
        .next()
        .and_then(|n| n.split('-').next())
        .ok_or("bad store path")?
        .to_string();

    let port = pick_unused_port().ok_or("no free port")?;
    let cfg = format!(
        r#"
bind = "127.0.0.1:{port}"
virtual_nix_store = "{}"
real_nix_store = "{}"
nix_db_path = "{}"
"#,
        store.store_dir.display(),
        store.store_dir.display(),
        store.db_path().display(),
    );
    let _guard = start_harmonia_cache(&cfg, port).await?;

    let out = Command::new("curl")
        .args([
            "--fail",
            "-sS",
            "--max-time",
            "5",
            &format!("http://127.0.0.1:{port}/serve/{hash}/"),
        ])
        .output()?;
    assert!(
        out.status.success(),
        "curl /serve failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8(out.stdout)?;

    assert!(
        !body.contains(r#"onmouseover="alert(1)"#),
        "directory listing leaked unescaped attribute payload:\n{body}"
    );
    // [[css]] filename must appear literally, proving values are not
    // re-expanded as template placeholders.
    assert!(
        body.contains("[[css]]") || body.contains("%5B%5Bcss%5D%5D"),
        "expected literal [[css]] filename in listing:\n{body}"
    );

    Ok(())
}

/// `Host` and `X-Forwarded-Proto` are reflected into the landing page as the
/// suggested substituter URL. They must be HTML-escaped (and the scheme
/// restricted) so a hostile header cannot inject markup or a fake cache URL.
#[tokio::test]
async fn landing_page_escapes_host_and_proto_headers() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;
    let root = temp_dir.path().join("root");
    let store = LocalStore::init(&root)?;

    let port = pick_unused_port().ok_or("no free port")?;
    let cfg = format!(
        r#"
bind = "127.0.0.1:{port}"
virtual_nix_store = "{}"
real_nix_store = "{}"
nix_db_path = "{}"
"#,
        store.store_dir.display(),
        store.store_dir.display(),
        store.db_path().display(),
    );
    let _guard = start_harmonia_cache(&cfg, port).await?;

    let out = Command::new("curl")
        .args([
            "--fail",
            "-sS",
            "--max-time",
            "5",
            "-H",
            "Host: evil<script>alert(1)</script>",
            "-H",
            "X-Forwarded-Proto: javascript:<img src=x onerror=alert(1)>",
            &format!("http://127.0.0.1:{port}/"),
        ])
        .output()?;
    assert!(
        out.status.success(),
        "curl / failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8(out.stdout)?;

    assert!(
        !body.contains("<script>alert(1)</script>"),
        "landing page reflected Host header verbatim:\n{body}"
    );
    assert!(
        !body.contains("<img src=x"),
        "landing page reflected X-Forwarded-Proto verbatim:\n{body}"
    );
    assert!(
        !body.contains("javascript:"),
        "landing page accepted non-http(s) scheme from header"
    );

    Ok(())
}
