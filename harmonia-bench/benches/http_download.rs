mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Scratch buffer for draining response bodies. Large enough to amortise
/// per-read syscall overhead while staying well inside L2.
const DRAIN_BUF: usize = 64 * 1024;

/// A persistent HTTP/1.1 keep-alive connection.
///
/// Response bodies are drained into a fixed scratch buffer rather than
/// accumulated, so the client side contributes only socket reads to the
/// measured wall-clock and the benchmark reflects server throughput.
struct Conn {
    reader: BufReader<TcpStream>,
    host: String,
    drain: Box<[u8; DRAIN_BUF]>,
}

impl Conn {
    async fn connect(addr: &str) -> Self {
        let stream = TcpStream::connect(addr).await.expect("connect failed");
        stream.set_nodelay(true).ok();
        Self {
            reader: BufReader::new(stream),
            host: addr.to_string(),
            drain: Box::new([0u8; DRAIN_BUF]),
        }
    }

    /// Issue a GET and parse status + Content-Length, leaving the body unread.
    async fn get_head(&mut self, path: &str) -> (u16, u64) {
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\n\r\n",
            path, self.host
        );
        self.reader
            .get_mut()
            .write_all(request.as_bytes())
            .await
            .expect("write failed");

        let mut status_line = String::new();
        self.reader
            .read_line(&mut status_line)
            .await
            .expect("read status");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .expect("bad status line");

        let mut content_length: Option<u64> = None;
        let mut line = String::new();
        loop {
            line.clear();
            self.reader.read_line(&mut line).await.expect("read header");
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some(val) = line
                .strip_prefix("Content-Length: ")
                .or_else(|| line.strip_prefix("content-length: "))
            {
                content_length = Some(val.trim().parse().expect("bad content-length"));
            }
        }
        // The keep-alive connection relies on draining exactly the declared
        // body length; fail loudly rather than desync on the next request.
        (
            status,
            content_length.expect("response missing Content-Length"),
        )
    }

    /// GET `path` and return the full body (for small responses like narinfo).
    async fn get_body(&mut self, path: &str) -> (u16, Vec<u8>) {
        let (status, len) = self.get_head(path).await;
        let mut body = vec![0u8; len as usize];
        self.reader.read_exact(&mut body).await.expect("read body");
        (status, body)
    }

    /// GET `path` and discard the body, returning the number of bytes drained.
    async fn get_drain(&mut self, path: &str) -> (u16, u64) {
        let (status, len) = self.get_head(path).await;
        let mut remaining = len;
        while remaining > 0 {
            let want = remaining.min(DRAIN_BUF as u64) as usize;
            let n = self
                .reader
                .read(&mut self.drain[..want])
                .await
                .expect("read body");
            assert!(n > 0, "unexpected EOF with {} bytes remaining", remaining);
            remaining -= n as u64;
        }
        (status, len)
    }
}

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

async fn fetch_narinfo(conn: &mut Conn, hash: &str) -> NarInfo {
    let path = format!("/{}.narinfo", hash);
    let (status, body) = conn.get_body(&path).await;
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

async fn download_nar(conn: &mut Conn, nar_url: &str) -> u64 {
    let path = format!("/{}", nar_url);
    let (status, len) = conn.get_drain(&path).await;
    assert!(
        (200..300).contains(&status),
        "NAR {}: status {}",
        path,
        status
    );
    len
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
        let mut conn = Conn::connect(&addr).await;
        let mut results = Vec::new();
        for path in &paths {
            let hash = store_path_hash(path);
            let narinfo = fetch_narinfo(&mut conn, &hash).await;
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
        // One persistent connection reused across all iterations so we measure
        // server-side NAR throughput, not TCP handshake latency.
        let mut conn = rt.block_on(Conn::connect(&addr));
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                rt.block_on(async {
                    for (_, narinfo) in &narinfos {
                        download_nar(&mut conn, &narinfo.url).await;
                    }
                });
                total += start.elapsed();
            }
            total
        })
    });

    let nar_urls: std::sync::Arc<Vec<String>> =
        std::sync::Arc::new(narinfos.iter().map(|(_, ni)| ni.url.clone()).collect());

    for concurrency in [4, 16] {
        group.bench_function(format!("concurrent_{}", concurrency), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let start = Instant::now();
                    rt.block_on(async {
                        // One persistent connection per worker. NAR sizes are
                        // heavily skewed, so workers pull from a shared atomic
                        // cursor to stay busy until the queue drains.
                        let next = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                        let mut handles = Vec::with_capacity(concurrency);
                        for _ in 0..concurrency {
                            let addr = addr.clone();
                            let urls = nar_urls.clone();
                            let next = next.clone();
                            handles.push(tokio::spawn(async move {
                                let mut conn = Conn::connect(&addr).await;
                                let mut bytes = 0u64;
                                loop {
                                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    let Some(url) = urls.get(i) else { break };
                                    bytes += download_nar(&mut conn, url).await;
                                }
                                bytes
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
