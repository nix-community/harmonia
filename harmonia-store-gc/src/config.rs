// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Read resolved nix.conf settings via `nix config show`. Reusing Nix's
//! own config resolution avoids reimplementing the multi-file parser.

use std::process::Command;

use tracing::warn;

/// `nix config show <key>` parsed as a bool. Returns `default` if `nix`
/// is not in PATH, the key is unknown, or the value is not a bool.
///
/// ```no_run
/// use harmonia_store_gc::config::bool_setting;
///
/// // Falls back to the default when the setting does not exist.
/// assert!(bool_setting("no-such-setting", true));
/// ```
pub fn bool_setting(key: &str, default: bool) -> bool {
    // `nix config show` is gated on nix-command. Pass the flag so this
    // works without it in nix.conf.
    let out = Command::new("nix")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        assert_eq!(parse_bool(b"true\n"), Some(true));
        assert_eq!(parse_bool(b"false\n"), Some(false));
        assert_eq!(parse_bool(b"42\n"), None);
        assert_eq!(parse_bool(b""), None);
    }

    #[test]
    fn unknown_key_falls_back_to_default() {
        assert!(bool_setting("this-setting-does-not-exist", true));
        assert!(!bool_setting("this-setting-does-not-exist", false));
    }
}
