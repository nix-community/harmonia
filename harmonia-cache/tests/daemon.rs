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
        // Use HARMONIA_DAEMON_BIN env var if set (for coverage), otherwise cargo run
        let child = if let Ok(bin_path) = std::env::var("HARMONIA_DAEMON_BIN") {
            Command::new(bin_path)
                .env("HARMONIA_DAEMON_CONFIG", &config_path)
                .spawn()?
        } else {
            // Fall back to cargo run for normal development
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

    // Start harmonia-cache
    // Use HARMONIA_CACHE_BIN env var if set (for coverage), otherwise cargo's built-in path
    let bin_path = std::env::var("HARMONIA_CACHE_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_harmonia-cache").to_string());
    let cache_process = Command::new(&bin_path)
        .env("CONFIG_FILE", &config_path)
        .env("RUST_LOG", "debug")
        .spawn()?;

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
            // Use SIGTERM for graceful shutdown to allow coverage data to be flushed
            // (SIGKILL would terminate immediately without flushing profraw data)
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;

            let pid = Pid::from_raw(child.id() as i32);
            let _ = kill(pid, Signal::SIGTERM);

            // Wait up to 5 seconds for graceful shutdown (actix needs time to clean up)
            for _ in 0..50 {
                match child.try_wait() {
                    Ok(Some(_)) => return, // Process exited gracefully
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                    Err(_) => break,
                }
            }

            // If still running after grace period, force kill
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

/// Builder for test cache instances
#[derive(Default)]
pub struct TestCacheBuilder {
    priority: Option<u32>,
    signing_keys: Vec<String>,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    daemon: Option<DaemonInstance>,
}

impl TestCacheBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn priority(mut self, priority: u32) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn signing_key(mut self, key: &str) -> Self {
        self.signing_keys.push(key.trim().to_string());
        self
    }

    pub fn tls(mut self, cert: &str, key: &str) -> Self {
        self.tls_cert = Some(cert.to_string());
        self.tls_key = Some(key.to_string());
        self
    }

    pub fn daemon(mut self, daemon: DaemonInstance) -> Self {
        self.daemon = Some(daemon);
        self
    }

    pub async fn build(self) -> Result<TestCache> {
        let temp_dir = CanonicalTempDir::new()?;
        let port = pick_unused_port().ok_or("No available ports")?;

        let (store_dir, daemon_socket) = if let Some(ref daemon) = self.daemon {
            (daemon.store_dir.clone(), Some(daemon.socket_path.clone()))
        } else {
            (temp_dir.path().join("store"), None)
        };

        // Write signing keys to temp files
        let mut key_files = Vec::new();
        for key in &self.signing_keys {
            let mut file = NamedTempFile::new()?;
            use std::io::Write;
            write!(file, "{key}")?;
            file.flush()?;
            key_files.push(file);
        }

        // Write TLS files if configured
        let tls_files = match (&self.tls_cert, &self.tls_key) {
            (Some(cert), Some(key)) => {
                let cert_path = temp_dir.path().join("tls-cert.pem");
                let key_path = temp_dir.path().join("tls-key.pem");
                std::fs::write(&cert_path, cert)?;
                std::fs::write(&key_path, key)?;
                Some((cert_path, key_path))
            }
            _ => None,
        };

        // Build config
        let mut config = format!(
            "bind = \"127.0.0.1:{port}\"\n\
             virtual_nix_store = \"{store}\"\n\
             real_nix_store = \"{store}\"\n",
            store = store_dir.display(),
        );

        if let Some(socket) = &daemon_socket {
            config.push_str(&format!("daemon_socket = \"{}\"\n", socket.display()));
        }

        if let Some(p) = self.priority {
            config.push_str(&format!("priority = {p}\n"));
        }

        if !key_files.is_empty() {
            let paths: Vec<_> = key_files
                .iter()
                .map(|f| format!("\"{}\"", f.path().display()))
                .collect();
            config.push_str(&format!("sign_key_paths = [{}]\n", paths.join(", ")));
        }

        if let Some((ref cert_path, ref key_path)) = tls_files {
            config.push_str(&format!(
                "tls_cert_path = \"{}\"\ntls_key_path = \"{}\"\n",
                cert_path.display(),
                key_path.display()
            ));
        }

        let guard = start_harmonia_cache(&config, port).await?;

        Ok(TestCache {
            port,
            tls: tls_files.is_some(),
            tls_cert_path: tls_files.map(|(c, _)| c),
            _temp_dir: temp_dir,
            _guard: guard,
            _key_files: key_files,
            _daemon: self.daemon,
        })
    }
}

/// A running test cache instance with helper methods
pub struct TestCache {
    pub port: u16,
    tls: bool,
    tls_cert_path: Option<PathBuf>,
    _temp_dir: CanonicalTempDir,
    _guard: Box<dyn Send>,
    _key_files: Vec<NamedTempFile>,
    _daemon: Option<DaemonInstance>,
}

impl TestCache {
    pub fn builder() -> TestCacheBuilder {
        TestCacheBuilder::new()
    }

    /// Start a minimal cache (no daemon, no keys)
    pub async fn start() -> Result<Self> {
        Self::builder().build().await
    }

    pub fn url(&self, path: &str) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://127.0.0.1:{}{path}", self.port)
    }

    fn curl_args(&self) -> Vec<&str> {
        let mut args = vec!["--fail", "--max-time", "5", "--silent"];
        if self.tls {
            args.push("--insecure");
        }
        args
    }

    pub fn curl(&self, path: &str) -> Result<String> {
        let url = self.url(path);
        let output = Command::new("curl")
            .args(self.curl_args())
            .arg(&url)
            .output()?;

        if !output.status.success() {
            return Err(format!(
                "Request to {} failed: {}",
                path,
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        Ok(String::from_utf8(output.stdout)?)
    }

    pub fn curl_with_headers(&self, path: &str) -> Result<String> {
        let url = self.url(path);
        let output = Command::new("curl")
            .args(self.curl_args())
            .arg("--include")
            .arg(&url)
            .output()?;

        if !output.status.success() {
            return Err(format!(
                "Request to {} failed: {}",
                path,
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        Ok(String::from_utf8(output.stdout)?)
    }

    /// Get the TLS certificate path (for curl --cacert)
    pub fn tls_cert_path(&self) -> Option<&PathBuf> {
        self.tls_cert_path.as_ref()
    }
}

/// Set up a custom Nix environment with all required directories and env vars.
/// Returns environment variables for nix commands.
pub fn setup_nix_env(root: &std::path::Path) -> Vec<(String, String)> {
    // Create required directories
    std::fs::create_dir_all(root.join("store")).expect("Failed to create store dir");
    std::fs::create_dir_all(root.join("var/log/nix/drvs")).expect("Failed to create log dir");
    std::fs::create_dir_all(root.join("var/nix/profiles")).expect("Failed to create profiles dir");
    std::fs::create_dir_all(root.join("var/nix/db")).expect("Failed to create db dir");
    std::fs::create_dir_all(root.join("etc")).expect("Failed to create etc dir");
    std::fs::create_dir_all(root.join("cache")).expect("Failed to create cache dir");

    vec![
        (
            "NIX_STORE_DIR".to_string(),
            root.join("store").to_string_lossy().to_string(),
        ),
        (
            "NIX_DATA_DIR".to_string(),
            root.join("share").to_string_lossy().to_string(),
        ),
        (
            "NIX_LOG_DIR".to_string(),
            root.join("var/log/nix").to_string_lossy().to_string(),
        ),
        (
            "NIX_STATE_DIR".to_string(),
            root.join("var/nix").to_string_lossy().to_string(),
        ),
        (
            "NIX_CONF_DIR".to_string(),
            root.join("etc").to_string_lossy().to_string(),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            root.join("cache").to_string_lossy().to_string(),
        ),
        (
            "NIX_CONFIG".to_string(),
            "substituters =\nconnect-timeout = 0\nsandbox = false".to_string(),
        ),
        ("_NIX_TEST_NO_SANDBOX".to_string(), "1".to_string()),
        ("NIX_REMOTE".to_string(), "".to_string()),
    ]
}

/// Build a simple derivation that outputs "hello" to the build log.
/// Returns the derivation filename (hash-name.drv).
pub fn build_hello_derivation(env_vars: &[(String, String)]) -> Result<String> {
    // Use nix-build with a simple derivation expression
    let expr = r#"
        derivation {
            name = "test-log";
            system = builtins.currentSystem;
            builder = "/bin/sh";
            args = ["-c" "echo hello; echo done > $out"];
        }
    "#;

    let mut cmd = Command::new("nix-build");
    cmd.args(["--expr", expr, "--no-out-link"]);

    // Clear NIX_REMOTE and set our custom env
    cmd.env_remove("NIX_REMOTE");
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let output = cmd.output()?;

    if !output.status.success() {
        return Err(format!(
            "nix-build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    // The output is the store path, but we need the .drv path
    // Use nix-store -qd to get the derivation
    let store_path = String::from_utf8(output.stdout)?.trim().to_string();

    let mut cmd = Command::new("nix-store");
    cmd.args(["-qd", &store_path]);
    cmd.env_remove("NIX_REMOTE");
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let output = cmd.output()?;
    if !output.status.success() {
        return Err(format!(
            "nix-store -qd failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let drv_path = String::from_utf8(output.stdout)?.trim().to_string();

    // Extract just the hash-name.drv part from the full path
    let drv_name = std::path::Path::new(&drv_path)
        .file_name()
        .ok_or("No filename in drv path")?
        .to_string_lossy()
        .to_string();

    Ok(drv_name)
}
