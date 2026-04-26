#![allow(unsafe_code)]

use std::net::TcpListener;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::Duration;

use common::{CanonicalTempDir, LocalStore};
use nix::fcntl::{FcntlArg, FdFlag, fcntl};

mod common;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Emulate systemd: pass a pre-bound listener as fd 3 with `LISTEN_FDS=1`.
/// Config `bind` is unroutable so the test fails unless the inherited socket
/// is used. `LISTEN_PID` stays unset because `Command` fixes envp before fork.
#[tokio::test]
async fn test_socket_activation_tcp() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;
    let store = LocalStore::init(temp_dir.path())?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let listener = OwnedFd::from(listener);
    // std sockets are CLOEXEC; clear it so the fd survives exec.
    fcntl(&listener, FcntlArg::F_SETFD(FdFlag::empty()))?;
    let raw = listener.as_raw_fd();

    let config_file = common::write_toml_config(&format!(
        "bind = \"255.255.255.255:1\"\nnix_db_path = \"{}\"\npriority = 30\n",
        store.db_path().display(),
    ))?;

    let bin_path = std::env::var("HARMONIA_CACHE_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_harmonia-cache").to_string());
    let mut cmd = Command::new(&bin_path);
    cmd.env("CONFIG_FILE", config_file.path())
        .env("RUST_LOG", "debug")
        .env("LISTEN_FDS", "1");
    // SAFETY: only async-signal-safe syscalls (dup2/close) between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            let old = BorrowedFd::borrow_raw(raw);
            std::mem::forget(nix::unistd::dup2_raw(old, 3)?);
            if raw != 3 {
                nix::unistd::close(raw)?;
            }
            Ok(())
        });
    }
    let _guard = common::ProcessGuard::new(cmd.spawn()?);
    drop(listener);

    let url = format!("http://127.0.0.1:{port}/nix-cache-info");
    let start = std::time::Instant::now();
    let body = loop {
        let output = Command::new("curl")
            .args(["--fail", "--silent", "--max-time", "2", &url])
            .output()?;
        if output.status.success() {
            break String::from_utf8(output.stdout)?;
        }
        if start.elapsed() > Duration::from_secs(30) {
            return Err(format!("timeout waiting for {url}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(body.contains("StoreDir:"), "got: {body}");

    Ok(())
}
