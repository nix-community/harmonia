use std::collections::BTreeSet;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use harmonia_store_path::StorePath;
use harmonia_store_ref_scan::RefScanSink;

const ALPHABET: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";

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

fn embed_hashes(data: &mut [u8], cands: &[StorePath], embed: usize, state: &mut u64) {
    for c in cands.iter().take(embed) {
        let hash = c.hash().to_string();
        let off = (xorshift(state) as usize) % (data.len() - hash.len());
        data[off..off + hash.len()].copy_from_slice(hash.as_bytes());
    }
}

/// A NAR-sized blob of mostly random binary data with a fraction of the
/// candidate hashes embedded at random offsets: the realistic worker case
/// where an output references a handful of its many build inputs.
fn make_blob(size: usize, cands: &[StorePath], embed: usize) -> Vec<u8> {
    let mut state = 0x1234_5678_9abc_def0;
    let mut data: Vec<u8> = (0..size)
        .map(|_| (xorshift(&mut state) & 0xff) as u8)
        .collect();
    embed_hashes(&mut data, cands, embed, &mut state);
    data
}

fn make_ascii_text(size: usize, cands: &[StorePath], embed: usize) -> Vec<u8> {
    let sample = b"pub fn parse_derivation(input: &[u8]) -> Result<Derivation, ParseError> { let path = '/nix/store/0123456789abcdfghijklmnpqrsvwxyz-example-1.0'; }\n";
    let mut data = Vec::with_capacity(size);
    while data.len() < size {
        let rem = (size - data.len()).min(sample.len());
        data.extend_from_slice(&sample[..rem]);
    }
    let mut state = 0xdead_beef_cafe_babe;
    embed_hashes(&mut data, cands, embed, &mut state);
    data
}

fn make_valid_runs(size: usize, cands: &[StorePath], embed: usize) -> Vec<u8> {
    let mut state = 0x5555_aaaa_1234_5678;
    let mut data: Vec<u8> = (0..size)
        .map(|_| ALPHABET[(xorshift(&mut state) % 32) as usize])
        .collect();
    embed_hashes(&mut data, cands, embed, &mut state);
    data
}

fn scan_chunked(data: &[u8], chunk: usize, cand_set: &BTreeSet<StorePath>) -> BTreeSet<StorePath> {
    let mut sink = RefScanSink::new(cand_set, None);
    for part in data.chunks(chunk) {
        sink.feed(part);
    }
    sink.found_paths()
}

fn benchmark_ref_scan(c: &mut Criterion) {
    const SIZE: usize = 16 * 1024 * 1024;
    let cands = candidates(512);
    let cand_set: BTreeSet<StorePath> = cands.iter().cloned().collect();
    let blob = make_blob(SIZE, &cands, 8);
    let text = make_ascii_text(SIZE, &cands, 8);
    let runs = make_valid_runs(1024 * 1024, &cands, 8);

    let mut group = c.benchmark_group("ref_scan");
    group.throughput(Throughput::Bytes(SIZE as u64));

    // Vary the chunk size to exercise the boundary/overlap path at different
    // frequencies (NAR streams arrive in fixed-size reads).
    for chunk in [4096usize, 65536, 1024 * 1024] {
        group.bench_with_input(BenchmarkId::new("chunk", chunk), &chunk, |b, &chunk| {
            b.iter(|| scan_chunked(&blob, chunk, &cand_set));
        });
    }

    group.bench_function("ascii_text_64k", |b| {
        b.iter(|| scan_chunked(&text, 65536, &cand_set));
    });

    group.throughput(Throughput::Bytes(runs.len() as u64));
    group.bench_function("valid_base32_1m", |b| {
        b.iter(|| scan_chunked(&runs, 65536, &cand_set));
    });

    group.finish();
}

criterion_group!(benches, benchmark_ref_scan);
criterion_main!(benches);
