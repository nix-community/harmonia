// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// Integration tests that talk to a real cppnix `nix-daemon` over a Unix
// socket. These exist to catch wire-protocol mismatches between our client
// and the C++ daemon that self-tests (client <-> our own server) cannot
// detect.
//
// Regression test for https://github.com/nix-community/harmonia/issues/954:
// several operations (AddTempRoot, EnsurePath, AddIndirectRoot, OptimiseStore,
// AddSignatures, AddBuildLog) are followed by a trailing `1` on the wire that
// the client used to leave unread. The stale word then desynced the *next*
// request on the same connection. We only ever observed this when reusing a
// connection, which is why the tests below always issue a second request.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use harmonia_protocol::types::DaemonStore;
use harmonia_store_core::store_path::{StoreDir, StorePath};
use harmonia_store_remote::DaemonClient;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct NixDaemon {
    _tmp: tempfile::TempDir,
    child: Child,
    socket: PathBuf,
    store_dir: StoreDir,
}

impl Drop for NixDaemon {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn spawn_nix_daemon() -> Result<NixDaemon> {
    // Canonicalise so the (typically short) macOS /var -> /private/var symlink
    // does not push the socket path past sun_path's 104/108 byte limit.
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().canonicalize()?;
    let store_dir = root.join("store");
    let state_dir = root.join("state");
    let log_dir = state_dir.join("log");
    let conf_dir = root.join("etc");
    let socket = root.join("d.sock");
    std::fs::create_dir_all(&store_dir)?;
    std::fs::create_dir_all(&state_dir)?;
    std::fs::create_dir_all(&log_dir)?;
    std::fs::create_dir_all(&conf_dir)?;

    let init = Command::new("nix-store")
        .args([
            "--init",
            "--store",
            &format!(
                "local?store={}&state={}",
                store_dir.display(),
                state_dir.display()
            ),
        ])
        .output()
        .await?;
    if !init.status.success() {
        return Err(format!(
            "nix-store --init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        )
        .into());
    }

    let child = Command::new("nix-daemon")
        .env("NIX_STORE_DIR", &store_dir)
        .env("NIX_STATE_DIR", &state_dir)
        .env("NIX_LOG_DIR", &log_dir)
        .env("NIX_CONF_DIR", &conf_dir)
        .env("NIX_DAEMON_SOCKET_PATH", &socket)
        .env_remove("NIX_REMOTE")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    // Wait for the socket to appear.
    timeout(Duration::from_secs(30), async {
        while !socket.exists() {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "timed out waiting for nix-daemon socket")?;

    Ok(NixDaemon {
        _tmp: tmp,
        child,
        socket,
        store_dir: StoreDir::new(store_dir)?,
    })
}

fn dummy_path() -> StorePath {
    // Any syntactically valid store path will do; the daemon does not
    // validate existence for AddTempRoot/IsValidPath.
    StorePath::from_bytes(b"00000000000000000000000000000000-x").expect("valid store path")
}

/// Core regression scenario: an op that returns a trailing `1`, followed by a
/// second op on the *same* connection. Before the fix the second call failed
/// with "No discriminant in enum `RawLogMessageType` matches the value `1`".
#[tokio::test]
async fn add_temp_root_then_reuse_connection() -> Result<()> {
    let daemon = spawn_nix_daemon().await?;
    let mut client = DaemonClient::builder()
        .set_store_dir(&daemon.store_dir)
        .connect_unix(&daemon.socket)
        .await?;

    let path = dummy_path();

    client.add_temp_root(&path).await?;
    // Second request must still parse cleanly.
    let valid = client.is_valid_path(&path).await?;
    assert!(!valid, "dummy path should not be valid in fresh store");

    // Exercise the other ops that share the same wire shape so we don't
    // regress them individually.
    client.add_temp_root(&path).await?;
    client.add_temp_root(&path).await?;
    client.optimise_store().await?;
    let _ = client.is_valid_path(&path).await?;

    Ok(())
}
