use criterion::{Criterion, criterion_group, criterion_main};
use std::io::Write;
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use tempfile::{NamedTempFile, tempdir};
use tokio::time::{sleep, timeout};

// --- Server utilities ---

/// Guard that terminates the harmonia process on drop
struct HarmoniaGuard {
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

            // Wait up to 5 seconds for graceful shutdown
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

/// Pick an unused port on localhost
fn pick_unused_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()
        .map(|a| a.port())
}

/// Start harmonia-cache server and wait for it to be ready.
async fn start_harmonia(bin_path: &str) -> (u16, HarmoniaGuard) {
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

// --- Build utilities ---

/// Build a flake output and return the store path
fn nix_build(flake_ref: &str) -> String {
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

/// Build harmonia-cache using cargo and return the binary path
fn cargo_build_harmonia() -> String {
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

// --- Benchmark ---

fn benchmark_closure_download(c: &mut Criterion) {
    // Build harmonia: use nix if HARMONIA_FLAKE is set, otherwise cargo build
    let harmonia_bin = if let Ok(flake) = std::env::var("HARMONIA_FLAKE") {
        eprintln!("Building harmonia from {}...", flake);
        let harmonia_path = nix_build(&flake);
        let bin = format!("{}/bin/harmonia-cache", harmonia_path);
        eprintln!("Harmonia built: {}", bin);
        bin
    } else {
        let bin = cargo_build_harmonia();
        eprintln!("Harmonia built: {}", bin);
        bin
    };

    // Build benchmark closure (Python with packages, fetches from cache.nixos.org)
    let closure_flake =
        std::env::var("BENCH_CLOSURE_FLAKE").unwrap_or_else(|_| ".#bench-closure".to_string());
    eprintln!("Building benchmark closure from {}...", closure_flake);
    let closure_path = nix_build(&closure_flake);
    eprintln!("Closure built: {}", closure_path);

    // Start harmonia server
    eprintln!("Starting harmonia server...");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (port, _guard) = rt.block_on(start_harmonia(&harmonia_bin));
    eprintln!("Harmonia server running on port {}", port);

    let mut group = c.benchmark_group("closure");
    // Downloading closure takes a while, adjust timing
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(60));

    group.bench_function("download", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                // Create fresh temp store for each iteration
                // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
                let temp = tempdir().unwrap();
                let temp_canonical = temp.path().canonicalize().unwrap();
                let store_path = temp_canonical.join("store");
                let state_path = temp_canonical.join("state");
                let store = format!(
                    "local?store={}&state={}",
                    store_path.display(),
                    state_path.display()
                );

                // Initialize store
                let init_output = Command::new("nix-store")
                    .args(["--init", "--store", &store])
                    .env_remove("NIX_REMOTE")
                    .output()
                    .expect("failed to run nix-store --init");
                assert!(
                    init_output.status.success(),
                    "nix-store --init failed with status {}: {}",
                    init_output.status,
                    String::from_utf8_lossy(&init_output.stderr)
                );

                // Time the download
                let start = Instant::now();
                let status = Command::new("nix")
                    .args([
                        "--extra-experimental-features",
                        "nix-command",
                        "copy",
                        "--no-check-sigs",
                        "--from",
                        &format!("http://127.0.0.1:{}", port),
                        "--to",
                        &store,
                        &closure_path,
                    ])
                    .env_remove("NIX_REMOTE")
                    .status()
                    .unwrap();
                assert!(status.success(), "nix copy failed");
                total += start.elapsed();
            }
            total
        })
    });

    group.finish();
}

criterion_group!(benches, benchmark_closure_download);
criterion_main!(benches);
