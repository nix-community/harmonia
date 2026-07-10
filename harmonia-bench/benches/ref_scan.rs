use std::collections::BTreeSet;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use harmonia_store_path::StorePath;
use harmonia_store_ref_scan::RefScanSink;

/// Cheap deterministic PRNG so benchmark inputs are reproducible without a
/// dependency on `rand`.
fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Build `count` distinct candidate store paths.
fn candidates(count: usize) -> Vec<StorePath> {
    const ALPHABET: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";
    let mut state = 0x9e37_79b9_7f4a_7c15;
    (0..count)
        .map(|i| {
            let hash: String = (0..32)
                .map(|_| ALPHABET[(xorshift(&mut state) % 32) as usize] as char)
                .collect();
            StorePath::from_base_path(&format!("{hash}-candidate-{i}")).unwrap()
        })
        .collect()
}

/// A NAR-sized blob of mostly random binary data with a fraction of the
/// candidate hashes embedded at random offsets: the realistic worker case
/// where an output references a handful of its many build inputs.
fn make_blob(size: usize, cands: &[StorePath], embed: usize) -> Vec<u8> {
    let mut state = 0x1234_5678_9abc_def0;
    let mut data: Vec<u8> = (0..size)
        .map(|_| (xorshift(&mut state) & 0xff) as u8)
        .collect();
    for c in cands.iter().take(embed) {
        let hash = c.hash().to_string();
        let off = (xorshift(&mut state) as usize) % (size - hash.len());
        data[off..off + hash.len()].copy_from_slice(hash.as_bytes());
    }
    data
}

fn benchmark_ref_scan(c: &mut Criterion) {
    const SIZE: usize = 16 * 1024 * 1024;
    let cands = candidates(512);
    let cand_set: BTreeSet<StorePath> = cands.iter().cloned().collect();
    let blob = make_blob(SIZE, &cands, 8);

    let mut group = c.benchmark_group("ref_scan");
    group.throughput(Throughput::Bytes(SIZE as u64));

    // Vary the chunk size to exercise the boundary/overlap path at different
    // frequencies (NAR streams arrive in fixed-size reads).
    for chunk in [4096usize, 65536, 1024 * 1024] {
        group.bench_with_input(BenchmarkId::new("chunk", chunk), &chunk, |b, &chunk| {
            b.iter(|| {
                let mut sink = RefScanSink::new(&cand_set, None);
                for part in blob.chunks(chunk) {
                    sink.feed(part);
                }
                sink.found_paths()
            });
        });
    }

    group.finish();
}

criterion_group!(benches, benchmark_ref_scan);
criterion_main!(benches);
