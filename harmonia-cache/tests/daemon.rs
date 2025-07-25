use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::time::{sleep, timeout};

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
    // actually read by tests
    #[allow(dead_code)]
    pub store_dir: PathBuf,
    // actually read by tests
    #[allow(dead_code)]
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
        let child = Command::new("cargo")
            .args(["run", "-p", "harmonia-daemon", "--"])
            .env("HARMONIA_DAEMON_CONFIG", &config_path)
            .spawn()?;

        // Create a guard that owns the config file
        let guard = Box::new(ProcessAndFileGuard {
            _process: ProcessGuard::new(child),
            _config_file: config_file,
        });

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

    let cache_process = Command::new("cargo")
        .args(["run", "-p", "harmonia-cache", "--"])
        .env("CONFIG_FILE", &config_path)
        .spawn()?;

    let guard = Box::new(ProcessAndFileGuard {
        _process: ProcessGuard::new(cache_process),
        _config_file: config_file,
    });

    // Wait for HTTP server to be ready
    wait_for_port("127.0.0.1", port, Duration::from_secs(10)).await?;

    Ok(guard)
}

// Helper functions

async fn wait_for_port(host: &str, port: u16, timeout_duration: Duration) -> Result<()> {
    timeout(timeout_duration, async {
        loop {
            match tokio::net::TcpStream::connect((host, port)).await {
                Ok(_) => return Ok(()),
                Err(_) => sleep(Duration::from_millis(100)).await,
            }
        }
    })
    .await
    .map_err(|_| -> Box<dyn std::error::Error> {
        format!("Timeout waiting for port {}:{}", host, port).into()
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
