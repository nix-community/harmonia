use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command as AsyncCommand;

mod daemon;

use daemon::{
    CanonicalTempDir, Daemon, DaemonConfig, NixDaemon, pick_unused_port, start_harmonia_cache,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// TCP proxy that drops connections after a byte limit.
struct FlakyProxy {
    port: u16,
    connection_count: Arc<AtomicUsize>,
    _handle: tokio::task::JoinHandle<()>,
}

impl FlakyProxy {
    async fn start(upstream_port: u16, byte_limit: usize) -> Result<Self> {
        let port = pick_unused_port().ok_or("No port")?;
        let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
        let connection_count = Arc::new(AtomicUsize::new(0));
        let count = connection_count.clone();

        let handle = tokio::spawn(async move {
            loop {
                if let Ok((client, _)) = listener.accept().await {
                    count.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(proxy_connection(client, upstream_port, byte_limit));
                }
            }
        });

        Ok(Self {
            port,
            connection_count,
            _handle: handle,
        })
    }

    fn connections(&self) -> usize {
        self.connection_count.load(Ordering::SeqCst)
    }
}

async fn proxy_connection(mut client: TcpStream, upstream_port: u16, limit: usize) {
    let Ok(mut upstream) = TcpStream::connect(format!("127.0.0.1:{upstream_port}")).await else {
        return;
    };
    let (mut client_read, mut client_write) = client.split();
    let (mut upstream_read, mut upstream_write) = upstream.split();

    // Forward HTTP request
    let mut buf = vec![0u8; 8192];
    let mut req = Vec::new();
    loop {
        let Ok(n) = client_read.read(&mut buf).await else {
            return;
        };
        if n == 0 {
            return;
        }
        req.extend_from_slice(&buf[..n]);
        let _ = upstream_write.write_all(&buf[..n]).await;
        if req.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let _ = upstream_write.flush().await;

    // Forward response up to limit
    let mut sent = 0;
    loop {
        let Ok(n) = upstream_read.read(&mut buf).await else {
            return;
        };
        if n == 0 {
            break;
        }
        let to_send = n.min(limit.saturating_sub(sent));
        if to_send == 0 {
            break;
        }
        let _ = client_write.write_all(&buf[..to_send]).await;
        sent += to_send;
        if sent >= limit {
            break;
        }
    }
}

/// Test download retry over flaky connection using nix's download-attempts.
#[tokio::test]
async fn test_download_retry_over_flaky_connection() -> Result<()> {
    let temp = CanonicalTempDir::new()?;
    let store_dir = temp.path().join("store");
    let state_dir = temp.path().join("var");

    // Start nix-daemon (inits store)
    let daemon = NixDaemon::start(DaemonConfig {
        socket_path: temp.path().join("daemon.sock"),
        store_dir: store_dir.clone(),
        state_dir: state_dir.clone(),
    })
    .await?;

    // Create 50KB test file - triggers 2 retries with 30KB limit
    let big_file = temp.path().join("big-file");
    let data: Vec<u8> = (0..(50 * 1024)).map(|i| (i % 256) as u8).collect();
    fs::write(&big_file, &data)?;

    // Add to store
    let output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "store",
            "add-file",
            "--store",
            &format!(
                "local?store={}&state={}",
                store_dir.display(),
                state_dir.display()
            ),
            big_file.to_str().unwrap(),
        ])
        .env_remove("NIX_REMOTE")
        .output()?;
    if !output.status.success() {
        return Err(format!("Failed to add: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let store_path = String::from_utf8(output.stdout)?.trim().to_string();
    let store_basename = store_path.split('/').next_back().ok_or("Invalid path")?;

    // Start harmonia-cache
    let cache_port = pick_unused_port().ok_or("No port")?;
    let config = format!(
        "bind = \"127.0.0.1:{cache_port}\"\ndaemon_socket = \"{}\"\nvirtual_nix_store = \"{}\"\nreal_nix_store = \"{}\"",
        daemon.socket_path.display(),
        store_dir.display(),
        store_dir.display()
    );
    let _cache = start_harmonia_cache(&config, cache_port).await?;

    // Start flaky proxy (30KB limit = needs ~2 retries for 50KB NAR)
    let proxy = FlakyProxy::start(cache_port, 30 * 1024).await?;

    // Set up client store and cache dir
    let client_store = temp.path().join("client");
    let empty_conf = temp.path().join("conf");
    let cache_dir = temp.path().join("cache");
    fs::create_dir_all(&client_store)?;
    fs::create_dir_all(&empty_conf)?;
    fs::create_dir_all(&cache_dir)?;

    // Copy through flaky proxy with retries
    let output = AsyncCommand::new("nix")
        .args([
            "copy",
            "--from",
            &format!("http://127.0.0.1:{}", proxy.port),
            "--to",
            client_store.to_str().unwrap(),
            "--option",
            "download-attempts",
            "10",
            "--option",
            "require-sigs",
            "false",
            "--extra-experimental-features",
            "nix-command",
            &store_path,
        ])
        .env("NIX_STORE_DIR", &store_dir)
        .env("NIX_CONFIG", "")
        .env("NIX_CONF_DIR", &empty_conf)
        .env("XDG_CACHE_HOME", &cache_dir)
        .env_remove("NIX_USER_CONF_FILES")
        .env_remove("NIX_REMOTE")
        .output()
        .await?;

    if !output.status.success() {
        return Err(format!(
            "nix copy failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    // Verify file
    let copied = fs::read(client_store.join("nix/store").join(store_basename))?;
    assert_eq!(copied, data, "Copied file doesn't match original");
    assert!(
        proxy.connections() >= 2,
        "Expected multiple connections for retry test"
    );

    Ok(())
}
