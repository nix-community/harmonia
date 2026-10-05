use std::path::PathBuf;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use futures_util::{StreamExt as _, TryStreamExt as _};
use harmonia_file_nar::{NarByteStream, NarEvent, NarWriteError, parse_nar, restore};
use tempfile::tempdir;

async fn dump_nar(path: PathBuf) -> Bytes {
    let mut nar = BytesMut::new();
    let mut stream = NarByteStream::new(path);
    while let Some(chunk) = stream.next().await {
        nar.extend_from_slice(&chunk.expect("dump failed"));
    }
    nar.freeze()
}

fn benchmark_nar(c: &mut Criterion) {
    let closure = harmonia_bench::build_closure();
    let paths: Vec<PathBuf> = harmonia_bench::closure_paths(&closure)
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Keep the NARs in memory so the restore benchmark measures the restorer
    // and not the source store's page cache or disk.
    let nars: Vec<Bytes> = rt.block_on(async {
        let mut nars = Vec::new();
        for path in &paths {
            nars.push(dump_nar(path.clone()).await);
        }
        nars
    });
    let total: u64 = nars.iter().map(|n| n.len() as u64).sum();
    eprintln!(
        "{} NARs, {:.1} MiB",
        nars.len(),
        total as f64 / 1024.0 / 1024.0
    );

    let mut group = harmonia_bench::slow_group(c, "nar");
    group.throughput(Throughput::Bytes(total));

    group.bench_function("dump", |b| {
        b.iter(|| {
            rt.block_on(async {
                for path in &paths {
                    let mut stream = NarByteStream::new(path.clone());
                    while let Some(chunk) = stream.next().await {
                        chunk.expect("dump failed");
                    }
                }
            })
        })
    });

    group.bench_function("parse", |b| {
        b.iter(|| {
            rt.block_on(async {
                for nar in &nars {
                    parse_nar(&nar[..])
                        .try_for_each(|event| async move {
                            if let NarEvent::File { mut reader, .. } = event {
                                tokio::io::copy(&mut reader, &mut tokio::io::sink()).await?;
                            }
                            Ok(())
                        })
                        .await
                        .expect("parse failed");
                }
            })
        })
    });

    group.bench_function("restore", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let dir = tempdir().unwrap();
                let start = Instant::now();
                rt.block_on(async {
                    for (i, nar) in nars.iter().enumerate() {
                        let target = dir.path().join(i.to_string());
                        let events = parse_nar(&nar[..]).map_err(|e| {
                            NarWriteError::create_file_error(PathBuf::from("<nar>"), e)
                        });
                        restore(events, target).await.expect("restore failed");
                    }
                });
                total += start.elapsed();
            }
            total
        })
    });

    group.finish();
}

criterion_group!(benches, benchmark_nar);
criterion_main!(benches);
