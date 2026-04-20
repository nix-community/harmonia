mod common;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Scratch buffer for draining response bodies. Large enough to amortise
/// per-read syscall overhead while staying well inside L2.
const DRAIN_BUF: usize = 64 * 1024;

enum BodyFraming {
    Length(u64),
    Chunked,
}

/// A persistent HTTP/1.1 keep-alive connection.
///
/// Response bodies are drained into a fixed scratch buffer rather than
/// accumulated, so the client side contributes only socket reads to the
/// measured wall-clock and the benchmark reflects server throughput.
struct Conn {
    reader: BufReader<TcpStream>,
    host: String,
    drain: Box<[u8; DRAIN_BUF]>,
    accept_encoding: &'static str,
}

impl Conn {
    async fn connect(addr: &str) -> Self {
        Self::connect_with_encoding(addr, "identity").await
    }

    async fn connect_with_encoding(addr: &str, accept_encoding: &'static str) -> Self {
        let stream = TcpStream::connect(addr).await.expect("connect failed");
        stream.set_nodelay(true).ok();
        Self {
            reader: BufReader::new(stream),
            host: addr.to_string(),
            drain: Box::new([0u8; DRAIN_BUF]),
            accept_encoding,
        }
    }

    /// Issue a GET and parse status + framing, leaving the body unread.
    async fn get_head(&mut self, path: &str) -> (u16, BodyFraming) {
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nAccept-Encoding: {}\r\nConnection: keep-alive\r\n\r\n",
            path, self.host, self.accept_encoding
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
        let mut chunked = false;
        let mut line = String::new();
        loop {
            line.clear();
            self.reader.read_line(&mut line).await.expect("read header");
            if line == "\r\n" || line == "\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(val) = lower.strip_prefix("content-length: ") {
                content_length = Some(val.trim().parse().expect("bad content-length"));
            } else if lower
                .strip_prefix("transfer-encoding: ")
                .is_some_and(|v| v.trim() == "chunked")
            {
                chunked = true;
            }
        }
        // The keep-alive connection relies on draining exactly the declared
        // framing; fail loudly rather than desync on the next request.
        let framing = if chunked {
            BodyFraming::Chunked
        } else {
            BodyFraming::Length(content_length.expect("response missing Content-Length"))
        };
        (status, framing)
    }

    async fn drain_exact(&mut self, mut remaining: u64) {
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
    }

    /// Drain a chunked-encoded body and return total payload bytes.
    async fn drain_chunked(&mut self) -> u64 {
        let mut total = 0u64;
        let mut line = String::new();
        loop {
            line.clear();
            self.reader
                .read_line(&mut line)
                .await
                .expect("read chunk size");
            let size_str = line.trim_end();
            // Ignore chunk extensions after ';'
            let size_hex = size_str.split(';').next().unwrap();
            let size = u64::from_str_radix(size_hex, 16)
                .unwrap_or_else(|_| panic!("bad chunk size: {line:?}"));
            if size == 0 {
                // trailer + final CRLF
                loop {
                    line.clear();
                    self.reader
                        .read_line(&mut line)
                        .await
                        .expect("read trailer");
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                }
                return total;
            }
            self.drain_exact(size).await;
            total += size;
            // CRLF after chunk data
            line.clear();
            self.reader
                .read_line(&mut line)
                .await
                .expect("read chunk CRLF");
        }
    }

    /// GET `path` and return the full body (for small responses like narinfo).
    async fn get_body(&mut self, path: &str) -> (u16, Vec<u8>) {
        let (status, framing) = self.get_head(path).await;
        let BodyFraming::Length(len) = framing else {
            panic!("get_body requires Content-Length")
        };
        let mut body = vec![0u8; len as usize];
        self.reader.read_exact(&mut body).await.expect("read body");
        (status, body)
    }

    /// GET `path` and discard the body, returning the number of bytes drained.
    async fn get_drain(&mut self, path: &str) -> (u16, u64) {
        let (status, framing) = self.get_head(path).await;
        let len = match framing {
            BodyFraming::Length(len) => {
                self.drain_exact(len).await;
                len
            }
            BodyFraming::Chunked => self.drain_chunked().await,
        };
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

    // One-off wire-size probe per encoding, printed alongside the timing runs.
    rt.block_on(async {
        for encoding in ["identity", "zstd"] {
            let mut conn = Conn::connect_with_encoding(&addr, encoding).await;
            let mut wire = 0u64;
            for (_, ni) in &narinfos {
                wire += download_nar(&mut conn, &ni.url).await;
            }
            eprintln!(
                "encoding={:<8} wire={:>9.2} MiB  ratio={:.3}",
                encoding,
                wire as f64 / 1024.0 / 1024.0,
                wire as f64 / total_nar_bytes as f64,
            );
        }
    });

    // narinfo latency: sequential GET of every <hash>.narinfo over a single
    // keep-alive connection. The pre-fetch loop above doubles as warm-up.
    {
        let hashes: Vec<String> = paths.iter().map(|p| store_path_hash(p)).collect();
        let mut group = c.benchmark_group("narinfo");
        group.sample_size(10);
        group.throughput(Throughput::Elements(hashes.len() as u64));
        group.bench_function("all", |b| {
            let mut conn = rt.block_on(Conn::connect(&addr));
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let start = Instant::now();
                    rt.block_on(async {
                        for hash in &hashes {
                            let (status, _) = conn.get_body(&format!("/{hash}.narinfo")).await;
                            assert_eq!(status, 200);
                        }
                    });
                    total += start.elapsed();
                }
                total
            })
        });
        group.finish();
    }

    let mut group = c.benchmark_group("http");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(5));

    for encoding in ["identity", "zstd"] {
        group.bench_function(format!("sequential_{encoding}"), |b| {
            let mut conn = rt.block_on(Conn::connect_with_encoding(&addr, encoding));
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
    }

    let nar_urls: std::sync::Arc<Vec<String>> =
        std::sync::Arc::new(narinfos.iter().map(|(_, ni)| ni.url.clone()).collect());

    for (concurrency, encoding) in [(4, "identity"), (16, "identity"), (4, "zstd"), (16, "zstd")] {
        group.bench_function(format!("concurrent_{concurrency}_{encoding}"), |b| {
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
                                let mut conn = Conn::connect_with_encoding(&addr, encoding).await;
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
