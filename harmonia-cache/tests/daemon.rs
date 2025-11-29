// Rust doesn't see that this is used in test binaries, so we need to allow dead code
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::time::{sleep, timeout};

pub use harmonia_utils_test::CanonicalTempDir;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Configuration for a daemon instance
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub store_dir: PathBuf,
    pub state_dir: PathBuf,
}

/// A running daemon instance
pub struct DaemonInstance {
    pub socket_path: PathBuf,
    pub store_dir: PathBuf,
    pub state_dir: PathBuf,
    _guard: Box<dyn Send>,
}

/// Trait for different daemon implementations
#[allow(async_fn_in_trait)]
pub trait Daemon: Send {
    /// Start the daemon with the given configuration
    async fn start(config: DaemonConfig) -> Result<DaemonInstance>;
}

/// Nix daemon implementation
pub struct NixDaemon;

impl Daemon for NixDaemon {
    async fn start(config: DaemonConfig) -> Result<DaemonInstance> {
        // Initialize the store
        let output = Command::new("nix-store")
            .args([
                "--init",
                "--store",
                &format!(
                    "local?store={}&state={}",
                    config.store_dir.display(),
                    config.state_dir.display()
                ),
            ])
            .output()?;

        if !output.status.success() {
            return Err(format!(
                "Failed to init store: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        // Start nix-daemon
        let mut cmd = Command::new("nix-daemon");
        cmd.env("NIX_STORE_DIR", &config.store_dir)
            .env("NIX_STATE_DIR", &config.state_dir)
            .env("NIX_LOG_DIR", config.state_dir.join("log"))
            .env("NIX_CONF_DIR", config.state_dir.join("etc"))
            .env("NIX_DAEMON_SOCKET_PATH", &config.socket_path)
            .env_remove("NIX_REMOTE")
            .env("NIX_CONFIG", "trusted-public-keys = cache.example.com-1:it/0WfLNR/PeSfxpCjB/tz8l5CmNr3F8hYBS0WWPVYHA== cache2.example.com-1:d/q03/F+ihXa1IGKwQ6hzUc3YQ3cSEyb5GO1N1NDFQ0=");

        println!(
            "Starting nix-daemon with socket: {}",
            config.socket_path.display()
        );
        let child = cmd.spawn()?;
        let guard = Box::new(ProcessGuard::new(child));

        Ok(DaemonInstance {
            socket_path: config.socket_path,
            store_dir: config.store_dir,
            state_dir: config.state_dir,
            _guard: guard,
        })
    }
}

/// Harmonia daemon implementation
pub struct HarmoniaDaemon;

impl Daemon for HarmoniaDaemon {
    async fn start(config: DaemonConfig) -> Result<DaemonInstance> {
        // Initialize the store
        let output = Command::new("nix-store")
            .args([
                "--init",
                "--store",
                &format!(
                    "local?store={}&state={}",
                    config.store_dir.display(),
                    config.state_dir.display()
                ),
            ])
            .output()?;

        if !output.status.success() {
            return Err(format!(
                "Failed to init store: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        // Create harmonia-daemon config
        let daemon_config = format!(
            r#"
socket_path = "{}"
store_dir = "{}"
db_path = "{}"
log_level = "debug"
"#,
            config.socket_path.display(),
            config.store_dir.display(),
            config.state_dir.join("db/db.sqlite").display(),
        );

        let config_file = write_toml_config(&daemon_config)?;
        let config_path = config_file.path().to_path_buf();

        // Start harmonia-daemon
        // Check if we have a built binary in the environment (from Nix build)
        // Otherwise fall back to cargo run for local development
        let child = if let Ok(harmonia_bin) = std::env::var("HARMONIA_BIN") {
            Command::new(format!("{harmonia_bin}/harmonia-daemon"))
                .env("HARMONIA_DAEMON_CONFIG", &config_path)
                .spawn()?
        } else {
            Command::new("cargo")
                .args(["run", "-p", "harmonia-daemon", "--"])
                .env("HARMONIA_DAEMON_CONFIG", &config_path)
                .spawn()?
        };

        let pid = child.id();
        println!(
            "Starting harmonia-daemon with socket: {} (PID: {pid})",
            config.socket_path.display()
        );

        // Create a guard that owns the config file
        let guard = Box::new(ProcessAndFileGuard {
            _process: ProcessGuard::new(child),
            _config_file: config_file,
        });

        // Wait for socket to be created
        wait_for_socket(&config.socket_path, pid, Duration::from_secs(120)).await?;

        Ok(DaemonInstance {
            socket_path: config.socket_path,
            store_dir: config.store_dir,
            state_dir: config.state_dir,
            _guard: guard,
        })
    }
}

/// Pick an unused port on localhost
pub fn pick_unused_port() -> Option<u16> {
    use std::net::TcpListener;

    // Bind to 127.0.0.1:0 - the OS will assign an available port
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;

    // Get the actual port that was assigned
    let port = listener.local_addr().ok()?.port();

    // Drop the listener to free the port
    drop(listener);

    Some(port)
}

/// Helper to start harmonia-cache server
pub async fn start_harmonia_cache(config: &str, port: u16) -> Result<Box<dyn Send>> {
    let config_file = write_toml_config(config)?;
    let config_path = config_file.path().to_path_buf();

    // Check if we have a built binary in the environment (from Nix build)
    // Otherwise fall back to cargo run for local development
    let cache_process = if let Ok(harmonia_bin) = std::env::var("HARMONIA_BIN") {
        Command::new(format!("{harmonia_bin}/harmonia-cache"))
            .env("CONFIG_FILE", &config_path)
            .env("RUST_LOG", "debug")
            .spawn()?
    } else {
        Command::new("cargo")
            .args(["run", "-p", "harmonia-cache", "--"])
            .env("CONFIG_FILE", &config_path)
            .env("RUST_LOG", "debug")
            .spawn()?
    };

    let pid = cache_process.id();
    println!("Started harmonia-cache process with PID: {pid}");

    let guard = Box::new(ProcessAndFileGuard {
        _process: ProcessGuard::new(cache_process),
        _config_file: config_file,
    });

    // Wait for HTTP server to be ready
    // For Unix sockets, we don't need to wait for a TCP port
    if port > 0 {
        wait_for_service("127.0.0.1", port, pid, Duration::from_secs(30)).await?;
    }

    Ok(guard)
}

// Helper functions

async fn wait_for_service(
    host: &str,
    port: u16,
    pid: u32,
    timeout_duration: Duration,
) -> Result<()> {
    println!("Waiting for service (PID {pid}) to start on {host}:{port}");
    let start = std::time::Instant::now();

    timeout(timeout_duration, async {
        let mut attempt = 0;
        loop {
            attempt += 1;

            // First check if the process is still running
            // Try to send signal 0 to check if process exists
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;

            if kill(Pid::from_raw(pid as i32), Signal::SIGCONT).is_err() {
                return Err(
                    format!("Process {pid} died while waiting for service to start").into(),
                );
            }

            match tokio::net::TcpStream::connect((host, port)).await {
                Ok(_) => {
                    println!(
                        "Service is ready on {}:{} after {} attempts ({:.2}s)",
                        host,
                        port,
                        attempt,
                        start.elapsed().as_secs_f32()
                    );
                    return Ok(());
                }
                Err(e) => {
                    if attempt % 10 == 0 {
                        println!("Still waiting for {host}:{port} (attempt {attempt}, error: {e})");
                    }
                    sleep(Duration::from_millis(100)).await
                }
            }
        }
    })
    .await
    .map_err(|_| -> Box<dyn std::error::Error> {
        format!(
            "Timeout waiting for service (PID {pid}) on {host}:{port} after {timeout_duration:?}"
        )
        .into()
    })?
}

async fn wait_for_socket(
    socket_path: &std::path::Path,
    pid: u32,
    timeout_duration: Duration,
) -> Result<()> {
    println!(
        "Waiting for socket (PID {pid}) at {}",
        socket_path.display()
    );
    let start = std::time::Instant::now();

    timeout(timeout_duration, async {
        let mut attempt = 0;
        loop {
            attempt += 1;

            // First check if the process is still running
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;

            if kill(Pid::from_raw(pid as i32), Signal::SIGCONT).is_err() {
                return Err(
                    format!("Process {pid} died while waiting for socket to be created").into(),
                );
            }

            if socket_path.exists() {
                println!(
                    "Socket is ready at {} after {} attempts ({:.2}s)",
                    socket_path.display(),
                    attempt,
                    start.elapsed().as_secs_f32()
                );
                return Ok(());
            }

            if attempt % 10 == 0 {
                println!(
                    "Still waiting for socket {} (attempt {attempt})",
                    socket_path.display()
                );
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| -> Box<dyn std::error::Error> {
        format!(
            "Timeout waiting for socket (PID {pid}) at {} after {timeout_duration:?}",
            socket_path.display()
        )
        .into()
    })?
}

fn write_toml_config(content: &str) -> Result<NamedTempFile> {
    use std::io::Write;
    let mut file = NamedTempFile::new()?;
    write!(file, "{content}")?;
    file.flush()?;
    Ok(file)
}

// Process guard implementations

struct ProcessGuard {
    child: Option<Child>,
}

impl ProcessGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct ProcessAndFileGuard {
    _process: ProcessGuard,
    _config_file: NamedTempFile,
}

impl Drop for ProcessAndFileGuard {
    fn drop(&mut self) {
        // ProcessGuard's drop will be called automatically
    }
}
