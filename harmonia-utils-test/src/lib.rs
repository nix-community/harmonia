// SPDX-FileCopyrightText: 2024 griff (original Nix.rs)
// SPDX-FileCopyrightText: 2025 Jörg Thalheim (Harmonia adaptation)
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Test utilities for Harmonia.
//!
//! This crate provides proptest strategies and macros for testing Harmonia crates.

use std::path::{Path, PathBuf};
use std::time::Duration;

use proptest::prelude::*;
use tempfile::TempDir;

/// A wrapper around TempDir that provides a canonicalized path.
/// This resolves symlinks like /var -> /private/var on macOS,
/// which is required for Nix store operations.
pub struct CanonicalTempDir {
    _inner: TempDir,
    path: PathBuf,
}

impl CanonicalTempDir {
    /// Create a new temporary directory with a canonicalized path.
    pub fn new() -> std::io::Result<Self> {
        let inner = TempDir::new()?;
        let path = inner.path().canonicalize()?;
        Ok(Self {
            _inner: inner,
            path,
        })
    }

    /// Get the canonicalized path to the temporary directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Byte string type alias.
pub type ByteString = bytes::Bytes;

pub mod helpers;

pub fn arb_filename() -> impl Strategy<Value = String> {
    "[a-zA-Z 0-9.?=+]+".prop_filter("Not cur and parent dir", |s| s != "." && s != "..")
}

pub fn arb_file_component() -> impl Strategy<Value = String> {
    "[a-zA-Z 0-9.?=+]+"
}

prop_compose! {
    pub fn arb_path()(prefix in "[a-zA-Z 0-9.?=+][a-zA-Z 0-9.?=+/]{0,250}", last in arb_filename()) -> PathBuf
    {
        let mut ret = PathBuf::from(prefix);
        ret.push(last);
        ret
    }
}

prop_compose! {
    pub fn arb_byte_string()(data in any::<Vec<u8>>()) -> ByteString {
        ByteString::from(data)
    }
}

prop_compose! {
    pub fn arb_system_time()(secs in arb_duration()) -> Duration
    {
        secs
    }
}

prop_compose! {
    pub fn arb_duration()(secs in proptest::num::i32::ANY) -> Duration
    {
        Duration::from_secs((secs as i64).unsigned_abs())
    }
}

#[macro_export]
macro_rules! pretty_prop_assert_eq {
    ($left:expr , $right:expr,) => ({
        $crate::pretty_prop_assert_eq!($left, $right)
    });
    ($left:expr , $right:expr) => ({
        match (&($left), &($right)) {
            (left_val, right_val) => {
                ::proptest::prop_assert!(*left_val == *right_val,
                    "assertion failed: `(left == right)`\
                          \n\
                          \n{}\
                          \n",
                          ::pretty_assertions::Comparison::new(left_val, right_val))
            }
        }
    });
    ($left:expr , $right:expr, $($arg:tt)*) => ({
        match (&($left), &($right)) {
            (left_val, right_val) => {
                ::proptest::prop_assert!(*left_val == *right_val,
                    "assertion failed: `(left == right)`: {}\
                          \n\
                          \n{}\
                          \n",
                           format_args!($($arg)*),
                           ::pretty_assertions::Comparison::new(left_val, right_val))
            }
        }
    });
}
