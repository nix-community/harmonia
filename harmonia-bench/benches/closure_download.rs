mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn benchmark_closure_download(c: &mut Criterion) {
    let harmonia_bin = common::build_harmonia();
    let closure_path = common::build_closure();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (port, _guard) = rt.block_on(common::start_harmonia(&harmonia_bin));
    eprintln!("Harmonia server running on port {}", port);

    let mut group = c.benchmark_group("closure");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("download", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let temp = tempdir().unwrap();
                let temp_canonical = temp.path().canonicalize().unwrap();
                let store_path = temp_canonical.join("store");
                let state_path = temp_canonical.join("state");
                let store = format!(
                    "local?store={}&state={}",
                    store_path.display(),
                    state_path.display()
                );

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
