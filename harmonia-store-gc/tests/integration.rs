use std::fs;
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::Connection;

use harmonia_store_db::{OpenMode, StoreDb};
use harmonia_store_gc::{GcOptions, GcStore, collect_garbage};

struct TestStore {
    #[allow(dead_code)] // keep tempdir alive
    dir: tempfile::TempDir,
    store_dir: PathBuf,
    state_dir: PathBuf,
}

#[derive(Clone)]
struct Pkg {
    id: i64,
    path: PathBuf,
    full: String,
}

/// Derive a store hash from the package name so tests don't have to spell
/// out 32 chars by hand. Not a real hash, just stable and unique per name.
fn fake_hash(name: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let mut s = String::with_capacity(32);
    let mut x = h;
    for _ in 0..32 {
        s.push(b"0123456789abcdfghijklmnpqrsvwxyz"[(x & 0x1f) as usize] as char);
        x = x.rotate_left(5) ^ h;
    }
    s
}

impl TestStore {
    fn new() -> Self {
        Self::new_inner(false)
    }

    /// Store dir is a symlink to the real directory, like /nix/store on
    /// installs with a relocated store. The kernel reports canonical
    /// paths for fds, the DB stores logical ones.
    fn new_symlinked() -> Self {
        Self::new_inner(true)
    }

    fn new_inner(symlink_store: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("store");
        let state_dir = dir.path().join("state");

        if symlink_store {
            let real = dir.path().join("store-real");
            fs::create_dir_all(&real).unwrap();
            std::os::unix::fs::symlink(&real, &store_dir).unwrap();
        } else {
            fs::create_dir_all(&store_dir).unwrap();
        }
        for d in ["db", "gcroots", "profiles", "temproots"] {
            fs::create_dir_all(state_dir.join(d)).unwrap();
        }
        fs::create_dir_all(store_dir.join(".links")).unwrap();

        let db = StoreDb::open(state_dir.join("db/db.sqlite"), OpenMode::Create).unwrap();
        db.create_schema().unwrap();

        TestStore {
            dir,
            store_dir,
            state_dir,
        }
    }

    fn db(&self) -> Connection {
        Connection::open(self.state_dir.join("db/db.sqlite")).unwrap()
    }

    fn add_path(&self, name: &str, nar_size: u64) -> Pkg {
        let hash = fake_hash(name);
        let basename = format!("{hash}-{name}");
        let path = self.store_dir.join(&basename);
        let full = format!("{}/{basename}", self.store_dir.display());

        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("file"), "x".repeat(nar_size as usize)).unwrap();

        let conn = self.db();
        conn.execute(
            "INSERT INTO ValidPaths (path, hash, registrationTime, narSize) VALUES (?, ?, 1000, ?)",
            rusqlite::params![full, format!("sha256:{hash}"), nar_size as i64],
        )
        .unwrap();
        Pkg {
            id: conn.last_insert_rowid(),
            path,
            full,
        }
    }

    fn add_ref(&self, referrer: &Pkg, reference: &Pkg) {
        self.db()
            .execute(
                "INSERT OR IGNORE INTO Refs (referrer, reference) VALUES (?, ?)",
                rusqlite::params![referrer.id, reference.id],
            )
            .unwrap();
    }

    fn set_deriver(&self, out: &Pkg, drv: &Pkg) {
        self.db()
            .execute(
                "UPDATE ValidPaths SET deriver = ? WHERE path = ?",
                rusqlite::params![drv.full, out.full],
            )
            .unwrap();
    }

    fn set_registration_time(&self, p: &Pkg, time: i64) {
        self.db()
            .execute(
                "UPDATE ValidPaths SET registrationTime = ? WHERE path = ?",
                rusqlite::params![time, p.full],
            )
            .unwrap();
    }

    fn add_drv_output(&self, drv: &Pkg, output_name: &str, out: &Pkg) {
        self.db()
            .execute(
                "INSERT INTO DerivationOutputs (drv, id, path) VALUES (?, ?, ?)",
                rusqlite::params![drv.id, output_name, out.full],
            )
            .unwrap();
    }

    /// Insert a BuildTraceV3 row (Nix ≥2.35 schema: drvPath is a store
    /// path basename, outputPath is a full store path).
    fn add_build_trace(&self, drv: &Pkg, output_name: &str, out: &Pkg) {
        let conn = self.db();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS BuildTraceV3 (
                id integer primary key autoincrement not null,
                drvPath text not null,
                outputName text not null,
                outputPath text not null,
                signatures text
            );
            CREATE INDEX IF NOT EXISTS IndexBuildTraceV3 ON BuildTraceV3(drvPath, outputName);",
        )
        .unwrap();
        // drvPath is a basename (no store dir prefix), matching Nix's
        // DrvOutput::to_string() which skips the store dir.
        let drv_basename = drv
            .full
            .strip_prefix(&format!("{}/", self.store_dir.display()))
            .unwrap_or(&drv.full);
        conn.execute(
            "INSERT INTO BuildTraceV3 (drvPath, outputName, outputPath) VALUES (?, ?, ?)",
            rusqlite::params![drv_basename, output_name, out.full],
        )
        .unwrap();
    }

    fn add_root(&self, root_name: &str, target: &Pkg) {
        let link = self.state_dir.join("gcroots").join(root_name);
        std::os::unix::fs::symlink(&target.path, &link).unwrap();
    }

    fn in_db(&self, p: &Pkg) -> bool {
        self.db()
            .query_row(
                "SELECT COUNT(*) FROM ValidPaths WHERE path = ?",
                [&p.full],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            > 0
    }

    fn run_gc(&self, extra_args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_harmonia-gc"))
            .arg("--store-dir")
            .arg(&self.store_dir)
            .arg("--state-dir")
            .arg(&self.state_dir)
            .args(extra_args)
            .output()
            .unwrap()
    }

    fn run_gc_ok(&self, extra_args: &[&str]) -> std::process::Output {
        let out = self.run_gc(extra_args);
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// Run GC with keep-outputs enabled via CLI flag.
    fn run_gc_keep_outputs_ok(&self) -> std::process::Output {
        self.run_gc_ok(&["--keep-outputs", "true"])
    }
}

#[test]
fn gc_deletes_unreferenced_paths() {
    let store = TestStore::new();

    let lib = store.add_path("lib", 100);
    let app = store.add_path("app", 200);
    store.add_ref(&app, &lib);
    store.add_root("app-root", &app);
    let dead = store.add_path("dead", 500);

    store.run_gc_ok(&[]);

    assert!(lib.path.exists() && app.path.exists());
    assert!(store.in_db(&lib) && store.in_db(&app));
    assert!(!dead.path.exists());
    assert!(!store.in_db(&dead));
}

#[test]
fn gc_dry_run_does_not_delete() {
    let store = TestStore::new();
    let dead = store.add_path("dead", 500);

    let out = store.run_gc_ok(&["--dry-run"]);

    assert!(dead.path.exists() && store.in_db(&dead));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // narSize of "dead" is 500; the estimate must be summed correctly.
    assert!(
        stdout.contains("1 store paths would be deleted (~500 bytes)"),
        "stdout: {stdout}"
    );
}

#[test]
fn gc_removes_unlocked_tmp_dir() {
    let store = TestStore::new();

    let tmp = store.store_dir.join("tmp-build-9999");
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("left-over"), "data").unwrap();

    let out = store.run_gc_ok(&[]);

    assert!(!tmp.exists(), "unlocked tmp dir is garbage");
    // No shared links exist, so no savings line must be logged.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("currently saving"), "stderr: {stderr}");
}

#[test]
fn gc_dry_run_counts_unknown_on_disk() {
    let store = TestStore::new();

    let unknown = store
        .store_dir
        .join(format!("{}-unknown", fake_hash("unknown")));
    fs::create_dir_all(&unknown).unwrap();

    let out = store.run_gc_ok(&["--dry-run"]);

    assert!(unknown.exists());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("1 store paths would be deleted (~0 bytes)"),
        "stdout: {stdout}"
    );
}

#[test]
fn gc_cleans_unused_links_only_when_not_dry_run() {
    let store = TestStore::new();

    let links = store.store_dir.join(".links");
    let dead_link = links.join("deadlink");
    fs::write(&dead_link, "unreferenced").unwrap();
    let shared_link = links.join("sharedlink");
    fs::write(&shared_link, "referenced").unwrap();
    let pkg = store.add_path("linked", 100);
    store.add_root("linked-root", &pkg);
    fs::hard_link(&shared_link, pkg.path.join("shared")).unwrap();
    fs::hard_link(&shared_link, pkg.path.join("shared2")).unwrap();

    store.run_gc_ok(&["--dry-run"]);
    assert!(dead_link.exists(), "dry run must not clean links");

    let out = store.run_gc_ok(&[]);
    assert!(!dead_link.exists(), "unreferenced link removed");
    assert!(shared_link.exists(), "referenced link kept");
    // "referenced" is 10 bytes with nlink 3 (one .links entry + two store
    // copies): dedup saves (3-2)*10 over independent copies.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("currently saving 10 bytes"),
        "stderr: {stderr}"
    );
}

#[test]
fn gc_keeps_transitive_closure() {
    let store = TestStore::new();

    let c = store.add_path("c", 10);
    let b = store.add_path("b", 10);
    let a = store.add_path("a", 10);
    store.add_ref(&a, &b);
    store.add_ref(&b, &c);
    store.add_root("root", &a);
    let trash = store.add_path("trash", 10);

    store.run_gc_ok(&[]);

    assert!(a.path.exists() && b.path.exists() && c.path.exists());
    assert!(!trash.path.exists());
}

#[test]
fn gc_handles_self_references() {
    let store = TestStore::new();

    let s = store.add_path("self-ref", 100);
    store.add_ref(&s, &s);
    store.add_root("root", &s);
    let trash = store.add_path("trash", 50);

    store.run_gc_ok(&[]);

    assert!(s.path.exists());
    assert!(!trash.path.exists());
}

#[test]
fn gc_removes_unknown_disk_entries() {
    let store = TestStore::new();

    let orphan = store
        .store_dir
        .join(format!("{}-orphan", fake_hash("orphan")));
    fs::create_dir_all(&orphan).unwrap();
    fs::write(orphan.join("stuff"), "data").unwrap();

    store.run_gc_ok(&[]);

    assert!(!orphan.exists());
}

/// A path registered after the graph snapshot, whose owner exited right
/// after the DB commit (stale temp root), shows up in the unknown-on-disk
/// scan. GC must not unlink it: that would leave a ValidPaths row without
/// a disk entry.
#[test]
fn gc_keeps_unknown_path_registered_after_snapshot() {
    let store = TestStore::new();

    // Anchor path so the store isn't empty.
    let root = store.add_path("root", 10);
    store.add_root("keep", &root);

    // On disk before GC starts, not yet in the DB: shows up in the
    // unknown-on-disk scan.
    let basename = format!("{}-late", fake_hash("late"));
    let late_path = store.store_dir.join(&basename);
    fs::create_dir_all(&late_path).unwrap();
    fs::write(late_path.join("file"), "data").unwrap();
    let late_full = format!("{}/{basename}", store.store_dir.display());

    // GC blocks on this fifo after the snapshot + unknown scan.
    let fifo = store.state_dir.join("sync.fifo");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits(0o600).unwrap()).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_harmonia-gc"))
        .arg("--store-dir")
        .arg(&store.store_dir)
        .arg("--state-dir")
        .arg(&store.state_dir)
        .env("_HARMONIA_GC_TEST_SYNC", &fifo)
        .spawn()
        .unwrap();

    // Blocks until GC opened the read end, i.e. the snapshot and the
    // unknown-on-disk scan are done.
    let fifo_w = fs::OpenOptions::new().write(true).open(&fifo).unwrap();

    // The "builder" registers the path. Its temp root file is already gone.
    store
        .db()
        .execute(
            "INSERT INTO ValidPaths (path, hash, registrationTime, narSize) VALUES (?, ?, 2000, 4)",
            rusqlite::params![late_full, format!("sha256:{}", fake_hash("late"))],
        )
        .unwrap();

    drop(fifo_w);
    let status = child.wait().unwrap();
    assert!(status.success());

    let in_db = store
        .db()
        .query_row(
            "SELECT COUNT(*) FROM ValidPaths WHERE path = ?",
            [&late_full],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        > 0;
    assert!(in_db, "registered path lost its ValidPaths row");
    assert!(
        late_path.exists(),
        "disk entry deleted while ValidPaths row exists (stale DB entry)"
    );
}

#[test]
fn gc_max_freed_stops_early() {
    let store = TestStore::new();
    store.add_path("dead1", 100);
    store.add_path("dead2", 100);

    let gc_store = GcStore::open(&store.store_dir, &store.state_dir).unwrap();
    let opts = GcOptions {
        max_freed: Some(1),
        ..Default::default()
    };
    let deleted = collect_garbage(&gc_store, &opts).unwrap().paths_deleted;

    assert_eq!(deleted, 1, "should stop after one path");
}

#[test]
fn gc_max_freed_batches_by_estimated_size() {
    let store = TestStore::new();
    // narSize 100 each, budget 250: first chunk = 3 paths (100+100+100 >= 250).
    // Real on-disk usage exceeds 250 after that, so the loop stops at 3.
    for i in 0..6 {
        store.add_path(&format!("dead{i}"), 100);
    }

    let gc_store = GcStore::open(&store.store_dir, &store.state_dir).unwrap();
    let opts = GcOptions {
        max_freed: Some(250),
        ..Default::default()
    };
    let deleted = collect_garbage(&gc_store, &opts).unwrap().paths_deleted;

    assert_eq!(deleted, 3, "first chunk must batch three paths by narSize");
}

#[test]
fn gc_max_freed_referrer_never_outlives_reference() {
    let store = TestStore::new();
    // dep has the lower id, so the first chunk picks it while top still
    // references it. Without referrer expansion the early stop leaves top
    // valid in the DB pointing at a deleted row.
    let dep = store.add_path("dead-dep", 100);
    let top = store.add_path("dead-top", 100);
    store.add_ref(&top, &dep);

    let gc_store = GcStore::open(&store.store_dir, &store.state_dir).unwrap();
    let opts = GcOptions {
        max_freed: Some(1), // stop after the first chunk
        ..Default::default()
    };
    collect_garbage(&gc_store, &opts).unwrap();

    let conn = store.db();
    let valid = |full: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM ValidPaths WHERE path = ?",
            [full],
            |r| r.get(0),
        )
        .unwrap()
    };
    // top surviving the early stop is fine, but only together with dep.
    assert!(
        valid(&top.full) == 0 || valid(&dep.full) == 1,
        "referrer outlived its reference in the DB"
    );
}

#[test]
fn gc_chunk_size_one_deletes_whole_dead_chain() {
    let store = TestStore::new();
    // A reference chain a -> b -> c, all dead. With one path per
    // transaction, referrer-first order must still delete every node and
    // never leave a surviving referrer pointing at a deleted reference.
    let c = store.add_path("dead-c", 100);
    let b = store.add_path("dead-b", 100);
    let a = store.add_path("dead-a", 100);
    store.add_ref(&a, &b);
    store.add_ref(&b, &c);

    let gc_store = GcStore::open(&store.store_dir, &store.state_dir).unwrap();
    let opts = GcOptions {
        chunk_size: 1,
        ..Default::default()
    };
    collect_garbage(&gc_store, &opts).unwrap();

    for p in [&a, &b, &c] {
        assert!(!p.path.exists(), "{} should be GC'd", p.full);
    }
    let conn = store.db();
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM ValidPaths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(remaining, 0, "all dead paths should be invalidated");
}

#[test]
fn gc_keep_recent_pins_recently_registered() {
    let store = TestStore::new();

    let old = store.add_path("old", 100);
    let recent = store.add_path("recent", 100);
    store.set_registration_time(&old, 1000);
    store.set_registration_time(&recent, 5000);

    let gc_store = GcStore::open(&store.store_dir, &store.state_dir).unwrap();
    let opts = GcOptions {
        keep_recent_after: Some(UNIX_EPOCH + Duration::from_secs(3000)),
        ..Default::default()
    };
    collect_garbage(&gc_store, &opts).unwrap();

    assert!(!old.path.exists(), "old path should be GC'd");
    assert!(
        recent.path.exists(),
        "recent path should survive --keep-recent"
    );
}

// libproc on macOS can't inspect other processes inside the nix
// sandbox. We can only test this via /proc on Linux.
#[cfg(target_os = "linux")]
#[test]
// pre_exec to pass an inherited fd needs unsafe.
#[allow(unsafe_code)]
fn gc_keeps_runtime_roots_from_open_fd() {
    use std::os::unix::process::CommandExt;
    let store = TestStore::new();

    let held = store.add_path("held", 100);
    let trash = store.add_path("trash", 100);

    // Inherit an fd into the child so the path shows up in the GC's
    // own /proc/<pid>/fd. Sandboxes can't always inspect other PIDs.
    let f = fs::File::open(held.path.join("file")).unwrap();
    let raw_fd = f.as_raw_fd();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_harmonia-gc"));
    cmd.arg("--store-dir")
        .arg(&store.store_dir)
        .arg("--state-dir")
        .arg(&store.state_dir);
    unsafe {
        cmd.pre_exec(move || {
            use nix::fcntl::{F_GETFD, F_SETFD, FdFlag, fcntl};
            use std::os::fd::BorrowedFd;
            let fd = BorrowedFd::borrow_raw(raw_fd);
            if let Ok(flags) = fcntl(fd, F_GETFD) {
                let mut flags = FdFlag::from_bits_truncate(flags);
                flags.remove(FdFlag::FD_CLOEXEC);
                let _ = fcntl(fd, F_SETFD(flags));
            }
            Ok(())
        });
    }
    let out = cmd.output().unwrap();
    drop(f);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(held.path.exists() && store.in_db(&held));
    assert!(!trash.path.exists());
}

#[test]
// pre_exec to pass an inherited fd needs unsafe.
#[allow(unsafe_code)]
fn gc_rebases_canonical_runtime_roots_to_logical_store() {
    use std::os::unix::process::CommandExt;
    let store = TestStore::new_symlinked();

    let held = store.add_path("held", 100);
    let trash = store.add_path("trash", 100);

    // Open via the canonical (symlink-resolved) path; /proc/<pid>/fd will
    // report that path, which must be rebased to the logical store prefix
    // before DB validation.
    let canon = fs::canonicalize(held.path.join("file")).unwrap();
    assert_ne!(canon, held.path.join("file"));
    let f = fs::File::open(&canon).unwrap();
    let raw_fd = f.as_raw_fd();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_harmonia-gc"));
    cmd.arg("--store-dir")
        .arg(&store.store_dir)
        .arg("--state-dir")
        .arg(&store.state_dir);
    unsafe {
        cmd.pre_exec(move || {
            use nix::fcntl::{F_GETFD, F_SETFD, FdFlag, fcntl};
            use std::os::fd::BorrowedFd;
            let fd = BorrowedFd::borrow_raw(raw_fd);
            if let Ok(flags) = fcntl(fd, F_GETFD) {
                let mut flags = FdFlag::from_bits_truncate(flags);
                flags.remove(FdFlag::FD_CLOEXEC);
                let _ = fcntl(fd, F_SETFD(flags));
            }
            Ok(())
        });
    }
    let out = cmd.output().unwrap();
    drop(f);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(held.path.exists() && store.in_db(&held));
    assert!(!trash.path.exists());
}

#[test]
fn gc_keeps_runtime_roots_from_environ() {
    let store = TestStore::new();

    let held = store.add_path("held", 100);
    let trash = store.add_path("trash", 100);

    // Pin via the child's environment, scanned from /proc/<pid>/environ.
    let out = Command::new(env!("CARGO_BIN_EXE_harmonia-gc"))
        .arg("--store-dir")
        .arg(&store.store_dir)
        .arg("--state-dir")
        .arg(&store.state_dir)
        .env("HARMONIA_GC_TEST_PIN", &held.full)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(held.path.exists() && store.in_db(&held));
    assert!(!trash.path.exists());
}

#[test]
fn gc_runtime_roots_validated_against_db() {
    let store = TestStore::new();

    // On disk and held open, but not in the DB. Runtime-root scan must
    // not pin an invalid path.
    let orphan = store
        .store_dir
        .join(format!("{}-orphan", fake_hash("orphan")));
    fs::create_dir_all(&orphan).unwrap();
    fs::write(orphan.join("file"), "data").unwrap();
    let _fd = fs::File::open(orphan.join("file")).unwrap();

    store.run_gc_ok(&[]);

    assert!(!orphan.exists());
}

#[test]
fn gc_skips_locked_tmp_dir() {
    let store = TestStore::new();

    let tmp = store.store_dir.join("tmp-build-1234");
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("in-progress"), "data").unwrap();

    let f = fs::File::open(&tmp).unwrap();
    let lock = nix::fcntl::Flock::lock(f, nix::fcntl::FlockArg::LockExclusiveNonblock)
        .expect("flock tmp dir");

    store.run_gc_ok(&[]);

    assert!(tmp.exists(), "locked tmp dir should survive");
    drop(lock);
}

#[test]
fn gc_deletes_readonly_paths() {
    use std::os::unix::fs::PermissionsExt;
    let store = TestStore::new();

    let ro = store.add_path("readonly", 100);
    fs::set_permissions(&ro.path, fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(ro.path.join("file"), fs::Permissions::from_mode(0o444)).unwrap();

    store.run_gc_ok(&[]);

    assert!(!ro.path.exists());
}

#[test]
fn gc_ignores_invalid_store_basenames() {
    let store = TestStore::new();

    // gcroot pointing at something inside the store dir that isn't a
    // store path (no hash-name prefix) — must not become a root.
    let bogus = store.store_dir.join("not-a-store-path");
    fs::create_dir_all(&bogus).unwrap();
    std::os::unix::fs::symlink(&bogus, store.state_dir.join("gcroots/bogus")).unwrap();

    let trash = store.add_path("trash", 100);

    store.run_gc_ok(&[]);

    assert!(!trash.path.exists());
    assert!(!bogus.exists());
}

#[test]
fn gc_keeps_deriver_of_alive_path() {
    let store = TestStore::new();

    let drv = store.add_path("pkg.drv", 50);
    let out = store.add_path("pkg", 200);
    store.set_deriver(&out, &drv);
    store.add_root("out-root", &out);
    let trash = store.add_path("trash", 100);

    store.run_gc_ok(&[]);

    // keep-derivations: rooted output keeps its .drv alive.
    assert!(out.path.exists() && drv.path.exists());
    assert!(!trash.path.exists());
}

#[test]
fn gc_deletes_drv_when_output_deriver_is_unset() {
    let store = TestStore::new();

    // Output with NULL deriver but a DerivationOutputs row: Nix's
    // keep-derivations pins drvs only via ValidPaths.deriver (gc.cc
    // requires queryPathInfo(out)->deriver == drv), so the drv is garbage.
    let drv = store.add_path("ca-pkg.drv", 50);
    let out = store.add_path("ca-pkg", 200);
    store.add_drv_output(&drv, "out", &out);
    store.add_root("ca-out-root", &out);

    store.run_gc_ok(&[]);

    assert!(out.path.exists(), "output deleted");
    assert!(
        !drv.path.exists(),
        "drv kept despite no alive path naming it as deriver"
    );
}

#[test]
fn gc_keeps_ca_outputs_of_alive_drv() {
    let store = TestStore::new();

    // Alive .drv should keep its outputs via DerivationOutputs.
    let drv = store.add_path("ca-pkg.drv", 50);
    let out = store.add_path("ca-pkg", 200);
    store.add_drv_output(&drv, "out", &out);
    store.add_root("drv-root", &drv);
    let trash = store.add_path("trash", 100);

    store.run_gc_keep_outputs_ok();

    // keep-outputs: alive .drv keeps its outputs alive.
    assert!(drv.path.exists(), "drv deleted");
    assert!(out.path.exists(), "CA output deleted despite alive drv");
    assert!(!trash.path.exists());
}

#[test]
fn gc_drops_partial_temp_root() {
    let store = TestStore::new();

    let complete = store.add_path("complete", 100);
    let partial = store.add_path("partial", 100);

    // Trailing entry without a NUL is a partial write — must not pin.
    let tmp_file = store.state_dir.join("temproots/99999");
    let mut data = complete.full.clone().into_bytes();
    data.push(0);
    data.extend_from_slice(partial.full.as_bytes());
    fs::write(&tmp_file, &data).unwrap();

    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&tmp_file)
        .unwrap();
    let lock = nix::fcntl::Flock::lock(f, nix::fcntl::FlockArg::LockSharedNonblock)
        .expect("flock temp roots file");

    store.run_gc_ok(&[]);

    assert!(complete.path.exists());
    assert!(!partial.path.exists());
    drop(lock);
}

#[test]
fn gc_keeps_temp_root_not_yet_in_db() {
    let store = TestStore::new();

    // Simulate a builder that wrote a temp root and materialized the
    // store path on disk, but hasn't registered it in ValidPaths yet.
    let hash = fake_hash("in-progress");
    let basename = format!("{hash}-in-progress");
    let path = store.store_dir.join(&basename);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("file"), "building").unwrap();

    let tmp_file = store.state_dir.join("temproots/12345");
    let mut data = format!("{}/{basename}", store.store_dir.display()).into_bytes();
    data.push(0);
    fs::write(&tmp_file, &data).unwrap();

    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&tmp_file)
        .unwrap();
    let lock = nix::fcntl::Flock::lock(f, nix::fcntl::FlockArg::LockSharedNonblock)
        .expect("flock temp roots file");

    store.run_gc_ok(&[]);

    assert!(
        path.exists(),
        "path with temp root deleted as unknown-on-disk"
    );
    drop(lock);
}

#[test]
fn gc_removes_stale_temp_roots_file() {
    let store = TestStore::new();

    // Temp roots file with no live owner — its roots are ignored.
    let dead = store.add_path("dead", 100);
    let tmp_file = store.state_dir.join("temproots/12345");
    let mut data = dead.full.clone().into_bytes();
    data.push(0);
    fs::write(&tmp_file, &data).unwrap();

    store.run_gc_ok(&[]);

    assert!(!tmp_file.exists());
    assert!(!dead.path.exists());
}

#[test]
fn gc_empty_store_is_noop() {
    let store = TestStore::new();
    let out = store.run_gc_ok(&[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("0 store paths deleted"), "stdout: {stdout}");
}

#[test]
fn gc_keeps_ca_output_via_build_trace() {
    // Dynamic / CA derivation: BuildTraceV3 maps drv→output.
    // With keep-outputs, alive drv keeps its CA output via BuildTraceV3.
    let store = TestStore::new();

    let drv = store.add_path("dyn-pkg.drv", 50);
    let out = store.add_path("dyn-pkg-out", 200);
    // No DerivationOutputs entry — only BuildTraceV3.
    store.add_build_trace(&drv, "out", &out);
    store.add_root("drv-root", &drv);
    let trash = store.add_path("trash", 100);

    store.run_gc_keep_outputs_ok();

    assert!(drv.path.exists(), "drv deleted");
    assert!(
        out.path.exists(),
        "CA output deleted despite alive drv (BuildTraceV3)"
    );
    assert!(!trash.path.exists());
}

#[test]
fn gc_keeps_ca_deriver_via_deriver_field_not_build_trace() {
    // An alive CA output keeps its drv only through ValidPaths.deriver,
    // like Nix; a BuildTraceV3 row alone must not pin the drv (the
    // output may have been rebuilt by a newer drv since).
    let store = TestStore::new();

    let stale_drv = store.add_path("dyn-pkg-old.drv", 50);
    let drv = store.add_path("dyn-pkg.drv", 50);
    let out = store.add_path("dyn-pkg-out", 200);
    store.add_build_trace(&stale_drv, "out", &out);
    store.add_build_trace(&drv, "out", &out);
    store.set_deriver(&out, &drv);
    store.add_root("out-root", &out);

    store.run_gc_ok(&[]);

    assert!(out.path.exists(), "output deleted");
    assert!(drv.path.exists(), "deriver deleted despite alive output");
    assert!(
        !stale_drv.path.exists(),
        "stale drv kept via BuildTraceV3 alone"
    );
}

#[test]
fn gc_store_dir_trailing_slash_is_normalized() {
    // "--store-dir /path/" must behave like "--store-dir /path"; an
    // unnormalized prefix would make every DB path look dead.
    let store = TestStore::new();
    let lib = store.add_path("lib", 100);
    store.add_root("lib-root", &lib);

    let out = Command::new(env!("CARGO_BIN_EXE_harmonia-gc"))
        .arg("--store-dir")
        .arg(format!("{}/", store.store_dir.display()))
        .arg("--state-dir")
        .arg(&store.state_dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(lib.path.exists() && store.in_db(&lib), "live path deleted");
}

#[test]
fn gc_refuses_store_dir_mismatching_db() {
    // A store dir that matches no DB path means the DB belongs to a
    // different store. Deleting "garbage" would wipe everything.
    let store = TestStore::new();
    let lib = store.add_path("lib", 100);
    store.add_root("lib-root", &lib);

    let other = store.dir.path().join("other-store");
    fs::create_dir_all(&other).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_harmonia-gc"))
        .arg("--store-dir")
        .arg(&other)
        .arg("--state-dir")
        .arg(&store.state_dir)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "GC must refuse a mismatched store dir"
    );
    assert!(lib.path.exists() && store.in_db(&lib));
}

#[test]
fn gc_fails_when_roots_dir_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        // Root bypasses permission checks, so EACCES cannot be simulated.
        return;
    }
    let store = TestStore::new();
    let lib = store.add_path("lib", 100);
    let sub = store.state_dir.join("gcroots/sub");
    fs::create_dir_all(&sub).unwrap();
    std::os::unix::fs::symlink(&lib.path, sub.join("lib-root")).unwrap();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();

    let out = store.run_gc(&[]);

    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        !out.status.success(),
        "GC must fail closed when a roots dir is unreadable"
    );
    assert!(lib.path.exists() && store.in_db(&lib), "live path deleted");
}

#[test]
fn gc_keeps_auto_root_with_unreadable_target() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        return; // root bypasses permission checks
    }
    // An EACCES on the indirect target says nothing about its existence;
    // the auto link must survive and the GC must fail closed.
    let store = TestStore::new();
    let lib = store.add_path("lib", 100);
    let auto = store.state_dir.join("gcroots/auto");
    fs::create_dir_all(&auto).unwrap();
    let private = store.dir.path().join("private");
    fs::create_dir_all(&private).unwrap();
    let user_link = private.join("result");
    std::os::unix::fs::symlink(&lib.path, &user_link).unwrap();
    std::os::unix::fs::symlink(&user_link, auto.join("x0")).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o000)).unwrap();

    let out = store.run_gc(&[]);

    fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!out.status.success(), "GC must fail closed");
    assert!(
        auto.join("x0").symlink_metadata().is_ok(),
        "auto root removed"
    );
    assert!(lib.path.exists());
}

#[test]
fn gc_keeps_indirect_auto_root() {
    // auto link -> user symlink -> store path. This covers ordinary
    // indirect-root retention. The next test covers an unfollowable second hop,
    // which is the property protected_symlinks exposes in practice.
    let store = TestStore::new();
    let lib = store.add_path("lib", 100);
    let auto = store.state_dir.join("gcroots/auto");
    fs::create_dir_all(&auto).unwrap();
    let user_link = store.dir.path().join("result");
    std::os::unix::fs::symlink(&lib.path, &user_link).unwrap();
    std::os::unix::fs::symlink(&user_link, auto.join("x0")).unwrap();

    store.run_gc_ok(&[]);

    assert!(
        lib.path.exists() && store.in_db(&lib),
        "indirect root deleted"
    );
    assert!(
        auto.join("x0").symlink_metadata().is_ok(),
        "live auto root removed"
    );
}

#[test]
fn gc_scans_roots_despite_unfollowable_indirect_target() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        return; // root bypasses permission checks
    }
    // A readable link whose target only fails to resolve *through* it
    // (here: second hop in a mode-000 dir, standing in for
    // fs.protected_symlinks, which a test cannot flip) is provably not a
    // store root — the scan must carry on, not abort.
    let store = TestStore::new();
    let lib = store.add_path("lib", 100);
    store.add_root("keep", &lib);
    let auto = store.state_dir.join("gcroots/auto");
    fs::create_dir_all(&auto).unwrap();
    let private = store.dir.path().join("private");
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("notes"), "").unwrap();
    let user_link = store.dir.path().join("result");
    std::os::unix::fs::symlink(private.join("notes"), &user_link).unwrap();
    std::os::unix::fs::symlink(&user_link, auto.join("x0")).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o000)).unwrap();

    let out = store.run_gc(&[]);

    fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        out.status.success(),
        "scan aborted on unfollowable non-store target: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(lib.path.exists() && store.in_db(&lib), "live path deleted");
    assert!(
        auto.join("x0").symlink_metadata().is_ok(),
        "auto root with unprovable target removed"
    );
}

#[test]
fn gc_removes_dangling_auto_root() {
    let store = TestStore::new();
    let auto = store.state_dir.join("gcroots/auto");
    fs::create_dir_all(&auto).unwrap();
    std::os::unix::fs::symlink(store.dir.path().join("gone"), auto.join("x1")).unwrap();

    store.run_gc_ok(&[]);

    assert!(
        auto.join("x1").symlink_metadata().is_err(),
        "dangling auto root must be removed"
    );
}

#[test]
fn gc_keeps_lock_file_of_active_build() {
    let store = TestStore::new();

    // Builder holds a temp root for the path it is building. Its .lock
    // sibling (not in the DB) shares the hash part and must survive.
    let alive = store.add_path("being-built", 100);
    let basename = alive
        .full
        .strip_prefix(&format!("{}/", store.store_dir.display()))
        .unwrap();
    let lock_file = store.store_dir.join(format!("{basename}.lock"));
    fs::write(&lock_file, "").unwrap();
    // Unrelated stale lock file: still garbage.
    let stale_lock = store
        .store_dir
        .join(format!("{}-stale.lock", fake_hash("stale")));
    fs::write(&stale_lock, "").unwrap();

    let tmp_file = store.state_dir.join("temproots/4242");
    let mut data = alive.full.clone().into_bytes();
    data.push(0);
    fs::write(&tmp_file, &data).unwrap();
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&tmp_file)
        .unwrap();
    let lock = nix::fcntl::Flock::lock(f, nix::fcntl::FlockArg::LockSharedNonblock).unwrap();

    store.run_gc_ok(&[]);

    assert!(lock_file.exists(), "active build's .lock file deleted");
    assert!(!stale_lock.exists(), "stale lock file kept");
    drop(lock);
}

// APFS rejects non-UTF-8 file names (EILSEQ), so this scenario can only be
// constructed on Linux.
#[cfg(target_os = "linux")]
#[test]
fn gc_deletes_non_utf8_store_entry() {
    let store = TestStore::new();
    let raw = std::ffi::OsStr::from_bytes(b"junk-\xff\xfe-entry");
    let path = store.store_dir.join(raw);
    match fs::write(&path, "garbage") {
        Ok(()) => {}
        // Filesystem enforces UTF-8 names (e.g. ZFS utf8only=on); the
        // scenario cannot exist here, so skip.
        Err(e) if e.raw_os_error() == Some(nix::libc::EILSEQ) => return,
        Err(e) => panic!("{e}"),
    }

    let out = store.run_gc_ok(&[]);

    assert!(
        path.symlink_metadata().is_err(),
        "non-UTF-8 garbage entry must be deleted"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 store paths deleted"), "stdout: {stdout}");
}

#[test]
fn gc_does_not_hang_on_tmp_fifo() {
    let store = TestStore::new();
    let fifo = store.store_dir.join("tmp-fifo");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits(0o644).unwrap()).unwrap();

    // Must terminate (no blocking open) and remove the FIFO.
    store.run_gc_ok(&[]);
    assert!(fifo.symlink_metadata().is_err(), "FIFO not collected");
}

#[test]
fn gc_removes_reserved_space_file_and_creates_private_lock() {
    use std::os::unix::fs::PermissionsExt;
    let store = TestStore::new();
    let reserved = store.state_dir.join("db/reserved");
    fs::write(&reserved, vec![b'X'; 1024]).unwrap();

    store.run_gc_ok(&[]);

    assert!(!reserved.exists(), "reserved space file must be freed");
    let mode = fs::metadata(store.state_dir.join("gc.lock"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "gc.lock must not be lockable by other users");
}

/// Connect to the gc-socket during the early window (before the graph is
/// loaded), register a root, and let the GC continue.
fn run_gc_with_early_root(
    store: &TestStore,
    root: &str,
    extra_args: &[&str],
) -> std::process::Output {
    use std::io::{Read, Write};

    let fifo = store.dir.path().join("sync-early");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits(0o600).unwrap()).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_harmonia-gc"))
        .arg("--store-dir")
        .arg(&store.store_dir)
        .arg("--state-dir")
        .arg(&store.state_dir)
        .args(extra_args)
        .env("_HARMONIA_GC_TEST_SYNC_EARLY", &fifo)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    // Blocks until the GC reached the sync point: lock held, early
    // socket up, graph not loaded yet.
    let fifo_w = fs::OpenOptions::new().write(true).open(&fifo).unwrap();

    let sock = store.state_dir.join("gc-socket/socket");
    let mut conn = std::os::unix::net::UnixStream::connect(&sock)
        .expect("gc-socket must be served before the graph is loaded");
    conn.write_all(format!("{root}\n").as_bytes()).unwrap();
    let mut ack = [0u8; 1];
    conn.read_exact(&mut ack).unwrap();
    assert_eq!(ack, [b'1'], "early root must be acked immediately");

    drop(fifo_w); // release the GC
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

#[test]
fn gc_socket_serves_before_graph_load_and_keeps_early_roots() {
    let store = TestStore::new();
    let dead = store.add_path("now-needed", 100);
    let trash = store.add_path("trash", 100);

    run_gc_with_early_root(&store, &dead.full, &[]);

    assert!(
        dead.path.exists() && store.in_db(&dead),
        "early-socket root was deleted"
    );
    assert!(!trash.path.exists());
}

#[test]
fn gc_dry_run_serves_socket_and_honors_roots() {
    let store = TestStore::new();
    let dead = store.add_path("now-needed", 100);
    let trash = store.add_path("trash", 100);

    let out = run_gc_with_early_root(&store, &dead.full, &["--dry-run"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains(&dead.full),
        "protected path reported as dead: {stdout}"
    );
    assert!(stdout.contains(&trash.full), "stdout: {stdout}");
    assert!(dead.path.exists() && trash.path.exists());
}

#[test]
fn gc_socket_root_mid_gc_keeps_path_and_deletes_rest() {
    // Mirror of the NixOS test's gc-socket phase: while the delete loop is
    // blocked on _HARMONIA_GC_TEST_SYNC, protect one dead path over the
    // socket. After release, it survives and the other dead path is gone.
    use std::io::{Read, Write};
    let store = TestStore::new();
    let saved = store.add_path("saved", 10);
    let other = store.add_path("other", 10);

    let fifo = store.dir.path().join("sync.fifo");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits(0o600).unwrap()).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_harmonia-gc"))
        .arg("--store-dir")
        .arg(&store.store_dir)
        .arg("--state-dir")
        .arg(&store.state_dir)
        .env("_HARMONIA_GC_TEST_SYNC", &fifo)
        .spawn()
        .unwrap();

    let fifo_w = fs::OpenOptions::new().write(true).open(&fifo).unwrap();

    let sock = store.state_dir.join("gc-socket/socket");
    let mut conn = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    conn.write_all(format!("{}\n", saved.full).as_bytes())
        .unwrap();
    let mut ack = [0u8; 1];
    conn.read_exact(&mut ack).unwrap();
    assert_eq!(ack, [b'1']);

    drop(fifo_w);
    let status = child.wait().unwrap();
    assert!(status.success());

    assert!(saved.path.exists() && store.in_db(&saved), "saved deleted");
    assert!(!other.path.exists(), "other survived");
    assert!(!store.in_db(&other));
}

#[test]
fn gc_vacuums_db_after_mass_deletion() {
    let store = TestStore::new();
    // Enough rows that deleting them leaves a freelist big enough to
    // trip the auto-vacuum heuristic (>25% free pages, >=64 pages).
    for i in 0..5000 {
        store.add_path(&format!("dead-{i}-padpadpadpadpadpadpadpadpad"), 1);
    }
    let db_path = store.state_dir.join("db/db.sqlite");
    let before = fs::metadata(&db_path).unwrap().len();

    let out = store.run_gc_ok(&[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("vacuum"), "stderr: {stderr}");

    let after = fs::metadata(&db_path).unwrap().len();
    assert!(
        after < before / 2,
        "db not vacuumed: before={before} after={after}"
    );
}
