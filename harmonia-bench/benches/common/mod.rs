use std::io::Write;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::time::{sleep, timeout};

/// Guard that terminates the harmonia process on drop.
pub struct HarmoniaGuard {
    child: Option<Child>,
    _config_file: NamedTempFile,
}

impl Drop for HarmoniaGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;

            let pid = Pid::from_raw(child.id() as i32);
            let _ = kill(pid, Signal::SIGTERM);

            for _ in 0..50 {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(_) => break,
                }
            }

            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn pick_unused_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()
        .map(|a| a.port())
}

/// Start harmonia-cache server and wait for it to be ready.
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

    wait_for_port(port, pid, Duration::from_secs(30)).await;

    (port, guard)
}

async fn wait_for_port(port: u16, pid: u32, timeout_duration: Duration) {
    let result = timeout(timeout_duration, async {
        loop {
            use nix::sys::signal::kill;
            use nix::unistd::Pid;

            if kill(Pid::from_raw(pid as i32), None).is_err() {
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

/// Build a flake output and return the store path.
pub fn nix_build(flake_ref: &str) -> String {
    let output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "build",
            "--no-link",
            "--print-out-paths",
            flake_ref,
        ])
        .output()
        .expect("nix build failed");
    assert!(
        output.status.success(),
        "nix build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// Build harmonia-cache using cargo and return the binary path.
pub fn cargo_build_harmonia() -> String {
    eprintln!("Building harmonia-cache with cargo (profiling profile)...");
    let status = Command::new("cargo")
        .args(["build", "--profile", "profiling", "-p", "harmonia-cache"])
        .status()
        .expect("cargo build failed");
    assert!(status.success(), "cargo build failed");

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = std::path::Path::new(manifest_dir)
        .parent()
        .expect("no parent dir");
    let bin_path = workspace_root.join("target/profiling/harmonia-cache");
    assert!(
        bin_path.exists(),
        "harmonia-cache binary not found at {:?}",
        bin_path
    );
    bin_path.to_string_lossy().to_string()
}

/// Build or locate the harmonia binary. Uses HARMONIA_FLAKE env var if set,
/// otherwise builds with cargo.
pub fn build_harmonia() -> String {
    if let Ok(flake) = std::env::var("HARMONIA_FLAKE") {
        eprintln!("Building harmonia from {}...", flake);
        let harmonia_path = nix_build(&flake);
        let bin = format!("{}/bin/harmonia-cache", harmonia_path);
        eprintln!("Harmonia built: {}", bin);
        bin
    } else {
        let bin = cargo_build_harmonia();
        eprintln!("Harmonia built: {}", bin);
        bin
    }
}

/// Build the benchmark closure and return its store path.
pub fn build_closure() -> String {
    let closure_flake =
        std::env::var("BENCH_CLOSURE_FLAKE").unwrap_or_else(|_| ".#bench-closure".to_string());
    eprintln!("Building benchmark closure from {}...", closure_flake);
    let closure_path = nix_build(&closure_flake);
    eprintln!("Closure built: {}", closure_path);
    closure_path
}
