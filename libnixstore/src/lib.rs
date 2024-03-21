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

    struct InternalTuple {
        lhs: String,
        rhs: String,
    }

    struct InternalDrv {
        outputs: Vec<InternalTuple>,
        input_drvs: Vec<String>,
        input_srcs: Vec<String>,
        platform: String,
        builder: String,
        args: Vec<String>,
        env: Vec<InternalTuple>,
    }

    unsafe extern "C++" {
        include!("libnixstore/include/nix.h");

        fn init();
        fn is_valid_path(path: &str) -> Result<bool>;
        fn query_path_hash(path: &str) -> Result<String>;
        fn query_path_info(path: &str, base32: bool) -> Result<InternalPathInfo>;
        fn query_path_from_hash_part(hash_part: &str) -> Result<String>;
        fn convert_hash(algo: &str, s: &str, to_base_32: bool) -> Result<String>;
        fn sign_string(secret_key: &str, msg: &str) -> Result<String>;
        fn check_signature(public_key: &str, sig: &str, msg: &str) -> Result<bool>;
        fn derivation_from_path(drv_path: &str) -> Result<InternalDrv>;
        fn get_store_dir() -> String;
        fn get_build_log(derivation_path: &str) -> Result<String>;
        fn get_nar_list(store_path: &str) -> Result<String>;
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

pub struct Drv {
    /// The mapping from output names to to realised outpaths, or `None` for outputs which are not
    /// realised in this store.
    pub outputs: std::collections::HashMap<String, Option<String>>,
    /// The paths of this derivation's input derivations.
    pub input_drvs: Vec<String>,
    /// The paths of this derivation's input sources; these are files which enter the nix store as a
    /// result of `nix-store --add` or a `./path` reference.
    pub input_srcs: Vec<String>,
    /// The `system` field of the derivation.
    pub platform: String,
    /// The `builder` field of the derivation, which is executed in order to realise the
    /// derivation's outputs.
    pub builder: String,
    /// The arguments passed to `builder`.
    pub args: Vec<String>,
    /// The environment with which the `builder` is executed.
    pub env: std::collections::HashMap<String, String>,
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
/// Parse the hash from a string representation in the format `[<type>:]<base16|base32|base64>` or
/// `<type>-<base64>` to a string representation of the hash, in `base-16`, `base-32`. The result
/// is not prefixed by the hash type.
pub fn convert_hash(algo: &str, s: &str, to_radix: Radix) -> Result<String, cxx::Exception> {
    ffi::convert_hash(algo, s, matches!(to_radix, Radix::Base32))
}

#[inline]
/// Return a detached signature of the given string.
pub fn sign_string(secret_key: &str, msg: &str) -> Result<String, cxx::Exception> {
    ffi::sign_string(secret_key, msg)
}

#[inline]
/// Verify that `sig` is a valid signature for `msg`, using the signer's `public_key`.
pub fn check_signature(public_key: &str, sig: &str, msg: &str) -> Result<bool, cxx::Exception> {
    ffi::check_signature(public_key, sig, msg)
}

#[inline]
/// Read a derivation, after ensuring its existence through `ensurePath()`.
pub fn derivation_from_path(drv_path: &str) -> Result<Drv, cxx::Exception> {
    let res = ffi::derivation_from_path(drv_path)?;
    let mut outputs = std::collections::HashMap::new();
    for out in res.outputs {
        outputs.insert(out.lhs, string_to_opt(out.rhs));
    }

    let mut env = std::collections::HashMap::new();
    for v in res.env {
        env.insert(v.lhs, v.rhs);
    }

    Ok(Drv {
        outputs,
        input_drvs: res.input_drvs,
        input_srcs: res.input_srcs,
        platform: res.platform,
        builder: res.builder,
        args: res.args,
        env,
    })
}

#[inline]
#[must_use]
/// Returns the path to the directory where nix store sources and derived files.
pub fn get_store_dir() -> String {
    ffi::get_store_dir()
}

#[inline]
#[must_use]
/// Return the build log of the specified store path, if available, or null otherwise.
pub fn get_build_log(derivation_path: &str) -> Option<String> {
    match ffi::get_build_log(derivation_path) {
        Ok(v) => string_to_opt(v),
        Err(_) => None,
    }
}

#[inline]
/// Return a JSON representation as String of the contents of a NAR (except file contents).
pub fn get_nar_list(store_path: &str) -> Result<String, cxx::Exception> {
    ffi::get_nar_list(store_path)
}
