use criterion::{Criterion, criterion_group, criterion_main};
use harmonia_bench::start_harmonia;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::tempdir;

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

    // Find the workspace root by looking for Cargo.lock
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
