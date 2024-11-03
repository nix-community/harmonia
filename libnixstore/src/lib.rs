#![warn(clippy::dbg_macro)]
#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(
    nonstandard_style,
    rust_2018_idioms,
    rustdoc::broken_intra_doc_links,
    rustdoc::private_intra_doc_links
)]
#![forbid(non_ascii_idents)]

#[cxx::bridge(namespace = "libnixstore")]
mod ffi {
    struct InternalPathInfo {
        drv: String,
        narhash: String,
        time: i64,
        size: u64,
        refs: Vec<String>,
        sigs: Vec<String>,
        ca: String,
    }

    unsafe extern "C++" {
        include!("libnixstore/include/nix.h");

        fn init();
        fn is_valid_path(path: &str) -> Result<bool>;
        fn query_path_hash(path: &str) -> Result<String>;
        fn query_path_info(path: &str, base32: bool) -> Result<InternalPathInfo>;
        fn query_path_from_hash_part(hash_part: &str) -> Result<String>;
        fn get_store_dir() -> String;
        fn get_real_store_dir() -> String;
    }
}

fn string_to_opt(v: String) -> Option<String> {
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

pub struct PathInfo {
    /// The deriver of this path, if one exists.
    pub drv: Option<String>,
    /// The result of executing `nix-store --dump` on this path and hashing its output.  This string
    /// can be either a hexidecimal or base32 string, depending on the arguments passed to
    /// `query_path_info()`.
    pub narhash: String,
    /// The time at which this store path was registered.
    pub time: i64,
    /// The size of the nar archive which would be produced by applying `nix-store --dump` to this
    /// path.
    pub size: u64,
    /// The store paths referenced by this path.
    pub refs: Vec<String>,
    /// The signatures on this store path; "note: not necessarily verified".
    pub sigs: Vec<String>,
    /// Indicates if this store-path is input-addressed (`None`) or content-addressed (`Some`).  The
    /// `String` value contains the content hash as well as "some other bits of data"; see
    /// `path-info.hh` for details.
    pub ca: Option<String>,
}

/// Nix's `libstore` offers two options for representing the
/// hash-part of store paths.
pub enum Radix {
    /// Ordinary hexadecimal, using the 16-character alphabet [0-9a-f]
    Base16,

    /// The encoding used for filenames directly beneath /nix/store, using a 32-character alphabet
    Base32,
}

impl Default for Radix {
    /// Defaults to base-32 since that is almost always what you want.
    fn default() -> Self {
        Self::Base32
    }
}

#[inline]
/// Perform any necessary effectful operation to make the store up and running.
pub fn init() {
    ffi::init();
}

#[inline]
#[must_use]
/// Check whether a path is valid.
pub fn is_valid_path(path: &str) -> bool {
    ffi::is_valid_path(path).unwrap_or(false)
}

#[inline]
/// Return narhash of a valid path. It is permitted to omit the name part of the store path.
pub fn query_path_hash(path: &str) -> Result<String, cxx::Exception> {
    ffi::query_path_hash(path)
}

#[inline]
/// Query information about a valid path. It is permitted to omit the name part of the store path.
/// The `radix` field affects only the `narHash` field of the result.
pub fn query_path_info(path: &str, radix: Radix) -> Result<PathInfo, cxx::Exception> {
    let res = ffi::query_path_info(path, matches!(radix, Radix::Base32))?;
    Ok(PathInfo {
        drv: string_to_opt(res.drv),
        narhash: res.narhash,
        time: res.time,
        size: res.size,
        refs: res.refs,
        sigs: res.sigs,
        ca: string_to_opt(res.ca),
    })
}

#[inline]
#[must_use]
/// Query the full store path given the hash part of a valid store path, or empty if the path
/// doesn't exist.
pub fn query_path_from_hash_part(hash_part: &str) -> Option<String> {
    match ffi::query_path_from_hash_part(hash_part) {
        Ok(v) => string_to_opt(v),
        Err(_) => None,
    }
}

#[inline]
#[must_use]
/// Returns the path to the directory where nix store sources and derived files.
pub fn get_store_dir() -> String {
    ffi::get_store_dir()
}

#[inline]
#[must_use]
/// Returns the physical path to the nix store.
pub fn get_real_store_dir() -> String {
    ffi::get_real_store_dir()
}
