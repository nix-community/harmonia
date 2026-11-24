// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Reference scanning for store path outputs.
//!
//! After a build completes, we need to discover which store paths are
//! referenced by the output. This module provides [`RefScanSink`], a
//! streaming scanner that can be fed arbitrary byte chunks (typically
//! from a NAR stream) and efficiently finds store path hash references.
//!
//! # Algorithm
//!
//! Rather than searching for each candidate pattern separately (O(n×k)),
//! we use the same approach as Nix's `search()` in `references.cc`:
//!
//! 1. Slide a window of [`HASH_LEN`] bytes across the input.
//! 2. Validate trailing bytes of each window against the nix-base32 alphabet
//!    to skip invalid regions (Boyer-Moore style). Large chunks run four
//!    independent lanes in parallel to hide lookup latency.
//! 3. When a valid 32-byte window is found, check a compact blocked bloom
//!    filter before probing the hash map, then extend through consecutive
//!    valid bytes in O(1) per byte.
//!
//! # Integration with NAR streaming
//!
//! `RefScanSink` implements a push-based interface: call [`RefScanSink::feed`]
//! with each chunk of NAR bytes. This allows scanning for references during
//! NAR serialization (for hash computation) without a separate disk walk,
//! matching Nix's `TeeSink{refsSink, hashSink}` pattern.

use std::collections::BTreeSet;

use foldhash::{HashMap, HashMapExt as _};
use harmonia_store_path::{StorePath, StorePathHash};
use harmonia_utils_base_encoding::base32;

/// Encoded length of a store path hash in nix-base32 (32 bytes).
const HASH_LEN: usize = StorePathHash::encoded_len();

/// Maximum number of bytes from a previous chunk that can participate in a
/// cross-boundary match (31 bytes).
const OVERLAP_LEN: usize = HASH_LEN - 1;

/// Minimum chunk length to split across four interleaved scan lanes.
const PARALLEL_LANES_MIN_LEN: usize = 512;

/// 256-byte lookup table: `1` for bytes that are valid nix-base32 characters,
/// `0` otherwise.
const NIX_BASE32_VALID: [u8; 256] = {
    let mut table = [0u8; 256];
    let chars = base32::ALPHABET_BYTES;
    let mut i = 0;
    while i < chars.len() {
        table[chars[i] as usize] = 1;
        i += 1;
    }
    table
};

/// Single-word blocked Bloom filter (1 KiB, 128 x 64-bit blocks, 2 bits per key)
/// keyed on the last 8 bytes of each candidate hash.
///
/// A lookup loads one 64-bit word from L1 cache and rejects false-positive
/// valid base32 windows before touching the hash map.
struct HashFilter {
    words: [u64; 128],
}

impl HashFilter {
    fn new() -> Self {
        Self { words: [0u64; 128] }
    }

    #[inline]
    fn slot(hash: &[u8; HASH_LEN]) -> (usize, u64) {
        let tail = u64::from_le_bytes(*hash.last_chunk::<8>().expect("HASH_LEN >= 8"));
        let h = tail.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let word = (h >> 57) as usize;
        let mask = (1u64 << ((h >> 51) & 63)) | (1u64 << ((h >> 45) & 63));
        (word, mask)
    }

    #[inline]
    fn insert(&mut self, hash: &[u8; HASH_LEN]) {
        let (word, mask) = Self::slot(hash);
        self.words[word] |= mask;
    }

    #[inline]
    fn may_contain(&self, hash: &[u8; HASH_LEN]) -> bool {
        let (word, mask) = Self::slot(hash);
        (self.words[word] & mask) == mask
    }
}

/// A streaming reference scanner that finds store path hashes in byte data.
///
/// Feed it chunks of bytes (e.g., from a NAR stream) via [`feed`](Self::feed),
/// then retrieve results with [`found_paths`](Self::found_paths).
///
/// # Example
///
/// ```ignore
/// let mut sink = RefScanSink::new(&candidates, Some(&self_path));
/// for chunk in nar_chunks {
///     sink.feed(&chunk);
/// }
/// let references = sink.found_paths();
/// ```
pub struct RefScanSink {
    /// Candidate hashes we are still looking for (removed on match, like Nix).
    pending: HashMap<[u8; HASH_LEN], StorePath>,
    /// Store paths found so far.
    found: BTreeSet<StorePath>,
    /// Compact prefilter over all initial candidates.
    filter: HashFilter,
    /// Trailing bytes from previous chunks for boundary matching.
    tail: [u8; OVERLAP_LEN],
    /// Number of valid bytes currently stored in `tail`.
    tail_len: u8,
}

impl RefScanSink {
    /// Create a new scanner for the given candidate store paths.
    ///
    /// `candidates` is the set of store paths to search for (typically all
    /// build inputs). `self_path` is the output path itself, for detecting
    /// self-references; it may also appear in `candidates`.
    ///
    /// Each candidate must have a distinct hash part, as Nix's
    /// `PathRefScanSink::fromPaths` asserts. Store paths are addressed by
    /// their hash, so a real store never yields two paths that share one; if
    /// such candidates are passed anyway, only the first is reported.
    pub fn new(candidates: &BTreeSet<StorePath>, self_path: Option<&StorePath>) -> Self {
        let cap = candidates.len() + usize::from(self_path.is_some());
        let mut pending = HashMap::with_capacity(cap);
        let mut filter = HashFilter::new();

        for sp in candidates {
            let hash_bytes = hash_to_bytes(sp);
            debug_assert!(
                !pending.contains_key(&hash_bytes),
                "reference candidates share a hash part: {sp}"
            );
            filter.insert(&hash_bytes);
            pending.entry(hash_bytes).or_insert_with(|| sp.clone());
        }
        if let Some(sp) = self_path {
            let hash_bytes = hash_to_bytes(sp);
            filter.insert(&hash_bytes);
            pending.entry(hash_bytes).or_insert_with(|| sp.clone());
        }

        Self {
            pending,
            found: BTreeSet::new(),
            filter,
            tail: [0u8; OVERLAP_LEN],
            tail_len: 0,
        }
    }

    /// Feed a chunk of bytes to the scanner.
    ///
    /// Handles boundary matches by keeping a fixed stack tail buffer from
    /// previous chunks without heap allocation.
    pub fn feed(&mut self, data: &[u8]) {
        if self.pending.is_empty() || data.is_empty() {
            return;
        }

        let prev_len = self.tail_len as usize;
        if prev_len > 0 && prev_len + data.len() >= HASH_LEN {
            let head_len = data.len().min(OVERLAP_LEN);
            let mut overlap = [0u8; OVERLAP_LEN * 2];
            overlap[..prev_len].copy_from_slice(&self.tail[..prev_len]);
            overlap[prev_len..prev_len + head_len].copy_from_slice(&data[..head_len]);
            self.search(&overlap[..prev_len + head_len]);
            if self.pending.is_empty() {
                return;
            }
        }

        self.search(data);
        if self.pending.is_empty() {
            return;
        }

        let take = data.len().min(OVERLAP_LEN);
        let keep = (OVERLAP_LEN - take).min(prev_len);
        if keep > 0 && keep < prev_len {
            self.tail.copy_within(prev_len - keep..prev_len, 0);
        }
        self.tail[keep..keep + take].copy_from_slice(&data[data.len() - take..]);
        self.tail_len = (keep + take) as u8;
    }

    /// Returns the set of store paths whose hashes were found.
    pub fn found_paths(&self) -> BTreeSet<StorePath> {
        self.found.clone()
    }

    /// Verify `data[i..i + HASH_LEN]` (given its last `checked_tail` bytes are
    /// already known valid), handle any contiguous valid run, and return the
    /// next start index to scan.
    #[inline(never)]
    fn check_candidate_window(
        &mut self,
        data: &[u8],
        mut i: usize,
        end_excl: usize,
        checked_tail: usize,
    ) -> usize {
        let slice = &data[..end_excl + HASH_LEN - 1];
        let mut candidate: &[u8; HASH_LEN] = slice[i..i + HASH_LEN]
            .try_into()
            .expect("slice length matches HASH_LEN");

        let mut j = HASH_LEN - checked_tail;
        while j > 0 {
            j -= 1;
            if NIX_BASE32_VALID[candidate[j] as usize] == 0 {
                return i + j + 1;
            }
        }

        loop {
            if self.filter.may_contain(candidate)
                && let Some(sp) = self.pending.remove(candidate)
            {
                self.found.insert(sp);
                if self.pending.is_empty() {
                    return end_excl;
                }
            }
            i += 1;
            if i >= end_excl {
                return end_excl;
            }
            candidate = slice[i..i + HASH_LEN]
                .try_into()
                .expect("slice length matches HASH_LEN");
            if NIX_BASE32_VALID[candidate[HASH_LEN - 1] as usize] == 0 {
                return i + HASH_LEN;
            }
        }
    }

    /// Scan start indices `start..end_excl` in `data`
    /// (requires `end_excl + HASH_LEN - 1 <= data.len()`).
    #[inline]
    fn search_range(&mut self, data: &[u8], mut i: usize, end_excl: usize) {
        if self.pending.is_empty() {
            return;
        }
        while i < end_excl {
            if NIX_BASE32_VALID[data[i + HASH_LEN - 1] as usize] == 0 {
                i += HASH_LEN;
                continue;
            }
            i = self.check_candidate_window(data, i, end_excl, 1);
            if self.pending.is_empty() {
                return;
            }
        }
    }

    #[inline]
    fn search(&mut self, data: &[u8]) {
        if data.len() < HASH_LEN || self.pending.is_empty() {
            return;
        }
        let total_starts = data.len() - HASH_LEN + 1;
        if data.len() < PARALLEL_LANES_MIN_LEN {
            self.search_range(data, 0, total_starts);
            return;
        }

        // Partition start indices into four disjoint quarters so four
        // independent cursors advance in parallel without loop-carried stalls.
        let q = total_starts / 4;
        let mut pos = [0, q, q * 2, q * 3];
        let end = [q, q * 2, q * 3, total_starts];

        while pos[0] < end[0] && pos[1] < end[1] && pos[2] < end[2] && pos[3] < end[3] {
            let mut last = [0u8; 4];
            let mut hits = [0u8; 4];
            for lane in 0..4 {
                let tail: &[u8; 2] = data[pos[lane] + HASH_LEN - 2..pos[lane] + HASH_LEN]
                    .try_into()
                    .expect("slice length matches 2");
                let a = NIX_BASE32_VALID[tail[1] as usize];
                let b = NIX_BASE32_VALID[tail[0] as usize];
                last[lane] = a;
                hits[lane] = a & b;
            }

            if (hits[0] | hits[1] | hits[2] | hits[3]) == 0 {
                for lane in 0..4 {
                    pos[lane] += HASH_LEN - (last[lane] as usize);
                }
                continue;
            }

            for lane in 0..4 {
                if hits[lane] != 0 {
                    pos[lane] = self.check_candidate_window(data, pos[lane], end[lane], 2);
                } else {
                    pos[lane] += HASH_LEN - (last[lane] as usize);
                }
            }
            if self.pending.is_empty() {
                return;
            }
        }

        for lane in 0..4 {
            self.search_range(data, pos[lane], end[lane]);
        }
    }
}

/// Encode a store path hash directly into a fixed-size stack byte array.
#[inline]
fn hash_to_bytes(sp: &StorePath) -> [u8; HASH_LEN] {
    let mut buf = [0u8; HASH_LEN];
    base32::encode_mut(sp.hash().as_ref(), &mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use futures_util::StreamExt as _;

    /// Helper: dump a path as NAR, feeding each byte chunk through the scanner.
    async fn scan_nar_for_references(
        path: &std::path::Path,
        candidates: &BTreeSet<StorePath>,
        self_path: Option<&StorePath>,
    ) -> BTreeSet<StorePath> {
        let mut sink = RefScanSink::new(candidates, self_path);

        // Stream the NAR and feed chunks to the scanner, just like
        // production code would do while computing the NAR hash.
        let mut stream = harmonia_file_nar::NarByteStream::new(path.to_path_buf());
        while let Some(chunk) = stream.next().await {
            sink.feed(&chunk.unwrap());
        }

        sink.found_paths()
    }

    /// Output file containing an input's hash part -> input is discovered as a reference.
    #[test]
    fn test_scan_finds_input_reference() {
        let input = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-input").unwrap();
        let hash_str = input.hash().to_string();
        let data = format!("some content /nix/store/{hash_str}-input more stuff");

        let mut candidates = BTreeSet::new();
        candidates.insert(input.clone());

        let mut sink = RefScanSink::new(&candidates, None);
        sink.feed(data.as_bytes());

        let refs = sink.found_paths();
        assert!(refs.contains(&input), "Should discover input as reference");
    }

    /// Output containing its own hash part -> self-reference detected.
    #[test]
    fn test_scan_finds_self_reference() {
        let self_path = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-self").unwrap();
        let hash_str = self_path.hash().to_string();
        let data = format!("#!/bin/sh\nexec /nix/store/{hash_str}-self/bin/real \"$@\"");

        let candidates = BTreeSet::new();
        let mut sink = RefScanSink::new(&candidates, Some(&self_path));
        sink.feed(data.as_bytes());

        let refs = sink.found_paths();
        assert!(refs.contains(&self_path), "Should detect self-reference");
    }

    /// Feed data in every possible chunk size to verify the tail logic
    /// handles hashes spanning any number of chunks (2, 3, ... up to N).
    #[test]
    fn test_scan_across_chunk_boundary() {
        let input = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-test").unwrap();
        let hash_str = input.hash().to_string();
        let content = format!("prefix{hash_str}suffix");
        let bytes = content.as_bytes();

        // chunk_size=1 means single-byte feeds (hash spans 32 chunks),
        // chunk_size=bytes.len() means one big feed.
        for chunk_size in 1..=bytes.len() {
            let mut candidates = BTreeSet::new();
            candidates.insert(input.clone());
            let mut sink = RefScanSink::new(&candidates, None);

            for chunk in bytes.chunks(chunk_size) {
                sink.feed(chunk);
            }

            let refs = sink.found_paths();
            assert!(
                refs.contains(&input),
                "Should find reference with chunk_size={chunk_size}"
            );
        }
    }

    #[test]
    fn test_scan_across_lane_boundaries() {
        let sp = StorePath::from_base_path("0123456789abcdfghijklmnpqrsvwxyz-lane").unwrap();
        let mut cands = BTreeSet::new();
        cands.insert(sp.clone());
        let hash = b"0123456789abcdfghijklmnpqrsvwxyz";
        for off in 0..=(1024 - HASH_LEN) {
            let mut buf = vec![b'e'; 1024];
            buf[off..off + HASH_LEN].copy_from_slice(hash);
            let mut sink = RefScanSink::new(&cands, None);
            sink.feed(&buf);
            assert!(sink.found_paths().contains(&sp), "missed offset {off}");
        }
    }

    #[test]
    fn test_scan_overlapping_valid_run() {
        let a = StorePath::from_base_path("0123456789abcdfghijklmnpqrsvwxyz-first").unwrap();
        let b = StorePath::from_base_path("123456789abcdfghijklmnpqrsvwxyz0-second").unwrap();
        let mut candidates = BTreeSet::new();
        candidates.insert(a.clone());
        candidates.insert(b.clone());

        let mut sink = RefScanSink::new(&candidates, None);
        sink.feed(b"0123456789abcdfghijklmnpqrsvwxyz0");
        assert_eq!(sink.found_paths(), candidates);
    }

    /// Input reference discovered when scanning over a NAR stream.
    #[tokio::test]
    async fn test_nar_scan_finds_input_reference() {
        let dir = tempfile::TempDir::new().unwrap();
        let output_dir = dir.path().join("output");
        fs::create_dir(&output_dir).unwrap();

        let input = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-input").unwrap();
        let hash_str = input.hash().to_string();

        fs::write(
            output_dir.join("file.txt"),
            format!("some content /nix/store/{hash_str}-input more stuff"),
        )
        .unwrap();

        let mut candidates = BTreeSet::new();
        candidates.insert(input.clone());

        let refs = scan_nar_for_references(&output_dir, &candidates, None).await;
        assert!(refs.contains(&input), "Should discover input as reference");
    }

    /// Self-reference detected when scanning over a NAR stream.
    #[tokio::test]
    async fn test_nar_scan_finds_self_reference() {
        let dir = tempfile::TempDir::new().unwrap();
        let output_dir = dir.path().join("output");
        fs::create_dir(&output_dir).unwrap();

        let self_path = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-self").unwrap();
        let hash_str = self_path.hash().to_string();

        fs::write(
            output_dir.join("wrapper.sh"),
            format!("#!/bin/sh\nexec /nix/store/{hash_str}-self/bin/real \"$@\""),
        )
        .unwrap();

        let candidates = BTreeSet::new();
        let refs = scan_nar_for_references(&output_dir, &candidates, Some(&self_path)).await;
        assert!(refs.contains(&self_path), "Should detect self-reference");
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    /// An invalid nix-base32 byte just left of a hash that straddles a chunk
    /// boundary used to overrun the overlap buffer: the leftmost-char skip
    /// leaves j==0, which the old guard mistook for a fully-valid window.
    #[test]
    fn invalid_byte_before_boundary_straddling_hash() {
        let sp = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-input").unwrap();
        let mut cands = BTreeSet::new();
        cands.insert(sp.clone());
        let hash = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

        // 'e' is not in the nix-base32 alphabet. Sweep every prefix length and
        // split point so the hash lands on each possible chunk boundary.
        for prefix in 1..=(2 * HASH_LEN) {
            let mut data = vec![b'e'; prefix];
            data.extend_from_slice(hash);

            for split in 1..data.len() {
                let mut sink = RefScanSink::new(&cands, None);
                sink.feed(&data[..split]);
                sink.feed(&data[split..]);
                assert!(
                    sink.found_paths().contains(&sp),
                    "hash not found with prefix={prefix} split={split}",
                );
            }
        }
    }
}
