// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! `harmonia-gc`: a faster `nix-collect-garbage`.

use std::path::{Path, PathBuf};

use clap::Parser;
use harmonia_store_gc::store::GcStore;
use harmonia_store_gc::{format_size, gc, profiles, unshare_mount_namespace};
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
    if !args.dry_run {
        unshare_mount_namespace();
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
        .transpose()?
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });

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

    let mut store = if args.dry_run {
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
    if let Some(v) = args.keep_outputs {
        store.graph_options.keep_outputs = v;
    }
    if let Some(v) = args.keep_derivations {
        store.graph_options.keep_derivations = v;
    }
    let opts = gc::GcOptions {
        dry_run: args.dry_run,
        max_freed,
        keep_recent_after,
        no_vacuum: args.no_vacuum,
        chunk_size: args.chunk_size,
        extra_gc_roots_dirs: args.gc_roots_dirs,
    };
    let (bytes_freed, paths_deleted) = gc::collect_garbage(&store, &opts)?;

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
    use super::{Args, EnsureFree, parse_ensure_free, parse_size};
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
