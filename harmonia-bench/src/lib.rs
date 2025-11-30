//! Benchmark utilities for harmonia-cache performance testing.

use std::io::Write;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::time::{sleep, timeout};

/// Guard that terminates the harmonia process on drop
pub struct HarmoniaGuard {
    child: Option<Child>,
    _config_file: NamedTempFile,
}

impl Drop for HarmoniaGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Use SIGTERM for graceful shutdown
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;

            let pid = Pid::from_raw(child.id() as i32);
            let _ = kill(pid, Signal::SIGTERM);

            // Wait up to 5 seconds for graceful shutdown
            for _ in 0..50 {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(_) => break,
                }
            }

            // Force kill if still running
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Pick an unused port on localhost
pub fn pick_unused_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()
        .map(|a| a.port())
}

/// Start harmonia-cache server and wait for it to be ready.
///
/// # Arguments
/// * `bin_path` - Path to the harmonia-cache binary
///
/// # Returns
/// Tuple of (port, guard). The guard will terminate the server on drop.
pub async fn start_harmonia(bin_path: &str) -> (u16, HarmoniaGuard) {
    let port = pick_unused_port().expect("no available port");

    let config = format!(
        r#"
bind = "127.0.0.1:{}"
priority = 30
"#,
        port
    );

    let mut config_file = NamedTempFile::new().expect("failed to create temp config file");
    write!(config_file, "{}", config).expect("failed to write config");
    config_file.flush().expect("failed to flush config");

    let child = Command::new(bin_path)
        .env("CONFIG_FILE", config_file.path())
        .env("RUST_LOG", "warn")
        .spawn()
        .expect("failed to start harmonia");

    let pid = child.id();

    let guard = HarmoniaGuard {
        child: Some(child),
        _config_file: config_file,
    };

    // Wait for server to be ready
    wait_for_port(port, pid, Duration::from_secs(30)).await;

    (port, guard)
}

async fn wait_for_port(port: u16, pid: u32, timeout_duration: Duration) {
    let result = timeout(timeout_duration, async {
        loop {
            // Check if process is still running
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;

            if kill(Pid::from_raw(pid as i32), Signal::SIGCONT).is_err() {
                panic!("harmonia process {} died while waiting for startup", pid);
            }

            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    if result.is_err() {
        panic!(
            "timeout waiting for harmonia on port {} after {:?}",
            port, timeout_duration
        );
    }
}
