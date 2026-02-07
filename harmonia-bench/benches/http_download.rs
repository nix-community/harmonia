mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Get all store paths in the closure of a path.
fn get_closure_paths(store_path: &str) -> Vec<String> {
    let output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "path-info",
            "--recursive",
            store_path,
        ])
        .output()
        .expect("nix path-info failed");
    assert!(
        output.status.success(),
        "nix path-info failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Extract the hash part from a store path (e.g., `/nix/store/abc123-name` -> `abc123`).
fn store_path_hash(path: &str) -> String {
    let basename = path.rsplit('/').next().unwrap();
    basename.split('-').next().unwrap().to_string()
}

/// Parsed narinfo with the fields we need to construct the NAR URL.
struct NarInfo {
    url: String,
    nar_size: u64,
}

/// Send an HTTP/1.1 GET request and return (status_code, headers, body_bytes).
async fn http_get(addr: &str, path: &str) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).await.expect("connect failed");
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, addr
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write failed");

    let mut reader = BufReader::new(stream);

    // Parse status line
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .await
        .expect("read status");
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("bad status line");

    // Parse headers
    let mut content_length: Option<u64> = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read header");
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(val) = line
            .strip_prefix("Content-Length: ")
            .or_else(|| line.strip_prefix("content-length: "))
        {
            content_length = val.trim().parse().ok();
        }
    }

    // Read body
    let mut body = Vec::new();
    if let Some(len) = content_length {
        body.resize(len as usize, 0);
        reader.read_exact(&mut body).await.expect("read body");
    } else {
        reader.read_to_end(&mut body).await.expect("read body");
    }

    (status_code, body)
}

async fn fetch_narinfo(addr: &str, hash: &str) -> NarInfo {
    let path = format!("/{}.narinfo", hash);
    let (status, body) = http_get(addr, &path).await;
    assert!(
        (200..300).contains(&status),
        "narinfo {}: status {}",
        path,
        status
    );

    let text = String::from_utf8(body).expect("narinfo not utf8");
    let mut nar_url = None;
    let mut nar_size = 0u64;

    for line in text.lines() {
        if let Some(val) = line.strip_prefix("URL: ") {
            nar_url = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("NarSize: ") {
            nar_size = val.trim().parse().unwrap_or(0);
        }
    }

    NarInfo {
        url: nar_url.expect("narinfo missing URL field"),
        nar_size,
    }
}

async fn download_nar(addr: &str, nar_url: &str) -> u64 {
    let path = format!("/{}", nar_url);
    let (status, body) = http_get(addr, &path).await;
    assert!(
        (200..300).contains(&status),
        "NAR {}: status {}",
        path,
        status
    );
    body.len() as u64
}

fn benchmark_http_download(c: &mut Criterion) {
    let harmonia_bin = common::build_harmonia();
    let closure_path = common::build_closure();

    let paths = get_closure_paths(&closure_path);
    eprintln!("Closure has {} store paths", paths.len());

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (port, _guard) = rt.block_on(common::start_harmonia(&harmonia_bin));
    let addr = format!("127.0.0.1:{}", port);
    eprintln!("Harmonia server running on {}", addr);

    // Pre-fetch all narinfos
    let narinfos: Vec<(String, NarInfo)> = rt.block_on(async {
        let mut results = Vec::new();
        for path in &paths {
            let hash = store_path_hash(path);
            let narinfo = fetch_narinfo(&addr, &hash).await;
            results.push((path.clone(), narinfo));
        }
        results
    });

    let total_nar_bytes: u64 = narinfos.iter().map(|(_, ni)| ni.nar_size).sum();
    eprintln!(
        "Total NAR size: {:.2} MiB across {} paths",
        total_nar_bytes as f64 / 1024.0 / 1024.0,
        narinfos.len()
    );

    let mut group = c.benchmark_group("http");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("sequential", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                rt.block_on(async {
                    for (_, narinfo) in &narinfos {
                        download_nar(&addr, &narinfo.url).await;
                    }
                });
                total += start.elapsed();
            }
            total
        })
    });

    for concurrency in [4, 16] {
        group.bench_function(format!("concurrent_{}", concurrency), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let start = Instant::now();
                    rt.block_on(async {
                        let semaphore =
                            std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
                        let mut handles = Vec::new();

                        for (_, narinfo) in &narinfos {
                            let addr = addr.clone();
                            let nar_url = narinfo.url.clone();
                            let sem = semaphore.clone();
                            handles.push(tokio::spawn(async move {
                                let _permit = sem.acquire().await.unwrap();
                                download_nar(&addr, &nar_url).await
                            }));
                        }
                        for handle in handles {
                            handle.await.unwrap();
                        }
                    });
                    total += start.elapsed();
                }
                total
            })
        });
    }

    group.finish();
}

criterion_group!(benches, benchmark_http_download);
criterion_main!(benches);
