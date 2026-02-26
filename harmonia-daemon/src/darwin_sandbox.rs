// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! macOS sandbox using `sandbox_init_with_parameters(3)`.
//!
//! Uses the same private API as Nix: `sandbox_init_with_parameters` from
//! `<sandbox.h>` is called in the child process after fork, before exec.
//! This is an undocumented but stable API used by all major browsers and
//! build systems on macOS.
//!
//! Profile structure matches Nix's `sandbox-defaults.sb`:
//! - `(deny default)` baseline
//! - Parameterized `_NIX_BUILD_TOP` and `_GLOBAL_TMP_DIR`
//! - Input paths added as `file-read* file-write* process-exec`
//! - Path ancestry for `file-read*` (allows `realpath()`)

use std::path::{Path, PathBuf};

/// The default sandbox profile, matching Nix's `sandbox-defaults.sb`.
///
/// Uses `(param ...)` for parameterized paths:
/// - `_NIX_BUILD_TOP`: the derivation's build directory
/// - `_GLOBAL_TMP_DIR`: the global temporary directory
const SANDBOX_DEFAULTS: &str = r#"
(define TMPDIR (param "_GLOBAL_TMP_DIR"))

; Disallow creating setuid/setgid binaries, since that
; would allow breaking build user isolation.
(deny file-write-setugid)

; Allow forking.
(allow process-fork)

; Allow reading system information like #CPUs, etc.
(allow sysctl-read)

; Allow POSIX semaphores and shared memory.
(allow ipc-posix*)

; Allow SYSV semaphores and shared memory.
(allow ipc-sysv*)

; Allow socket creation.
(allow system-socket)

; Allow sending signals within the sandbox.
(allow signal (target same-sandbox))

; Allow getpwuid.
(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo"))

; Access to /tmp and the build directory.
(allow file* process-exec network-outbound network-inbound
       (literal "/tmp")
       (subpath TMPDIR)
       (subpath (param "_NIX_BUILD_TOP")))

; Some packages like to read the system version.
(allow file-read*
       (literal "/System/Library/CoreServices/SystemVersion.plist")
       (literal "/System/Library/CoreServices/SystemVersionCompat.plist"))

; Without this line clang cannot write to /dev/null.
(allow file-read-metadata (literal "/dev"))

; Allow local networking when __darwinAllowLocalNetworking is set.
(if (param "_ALLOW_LOCAL_NETWORKING")
    (begin
      (allow network* (remote ip "localhost:*"))
      (allow network-inbound (local ip "*:*")) ; required to bind and listen

      ; Allow access to /etc/resolv.conf (which is a symlink to
      ; /private/var/run/resolv.conf).
      (allow file-read-metadata
             (literal "/var")
             (literal "/etc")
             (literal "/etc/resolv.conf")
             (literal "/private/etc/resolv.conf"))

      (allow file-read*
             (literal "/private/var/run/resolv.conf"))

      ; Allow DNS lookups. This is even needed for localhost, which lots of tests rely on
      (allow file-read-metadata (literal "/etc/hosts"))
      (allow file-read*         (literal "/private/etc/hosts"))
      (allow network-outbound (remote unix-socket (path-literal "/private/var/run/mDNSResponder")))))

; Standard devices.
(allow file*
       (literal "/dev/null")
       (literal "/dev/random")
       (literal "/dev/stderr")
       (literal "/dev/stdin")
       (literal "/dev/stdout")
       (literal "/dev/tty")
       (literal "/dev/urandom")
       (literal "/dev/zero")
       (subpath "/dev/fd"))

; Allow pseudo-terminals.
(allow file*
       (literal "/dev/ptmx")
       (regex #"^/dev/pty[a-z]+")
       (regex #"^/dev/ttys[0-9]+"))

; Does nothing, but reduces build noise.
(allow file* (literal "/dev/dtracehelper"))

; Allow access to zoneinfo since libSystem needs it.
(allow file-read* (subpath "/usr/share/zoneinfo"))

(allow file-read* (subpath "/usr/share/locale"))

; Metadata access for better error messages.
(allow file-read-metadata
       (literal "/etc")
       (literal "/var")
       (literal "/private/var/tmp"))

; Used by /bin/sh on macOS 10.15+.
(allow file*
       (literal "/private/var/select/sh"))

; Allow Rosetta 2 to run x86_64 binaries on aarch64-darwin.
(allow file-read*
       (subpath "/Library/Apple/usr/libexec/oah")
       (subpath "/System/Library/Apple/usr/libexec/oah")
       (subpath "/System/Library/LaunchDaemons/com.apple.oahd.plist")
       (subpath "/Library/Apple/System/Library/LaunchDaemons/com.apple.oahd.plist"))
"#;

/// Network access rules, matching Nix's `sandbox-network.sb`.
const SANDBOX_NETWORK: &str = r#"
; Allow local and remote network traffic.
(allow network* (local ip) (remote ip))

; Allow access to /etc/resolv.conf.
(allow file-read-metadata
       (literal "/var")
       (literal "/etc")
       (literal "/etc/resolv.conf")
       (literal "/private/etc/resolv.conf"))

(allow file-read*
       (literal "/private/var/run/resolv.conf"))

; Allow DNS lookups.
(allow network-outbound (remote unix-socket (path-literal "/private/var/run/mDNSResponder")))
(allow mach-lookup (global-name "com.apple.SystemConfiguration.DNSConfiguration"))

; Allow access to trustd.
(allow mach-lookup (global-name "com.apple.trustd"))
(allow mach-lookup (global-name "com.apple.trustd.agent"))
"#;

/// Minimal sandbox profile (used when `sandbox = false` on macOS).
/// Still prevents creating setuid/setgid binaries.
const SANDBOX_MINIMAL: &str = r#"
(allow default)

; Disallow creating setuid/setgid binaries.
(deny file-write-setugid)
"#;

/// Build a complete sandbox profile string for a derivation.
///
/// Matches Nix's `DarwinDerivationBuilder::setUser()` profile construction:
/// - `(deny default (with no-log))` baseline
/// - `SANDBOX_DEFAULTS` rules (devices, IPC, signals, etc.)
/// - Output paths get `file-read* file-write* process-exec`
/// - Input paths get `file-read* file-write* process-exec` (matching Nix:
///   "without file-write* allowed, access() incorrectly returns EPERM")
/// - Path ancestry gets `file-read*` (allows `realpath()`)
/// - Network rules for unsandboxed (fixed-output) derivations
pub fn generate_sandbox_profile(
    input_paths: &[PathBuf],
    output_paths: &[PathBuf],
    allow_network: bool,
    use_sandbox: bool,
    store_dir: &Path,
    additional_profile: &str,
) -> String {
    let mut profile = "(version 1)\n".to_string();

    if !use_sandbox {
        profile.push_str(SANDBOX_MINIMAL);
        return profile;
    }

    // Suppress syslog noise from sandbox violations. The syslog
    // destination is not configurable on macOS, matching Nix's default.
    profile.push_str("(deny default (with no-log))\n");

    profile.push_str(SANDBOX_DEFAULTS);

    if allow_network {
        profile.push_str(SANDBOX_NETWORK);
    }

    // Allow read/write/exec on output paths
    if !output_paths.is_empty() {
        profile.push_str("(allow file-read* file-write* process-exec\n");
        for path in output_paths {
            profile.push_str(&format!("\t(subpath \"{}\")\n", path.display()));
        }
        profile.push_str(")\n");
    }

    // Allow read/write/exec on input paths.
    // Matching Nix: "without file-write* allowed, access() incorrectly returns EPERM"
    //
    // Split into multiple allow groups to avoid exceeding the sandbox
    // interpreter's expression limit (see NixOS/nix#4119). We split
    // approximately at half the actual limit (1 << 14 bytes per group).
    let mut ancestry = std::collections::BTreeSet::new();
    let mut current_group = String::new();
    let breakpoint = 1 << 14;

    for path in input_paths {
        let md = std::fs::symlink_metadata(path);
        if let Ok(md) = md {
            let rule = if md.is_dir() {
                format!("\t(subpath \"{}\")\n", path.display())
            } else {
                format!("\t(literal \"{}\")\n", path.display())
            };

            // Start a new group if the current one is getting large
            if current_group.len() + rule.len() > breakpoint && !current_group.is_empty() {
                profile.push_str("(allow file-read* file-write* process-exec\n");
                profile.push_str(&current_group);
                profile.push_str(")\n");
                current_group.clear();
            }
            current_group.push_str(&rule);
        }

        // Collect ancestor directories for realpath() support.
        // Must include "/" — without file-read* on root, path
        // resolution fails and the sandbox aborts the process.
        let mut cur = path.to_path_buf();
        while let Some(parent) = cur.parent() {
            ancestry.insert(parent.to_path_buf());
            if parent == Path::new("/") {
                break;
            }
            cur = parent.to_path_buf();
        }
    }

    // Nix explicitly adds the store directory and its ancestors so that
    // realpath() works even when input_paths is empty. Include the store
    // dir itself (e.g. /nix/store) plus all parents up to /.
    {
        let mut cur = store_dir.to_path_buf();
        loop {
            ancestry.insert(cur.clone());
            match cur.parent() {
                Some(parent) if parent != cur => cur = parent.to_path_buf(),
                _ => break,
            }
        }
    }

    // Flush remaining input paths
    if !current_group.is_empty() {
        profile.push_str("(allow file-read* file-write* process-exec\n");
        profile.push_str(&current_group);
        profile.push_str(")\n");
    }

    // Allow file-read* on ancestor directories (allows realpath())
    if !ancestry.is_empty() {
        profile.push_str("(allow file-read*\n");
        for anc in &ancestry {
            profile.push_str(&format!("\t(literal \"{}\")\n", anc.display()));
        }
        profile.push_str(")\n");
    }

    profile.push_str(additional_profile);

    profile
}

/// Apply the sandbox profile in the current process using `sandbox_init_with_parameters`.
///
/// This must be called after fork(), before exec() — typically from a
/// `pre_exec` hook. Once applied, the sandbox cannot be removed.
///
/// `build_top`: the derivation's temporary build directory
/// `global_tmp_dir`: the global temp directory (canonicalized, no trailing slash)
///
/// # Safety
///
/// This calls a C function that modifies process-wide state. It must only
/// be called once per process, in a post-fork child before exec.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub unsafe fn apply_sandbox(
    profile: &str,
    build_top: &Path,
    global_tmp_dir: &Path,
    allow_local_networking: bool,
) -> Result<(), String> {
    use std::ffi::CString;
    use std::ptr;

    unsafe extern "C" {
        fn sandbox_init_with_parameters(
            profile: *const libc::c_char,
            flags: u64,
            parameters: *const *const libc::c_char,
            errorbuf: *mut *mut libc::c_char,
        ) -> libc::c_int;

        fn sandbox_free_error(errorbuf: *mut libc::c_char);
    }

    let profile_cstr =
        CString::new(profile).map_err(|e| format!("profile contains null byte: {e}"))?;

    let build_top_str = build_top.to_string_lossy();
    let global_tmp_str = global_tmp_dir.to_string_lossy();

    // Parameters are key-value pairs as a null-terminated array of C strings.
    let key1 = CString::new("_NIX_BUILD_TOP").unwrap();
    let val1 = CString::new(build_top_str.as_ref()).map_err(|e| format!("build_top: {e}"))?;
    let key2 = CString::new("_GLOBAL_TMP_DIR").unwrap();
    let val2 = CString::new(global_tmp_str.as_ref()).map_err(|e| format!("global_tmp_dir: {e}"))?;

    // Conditionally pass _ALLOW_LOCAL_NETWORKING for packages that need
    // localhost access (e.g. test suites using __darwinAllowLocalNetworking).
    let key3 = CString::new("_ALLOW_LOCAL_NETWORKING").unwrap();
    let val3 = CString::new("1").unwrap();

    let mut params: Vec<*const libc::c_char> =
        vec![key1.as_ptr(), val1.as_ptr(), key2.as_ptr(), val2.as_ptr()];
    if allow_local_networking {
        params.push(key3.as_ptr());
        params.push(val3.as_ptr());
    }
    params.push(ptr::null());

    let mut errbuf: *mut libc::c_char = ptr::null_mut();
    // SAFETY: sandbox_init_with_parameters is the documented macOS sandbox API.
    // We pass valid C strings and a null-terminated parameter array.
    let ret = unsafe {
        sandbox_init_with_parameters(profile_cstr.as_ptr(), 0, params.as_ptr(), &mut errbuf)
    };

    if ret != 0 {
        let msg = if !errbuf.is_null() {
            // SAFETY: errbuf is a valid C string allocated by the sandbox API.
            let s = unsafe {
                std::ffi::CStr::from_ptr(errbuf)
                    .to_string_lossy()
                    .into_owned()
            };
            unsafe { sandbox_free_error(errbuf) };
            s
        } else {
            "(null)".to_string()
        };
        Err(format!("sandbox_init_with_parameters failed: {msg}"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sandboxed profile uses `(deny default)`, minimal uses `(allow default)`.
    #[test]
    fn test_sandbox_vs_minimal_profile() {
        let sandboxed =
            generate_sandbox_profile(&[], &[], false, true, Path::new("/nix/store"), "");
        assert!(sandboxed.contains("(deny default (with no-log))"));
        // Should not contain the unconditional full network access from SANDBOX_NETWORK
        assert!(!sandboxed.contains("(allow network* (local ip) (remote ip))"));

        let minimal = generate_sandbox_profile(&[], &[], false, false, Path::new("/nix/store"), "");
        assert!(minimal.contains("(allow default)"));
        assert!(minimal.contains("(deny file-write-setugid)"));
    }

    /// Fixed-output derivations get network access rules.
    #[test]
    fn test_network_access_for_fixed_output() {
        let without = generate_sandbox_profile(&[], &[], false, true, Path::new("/nix/store"), "");
        // Should not contain the unconditional full network access from SANDBOX_NETWORK
        assert!(!without.contains("(allow network* (local ip) (remote ip))"));

        let with = generate_sandbox_profile(&[], &[], true, true, Path::new("/nix/store"), "");
        assert!(with.contains("(allow network* (local ip) (remote ip))"));
        assert!(with.contains("mDNSResponder"));
    }

    /// Empty input/output paths produce no empty `(allow ...)` groups
    /// which would grant unrestricted access.
    #[test]
    fn test_no_empty_allow_groups() {
        let profile = generate_sandbox_profile(&[], &[], false, true, Path::new("/nix/store"), "");
        // Should NOT contain "(allow file-read* file-write* process-exec\n)"
        // with no subpath/literal entries — that would allow all writes.
        assert!(
            !profile.contains("(allow file-read* file-write* process-exec\n)"),
            "Empty allow groups would grant unrestricted access"
        );
    }

    /// Input paths generate correct `subpath` (dir) vs `literal` (file) rules,
    /// plus ancestry `file-read*` entries.
    #[test]
    fn test_input_path_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_path = tmp.path().join("some_dir");
        std::fs::create_dir(&dir_path).unwrap();
        let file_path = tmp.path().join("some_file");
        std::fs::write(&file_path, "x").unwrap();

        let profile = generate_sandbox_profile(
            &[dir_path.clone(), file_path.clone()],
            &[],
            false,
            true,
            Path::new("/nix/store"),
            "",
        );

        assert!(
            profile.contains(&format!("(subpath \"{}\")", dir_path.display())),
            "Directories should use (subpath ...)"
        );
        assert!(
            profile.contains(&format!("(literal \"{}\")", file_path.display())),
            "Files should use (literal ...)"
        );
        // Ancestry
        assert!(
            profile.contains(&format!("(literal \"{}\")", tmp.path().display())),
            "Ancestor directories should get file-read* (literal ...)"
        );
        // Store dir ancestry is always included so realpath() works
        assert!(
            profile.contains("(literal \"/nix\")"),
            "Store dir ancestry must include /nix"
        );
        assert!(
            profile.contains("(literal \"/\")"),
            "Store dir ancestry must include /"
        );
    }

    /// Nix's sandbox-defaults.sb has a conditional `_ALLOW_LOCAL_NETWORKING`
    /// block for packages using `__darwinAllowLocalNetworking`. The SBPL
    /// text must always contain the `(if (param ...))` block; the parameter
    /// value controls activation at runtime.
    #[test]
    fn test_sandbox_defaults_has_local_networking_conditional() {
        let profile = generate_sandbox_profile(&[], &[], false, true, Path::new("/nix/store"), "");
        assert!(
            profile.contains(r#"(if (param "_ALLOW_LOCAL_NETWORKING")"#),
            "Profile must contain the _ALLOW_LOCAL_NETWORKING conditional block"
        );
        assert!(
            profile.contains(r#"(allow network* (remote ip "localhost:*"))"#),
            "Local networking conditional must allow localhost connections"
        );
    }

    /// Additional profile text is appended.
    #[test]
    fn test_additional_profile() {
        let extra = "(allow mach-lookup (global-name \"com.example.test\"))\n";
        let profile =
            generate_sandbox_profile(&[], &[], false, true, Path::new("/nix/store"), extra);
        assert!(profile.contains(extra));
    }

    /// `apply_sandbox` with Nix-compatible profile blocks writes outside
    /// the build directory. Uses `sandbox_init_with_parameters` via
    /// `pre_exec`, matching Nix's `DarwinDerivationBuilder::setUser()`.
    #[test]
    #[cfg(target_os = "macos")]
    #[allow(unsafe_code)]
    fn test_sandbox_write_isolation() {
        use std::os::unix::process::CommandExt;

        // macOS doesn't allow nesting sandbox profiles: if we're already
        // inside a nix build sandbox, sandbox_init_with_parameters returns
        // EINVAL. The nix test derivation sets this env var on Darwin.
        if std::env::var("_NIX_TEST_NO_SANDBOX").is_ok() {
            eprintln!("skipping: cannot nest macOS sandbox profiles inside nix build sandbox");
            return;
        }

        let build_dir = tempfile::tempdir().unwrap();
        let build_top = build_dir.path().canonicalize().unwrap();

        // Blocked dir must be outside _NIX_BUILD_TOP, _GLOBAL_TMP_DIR,
        // and all sandbox-paths (/private/tmp, /private/var/tmp, /usr/lib, etc.).
        // Use the default tempdir ($TMPDIR, typically /private/var/folders/...)
        // which is not in the sandbox allow-list.
        let blocked_dir = tempfile::Builder::new()
            .prefix("sandbox-blocked-")
            .tempdir()
            .unwrap();
        let blocked_resolved = blocked_dir.path().canonicalize().unwrap();
        let blocked_file = blocked_resolved.join("blocked.txt");

        // Matching Nix's default `sandbox-paths` config on macOS.
        // These are the host paths that every sandboxed build needs
        // (see `nix show-config | grep sandbox-paths`).
        let input_paths: Vec<PathBuf> = [
            "/System/Library/Frameworks",
            "/System/Library/PrivateFrameworks",
            "/bin/bash",
            "/bin/sh",
            "/private/tmp",
            "/private/var/tmp",
            "/usr/lib",
        ]
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect();

        let profile =
            generate_sandbox_profile(&input_paths, &[], false, true, Path::new("/nix/store"), "");

        // Single script: write to build dir (proves sandbox is active),
        // then attempt write to blocked dir (should fail).
        let verify_file = build_top.join("verify.txt");
        let script = format!(
            "printf 'ok' > '{}' && printf 'bad' > '{}'",
            verify_file.display(),
            blocked_file.display(),
        );

        let profile_clone = profile.clone();
        let build_top_clone = build_top.clone();
        // Use build_top as global tmp too (both are under sandbox control)
        let global_tmp = build_top.clone();

        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args(["-c", &script])
            .stderr(std::process::Stdio::null());

        // SAFETY: apply_sandbox is called in the post-fork child before exec.
        // It only affects the child process, matching Nix's setUser() pattern.
        unsafe {
            cmd.pre_exec(move || {
                apply_sandbox(&profile_clone, &build_top_clone, &global_tmp, false)
                    .map_err(std::io::Error::other)
            });
        }

        let status = cmd.status().unwrap();

        assert_eq!(
            std::fs::read_to_string(&verify_file).unwrap(),
            "ok",
            "Write to build dir must succeed (proves sandbox is active)"
        );
        assert!(
            !status.success(),
            "Command should fail because write to blocked dir is denied"
        );
        assert!(
            !blocked_file.exists(),
            "Blocked file should not have been created"
        );
    }
}
