// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! `harmonia-gc`: a faster `nix-collect-garbage`.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Parser;
use harmonia_store_fs::{make_store_writable, unshare_mount_namespace};
use harmonia_store_gc::{GcOptions, GcStore, collect_garbage, profiles};
use rayon::prelude::*;
use tracing::{debug, warn};

type MainResult<T> = Result<T, Box<dyn std::error::Error>>;

/// A faster nix-collect-garbage.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Remove old profile generations
    #[arg(short, long)]
    delete_old: bool,

    /// Delete generations older than SPEC (e.g. 30d). Implies --delete-old
    #[arg(long, value_name = "SPEC")]
    delete_older_than: Option<String>,

    /// Show what would be done without doing it
    #[arg(long)]
    dry_run: bool,

    /// Free until SIZE is available (e.g. 50G or 20%). Pinned paths are
    /// never freed
    #[arg(long, value_name = "SIZE", value_parser = parse_ensure_free)]
    ensure_free: Option<EnsureFree>,

    /// Keep paths registered within SPEC (e.g. 7d)
    #[arg(long, value_name = "SPEC")]
    keep_recent: Option<String>,

    /// Skip the database VACUUM after deletion
    #[arg(long)]
    no_vacuum: bool,

    /// Dead paths per delete transaction
    #[arg(long, value_name = "N")]
    chunk_size: Option<usize>,

    /// Override the keep-outputs nix.conf setting
    #[arg(long, value_name = "BOOL")]
    keep_outputs: Option<bool>,

    /// Override the keep-derivations nix.conf setting
    #[arg(long, value_name = "BOOL")]
    keep_derivations: Option<bool>,

    /// Extra directory to scan for GC roots (repeatable)
    #[arg(long = "gc-roots-dir", value_name = "PATH")]
    gc_roots_dirs: Vec<PathBuf>,

    /// Nix store directory
    #[arg(long, value_name = "PATH", default_value = "/nix/store")]
    store_dir: PathBuf,

    /// Nix state directory
    #[arg(long, value_name = "PATH", default_value = "/nix/var/nix")]
    state_dir: PathBuf,
}

/// `--ensure-free` target: absolute bytes or a percentage of the store's
/// filesystem.
#[derive(Debug, Clone, Copy, PartialEq)]
enum EnsureFree {
    Bytes(u64),
    Percent(f64),
}

impl EnsureFree {
    fn target_bytes(self, total: u64) -> u64 {
        match self {
            EnsureFree::Bytes(b) => b,
            EnsureFree::Percent(p) => (total as f64 * p / 100.0) as u64,
        }
    }
}

/// Format a byte count for human-readable output.
fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.2} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.2} KiB", b / KIB)
    } else {
        format!("{bytes} bytes")
    }
}

/// `nix config show <key>` parsed as a bool. Returns `default` if `nix`
/// is not in PATH, the key is unknown, or the value is not a bool.
/// Reusing Nix's own config resolution avoids reimplementing the
/// multi-file nix.conf parser.
fn bool_setting(key: &str, default: bool) -> bool {
    // `nix config show` is gated on nix-command. Pass the flag so this
    // works without it in nix.conf.
    let out = std::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "config",
            "show",
            key,
        ])
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o.stdout,
        // Don't fall back silently: a user who set e.g. keep-outputs=true
        // in nix.conf should learn why it is being ignored.
        Ok(o) => {
            warn!(
                "`nix config show {key}` failed ({}). Using default {default}",
                o.status
            );
            return default;
        }
        Err(e) => {
            warn!("cannot run `nix config show {key}`: {e}. Using default {default}");
            return default;
        }
    };
    parse_bool(&out).unwrap_or(default)
}

fn parse_bool(s: &[u8]) -> Option<bool> {
    match std::str::from_utf8(s).ok()?.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Parse a size like "50G" or a percentage like "20%".
fn parse_ensure_free(s: &str) -> Result<EnsureFree, String> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('%') {
        let p: f64 = num
            .trim()
            .parse()
            .map_err(|e| format!("invalid percentage '{s}': {e}"))?;
        if !p.is_finite() || !(0.0..=100.0).contains(&p) {
            return Err(format!("percentage '{s}' must be between 0 and 100"));
        }
        Ok(EnsureFree::Percent(p))
    } else {
        Ok(EnsureFree::Bytes(parse_size(s)?))
    }
}

/// Parse a size like "50G", "512M", "1024K", or plain bytes.
fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('K' | 'k') => (&s[..s.len() - 1], 1024u64),
        Some('M' | 'm') => (&s[..s.len() - 1], 1024 * 1024),
        Some('G' | 'g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        Some('T' | 't') => (&s[..s.len() - 1], 1024u64.pow(4)),
        _ => (s, 1),
    };
    let n: f64 = num
        .parse()
        .map_err(|e| format!("invalid size '{s}': {e}"))?;
    if !n.is_finite() || n < 0.0 || n * mult as f64 > u64::MAX as f64 {
        return Err(format!("size '{s}' is out of range"));
    }
    Ok((n * mult as f64) as u64)
}

/// Free and total bytes on the filesystem containing `path`.
fn filesystem_bytes(path: &Path) -> MainResult<(u64, u64)> {
    let st =
        nix::sys::statvfs::statvfs(path).map_err(|e| format!("statvfs {}: {e}", path.display()))?;
    // statvfs field types differ between Linux (u64) and macOS (u32).
    let frag = st.fragment_size() as u64;
    let avail = st.blocks_available() as u64 * frag;
    let total = st.blocks() as u64 * frag;
    Ok((avail, total))
}

/// Initialize the rayon global pool up front so thread creation failures
/// (e.g. EAGAIN under a sandbox thread limit) fall back to a single
/// thread instead of panicking on the first `par_iter`.
fn init_rayon() {
    if let Err(e) = rayon::ThreadPoolBuilder::new().build_global() {
        warn!("failed to start rayon thread pool ({e}), falling back to a single thread");
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build_global();
    }
}

fn main() -> MainResult<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let delete_old = args.delete_old || args.delete_older_than.is_some();

    // Must happen before rayon spawns its worker threads: only the
    // calling thread joins the new mount namespace.
    if !args.dry_run
        && let Err(e) = unshare_mount_namespace()
    {
        // EPERM is normal in containers. The remount below then affects
        // the host namespace, like legacy nix-store.
        warn!("no private mount namespace: {e}");
    }
    init_rayon();

    if args.ensure_free.is_some() && args.dry_run {
        return Err("--ensure-free cannot be combined with --dry-run".into());
    }

    // Validate every time spec before any destructive work. A bad
    // --keep-recent must not surface only after generations were deleted.
    let delete_older_cutoff = args
        .delete_older_than
        .as_deref()
        .map(profiles::parse_older_than)
        .transpose()?;
    let keep_recent_after = args
        .keep_recent
        .as_deref()
        .map(profiles::parse_older_than)
        .transpose()?;

    if delete_old {
        profiles::profile_dirs(&args.state_dir)
            .par_iter()
            .try_for_each(|dir| {
                profiles::remove_old_generations(dir, delete_older_cutoff, args.dry_run)
            })?;
    }

    let max_freed = if let Some(ensure_free) = args.ensure_free {
        let (avail, total) = filesystem_bytes(&args.store_dir)?;
        let target = ensure_free.target_bytes(total);
        if avail >= target {
            println!("{} already free, nothing to do", format_size(avail));
            return Ok(());
        }
        let need = target - avail;
        tracing::info!(
            "freeing at least {} to reach {} free",
            format_size(need),
            format_size(target)
        );
        Some(need)
    } else {
        None
    };
    let ensure_free_need = max_freed;

    let store = if args.dry_run {
        // No DB writes happen in a dry run, so don't take write locks or
        // flip the journal mode. A WAL database without its -shm/-wal
        // sidecars cannot be opened read-only. Fall back to read-write.
        GcStore::open_read_only(&args.store_dir, &args.state_dir).or_else(|e| {
            debug!("read-only open failed ({e}), retrying read-write");
            GcStore::open(&args.store_dir, &args.state_dir)
        })?
    } else {
        GcStore::open(&args.store_dir, &args.state_dir)?
    };
    let defaults = harmonia_store_gc::GraphOptions::default();
    let graph_options = harmonia_store_gc::GraphOptions {
        keep_outputs: args
            .keep_outputs
            .unwrap_or_else(|| bool_setting("keep-outputs", defaults.keep_outputs)),
        keep_derivations: args
            .keep_derivations
            .unwrap_or_else(|| bool_setting("keep-derivations", defaults.keep_derivations)),
    };
    if !args.dry_run {
        make_store_writable(store.layout().real_store_dir())?;
    }
    let mut opts = GcOptions {
        graph_options,
        dry_run: args.dry_run,
        max_freed,
        keep_recent_after,
        vacuum: !args.no_vacuum,
        extra_gc_roots_dirs: args.gc_roots_dirs,
        ..Default::default()
    };
    if let Some(n) = args.chunk_size {
        opts.chunk_size = n;
    }
    let report = collect_garbage(&store, &opts)?;
    {
        let mut out = std::io::BufWriter::new(std::io::stdout().lock());
        for p in &report.would_delete {
            writeln!(out, "{p}")?;
        }
    }
    let (bytes_freed, paths_deleted) = (report.bytes_freed, report.paths_deleted);

    if let Some(need) = ensure_free_need
        && bytes_freed < need
    {
        let hint = if keep_recent_after.is_some() {
            " (reachable from GC roots or pinned by --keep-recent)"
        } else {
            " (reachable from GC roots)"
        };
        warn!(
            "only freed {}, {} short of --ensure-free target. \
             The remaining paths are alive{hint}",
            format_size(bytes_freed),
            format_size(need - bytes_freed)
        );
    }

    if args.dry_run {
        println!(
            "{paths_deleted} store paths would be deleted (~{})",
            format_size(bytes_freed)
        );
    } else {
        println!(
            "{paths_deleted} store paths deleted, {} freed",
            format_size(bytes_freed)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Args, EnsureFree, bool_setting, format_size, parse_bool, parse_ensure_free, parse_size,
    };
    use clap::Parser;

    fn try_parse(list: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once(&"harmonia-gc").chain(list))
    }

    #[test]
    fn rejects_unknown_arguments() {
        // A typo'd flag must not silently run a destructive GC.
        assert!(try_parse(&["--dry-rnu"]).is_err());
        assert!(try_parse(&["--keep-resent", "2d"]).is_err());
        assert!(try_parse(&["--dry-run", "extra"]).is_err());
        let parsed = try_parse(&["--dry-run"]).unwrap();
        assert!(parsed.dry_run);
    }

    #[test]
    fn repeatable_and_default_args() {
        let parsed = try_parse(&["--gc-roots-dir", "/a", "--gc-roots-dir", "/b"]).unwrap();
        assert_eq!(parsed.gc_roots_dirs.len(), 2);
        assert_eq!(parsed.store_dir.to_str(), Some("/nix/store"));
        assert_eq!(parsed.state_dir.to_str(), Some("/nix/var/nix"));
        assert!(!parsed.delete_old);
        let parsed = try_parse(&["--delete-older-than", "30d"]).unwrap();
        assert_eq!(parsed.delete_older_than.as_deref(), Some("30d"));
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(1023), "1023 bytes");
        assert_eq!(format_size(1536), "1.50 KiB");
        assert_eq!(format_size(5 * 1024 * 1024 + 512 * 1024), "5.50 MiB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024 / 2), "1.50 GiB");
    }

    #[test]
    fn parse_bool_values() {
        assert_eq!(parse_bool(b"true\n"), Some(true));
        assert_eq!(parse_bool(b"false\n"), Some(false));
        assert_eq!(parse_bool(b"42\n"), None);
        assert_eq!(parse_bool(b""), None);
    }

    #[test]
    fn unknown_setting_falls_back_to_default() {
        assert!(bool_setting("this-setting-does-not-exist", true));
        assert!(!bool_setting("this-setting-does-not-exist", false));
    }

    #[test]
    fn parse_size_units() {
        assert_eq!(parse_size("0").unwrap(), 0);
        assert_eq!(parse_size("123").unwrap(), 123);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("2k").unwrap(), 2048);
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("3m").unwrap(), 3 * 1024 * 1024);
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("2g").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1T").unwrap(), 1024u64.pow(4));
        assert_eq!(parse_size("1.5K").unwrap(), 1536);
        assert_eq!(parse_size(" 4M ").unwrap(), 4 * 1024 * 1024);
        assert!(parse_size("abc").is_err());
        assert!(parse_size("-5G").is_err());
        assert!(parse_size("inf").is_err());
        assert!(parse_size("NaN").is_err());
        assert!(parse_size("99999999999999999999G").is_err());
    }

    #[test]
    fn parse_ensure_free_values() {
        assert_eq!(
            parse_ensure_free("50G").unwrap(),
            EnsureFree::Bytes(50 * 1024u64.pow(3))
        );
        assert_eq!(parse_ensure_free("1024").unwrap(), EnsureFree::Bytes(1024));
        assert_eq!(parse_ensure_free("20%").unwrap(), EnsureFree::Percent(20.0));
        assert_eq!(
            parse_ensure_free(" 12.5 % ").unwrap(),
            EnsureFree::Percent(12.5)
        );
        assert_eq!(
            parse_ensure_free("100%").unwrap(),
            EnsureFree::Percent(100.0)
        );
        assert!(parse_ensure_free("101%").is_err());
        assert!(parse_ensure_free("-5%").is_err());
        assert!(parse_ensure_free("abc%").is_err());
    }
}
