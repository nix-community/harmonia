use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::hash::{BuildHasher as _, RandomState};
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use harmonia_store_path::{StoreDir, StorePath};

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn base_paths(count: usize) -> Vec<String> {
    const ALPHABET: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";
    let mut state = 0x9e37_79b9_7f4a_7c15;
    (0..count)
        .map(|i| {
            let hash: String = (0..32)
                .map(|_| ALPHABET[(xorshift(&mut state) % 32) as usize] as char)
                .collect();
            format!("{hash}-python3.13-package-{i}")
        })
        .collect()
}

fn benchmark_store_path(c: &mut Criterion) {
    let bases = base_paths(3000);
    let paths: Vec<StorePath> = bases
        .iter()
        .map(|s| StorePath::from_base_path(s).unwrap())
        .collect();
    let store_dir = StoreDir::default();
    let mut group = c.benchmark_group("store_path");

    group.bench_function("parse", |b| {
        b.iter(|| StorePath::from_base_path(black_box(&bases[0])).unwrap())
    });

    let set: BTreeSet<&StorePath> = paths.iter().collect();
    group.bench_function("btreeset_contains_3000", |b| {
        b.iter(|| paths.iter().filter(|p| set.contains(black_box(p))).count())
    });

    group.bench_function("sort_3000", |b| {
        b.iter_batched_ref(
            || paths.clone(),
            |v| v.sort_unstable(),
            criterion::BatchSize::SmallInput,
        )
    });

    let state = RandomState::new();
    group.bench_function("hash_one", |b| {
        b.iter(|| state.hash_one(black_box(&paths[0])))
    });

    let hashset: HashSet<&StorePath> = paths.iter().collect();
    group.bench_function("hashset_contains_3000", |b| {
        b.iter(|| {
            paths
                .iter()
                .filter(|p| hashset.contains(black_box(p)))
                .count()
        })
    });

    let mut out = String::with_capacity(256);
    group.bench_function("display_full", |b| {
        b.iter(|| {
            out.clear();
            write!(out, "{}", store_dir.display(black_box(&paths[0]))).unwrap();
        })
    });

    group.bench_function("to_base_path", |b| {
        b.iter(|| black_box(&paths[0]).to_base_path())
    });

    group.finish();
}

criterion_group!(benches, benchmark_store_path);
criterion_main!(benches);
