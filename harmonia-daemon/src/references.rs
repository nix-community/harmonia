// SPDX-FileCopyrightText: 2025 Jörg Thalheim
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
//! 2. For each window position, validate characters right-to-left against
//!    the nix-base32 alphabet. If an invalid character is found at offset j,
//!    skip ahead by j+1 positions (Boyer-Moore style).
//! 3. When a valid 32-byte window is found, look it up in a `HashSet`.
//!
//! This gives O(n/32) amortized performance on binary data (most bytes are
//! not in the nix-base32 alphabet), independent of the number of candidates.
//!
//! # Integration with NAR streaming
//!
//! `RefScanSink` implements a push-based interface: call [`RefScanSink::feed`]
//! with each chunk of NAR bytes. This allows scanning for references during
//! NAR serialization (for hash computation) without a separate disk walk,
//! matching Nix's `TeeSink{refsSink, hashSink}` pattern.

use std::collections::{BTreeSet, HashSet};

use harmonia_store_core::store_path::{StorePath, StorePathHash};

/// Encoded length of a store path hash in nix-base32 (32 bytes).
const HASH_LEN: usize = StorePathHash::encoded_len();

/// 256-byte lookup table: `true` for bytes that are valid nix-base32 characters.
/// The nix-base32 alphabet is `0123456789abcdfghijklmnpqrsvwxyz`.
const NIX_BASE32_VALID: [bool; 256] = {
    let mut table = [false; 256];
    let chars = b"0123456789abcdfghijklmnpqrsvwxyz";
    let mut i = 0;
    while i < chars.len() {
        table[chars[i] as usize] = true;
        i += 1;
    }
    table
};

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
    /// Hash strings we're still looking for (removed on match, like Nix).
    pending: HashSet<[u8; HASH_LEN]>,
    /// Hash strings we've found so far.
    seen: HashSet<[u8; HASH_LEN]>,
    /// Map from hash bytes back to StorePath for result construction.
    back_map: Vec<([u8; HASH_LEN], StorePath)>,
    /// Tail bytes from the previous chunk for boundary matching.
    tail: Vec<u8>,
}

impl RefScanSink {
    /// Create a new scanner for the given candidate store paths.
    ///
    /// `candidates` is the set of store paths to search for (typically all
    /// build inputs). `self_path` is the output path itself, for detecting
    /// self-references.
    pub fn new(candidates: &BTreeSet<StorePath>, self_path: Option<&StorePath>) -> Self {
        let mut pending = HashSet::with_capacity(candidates.len() + 1);
        let mut back_map = Vec::with_capacity(candidates.len() + 1);

        for sp in candidates {
            let hash_bytes = hash_to_bytes(sp);
            pending.insert(hash_bytes);
            back_map.push((hash_bytes, sp.clone()));
        }

        if let Some(sp) = self_path {
            let hash_bytes = hash_to_bytes(sp);
            if pending.insert(hash_bytes) {
                back_map.push((hash_bytes, sp.clone()));
            }
        }

        Self {
            pending,
            seen: HashSet::new(),
            back_map,
            tail: Vec::with_capacity(HASH_LEN),
        }
    }

    /// Feed a chunk of bytes to the scanner.
    ///
    /// Handles boundary matches by keeping a tail buffer from the previous
    /// chunk. This mirrors Nix's `RefScanSink::operator()`.
    pub fn feed(&mut self, data: &[u8]) {
        if self.pending.is_empty() {
            return;
        }

        // Mirrors Nix's RefScanSink::operator() exactly.
        let tail_len = data.len().min(HASH_LEN);

        // Search the overlap region: copy of old tail + start of new data.
        // Uses a separate buffer so self.tail can be rebuilt independently.
        if !self.tail.is_empty() {
            let mut overlap = self.tail.clone();
            overlap.extend_from_slice(&data[..tail_len]);
            search(&overlap, &mut self.pending, &mut self.seen);
        }

        // Search the current chunk itself.
        search(data, &mut self.pending, &mut self.seen);

        // Rebuild tail: keep up to HASH_LEN bytes total
        // (suffix of old tail + suffix of new data).
        let rest = HASH_LEN - tail_len;
        if rest < self.tail.len() {
            self.tail.drain(..self.tail.len() - rest);
        }
        self.tail.extend_from_slice(&data[data.len() - tail_len..]);
    }

    /// Returns the set of store paths whose hashes were found.
    pub fn found_paths(&self) -> BTreeSet<StorePath> {
        let mut result = BTreeSet::new();
        for (hash_bytes, store_path) in &self.back_map {
            if self.seen.contains(hash_bytes) {
                result.insert(store_path.clone());
            }
        }
        result
    }
}

/// Convert a store path's hash to a fixed-size byte array for zero-alloc lookups.
fn hash_to_bytes(sp: &StorePath) -> [u8; HASH_LEN] {
    let s = sp.hash().to_string();
    let mut buf = [0u8; HASH_LEN];
    buf.copy_from_slice(s.as_bytes());
    buf
}

/// Core search algorithm matching Nix's `search()` in `references.cc`.
///
/// Scans `data` for valid nix-base32 windows of length [`HASH_LEN`].
/// Uses right-to-left character validation with Boyer-Moore-style skipping:
/// when an invalid character is found at offset j within the window,
/// advance by j+1 positions. On random binary data this skips ~32 bytes
/// per invalid character, giving O(n/32) amortized performance.
///
/// Matched hashes are moved from `pending` to `seen`.
#[inline]
fn search(data: &[u8], pending: &mut HashSet<[u8; HASH_LEN]>, seen: &mut HashSet<[u8; HASH_LEN]>) {
    if data.len() < HASH_LEN {
        return;
    }

    let mut i = 0;
    while i + HASH_LEN <= data.len() {
        // Scan the window right-to-left for valid nix-base32 characters.
        let mut j = HASH_LEN;
        loop {
            if j == 0 {
                break;
            }
            j -= 1;
            if !NIX_BASE32_VALID[data[i + j] as usize] {
                i += j + 1;
                break;
            }
        }
        if j > 0 {
            // Broke out early due to invalid character, already advanced i.
            continue;
        }

        // All HASH_LEN characters are valid nix-base32. Check the HashSet.
        let window: [u8; HASH_LEN] = data[i..i + HASH_LEN]
            .try_into()
            .expect("slice length matches HASH_LEN");

        if pending.remove(&window) {
            seen.insert(window);
        }

        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use futures::StreamExt as _;

    /// Helper: dump a path as NAR, feeding each byte chunk through the scanner.
    async fn scan_nar_for_references(
        path: &std::path::Path,
        candidates: &BTreeSet<StorePath>,
        self_path: Option<&StorePath>,
    ) -> BTreeSet<StorePath> {
        let mut sink = RefScanSink::new(candidates, self_path);

        // Stream the NAR and feed chunks to the scanner, just like
        // production code would do while computing the NAR hash.
        let mut stream = harmonia_nar::NarByteStream::new(path.to_path_buf());
        while let Some(chunk) = stream.next().await {
            sink.feed(&chunk.unwrap());
        }

        sink.found_paths()
    }

    /// Output file containing an input's hash part → input is discovered as a reference.
    #[tokio::test]
    async fn test_scan_finds_input_reference() {
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

    /// Output containing its own hash part → self-reference detected.
    #[tokio::test]
    async fn test_scan_finds_self_reference() {
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
}
