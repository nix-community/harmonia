//! The `nix-cache-info` file at the root of a binary cache.

use std::fmt;
use std::str::FromStr;

use harmonia_store_path::StoreDir;

/// Contents of a binary cache's `nix-cache-info` file.
///
/// Every field is `None` when the file leaves it out. Nix only uses these
/// values as defaults for the substituter's settings, so a missing field is
/// not the same as Nix's default value.
///
/// Parsing does not check [`store_dir`](Self::store_dir) against the local
/// store. Nix refuses a cache for a different store directory, so clients
/// have to compare it themselves.
///
/// # Examples
///
/// ```
/// use harmonia_store_nar_info::CacheInfo;
///
/// let text = "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 40\n";
/// let info: CacheInfo = text.parse().unwrap();
/// assert_eq!(info.priority, Some(40));
/// assert_eq!(info.to_string(), text);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheInfo {
    /// `StoreDir`: the store directory of the cache's paths.
    pub store_dir: Option<StoreDir>,
    /// `WantMassQuery`: whether Nix may query the cache for many paths at once.
    pub want_mass_query: Option<bool>,
    /// `Priority`: a lower value makes Nix try the cache earlier.
    pub priority: Option<i32>,
}

/// Error returned when a `nix-cache-info` line has a value that cannot be parsed.
#[derive(Debug, thiserror::Error)]
pub enum CacheInfoParseError {
    /// The value of a known key is malformed.
    #[error("line {line}: invalid {field} ({message})")]
    InvalidField {
        /// One-based line number in the input.
        line: usize,
        /// Key of the offending line.
        field: &'static str,
        /// Why the value was rejected.
        message: String,
    },
}

/// Parses the way Nix does: unknown keys and lines without a `:` are ignored,
/// and a later line overrides an earlier one with the same key.
impl FromStr for CacheInfo {
    type Err = CacheInfoParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut info = CacheInfo::default();
        for (i, line) in s.lines().enumerate() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            let invalid =
                |field: &'static str, e: &dyn fmt::Display| CacheInfoParseError::InvalidField {
                    line: i + 1,
                    field,
                    message: e.to_string(),
                };
            match key {
                "StoreDir" => {
                    info.store_dir =
                        Some(StoreDir::new(value).map_err(|e| invalid("StoreDir", &e))?);
                }
                "WantMassQuery" => info.want_mass_query = Some(value == "1"),
                "Priority" => {
                    info.priority = Some(value.parse().map_err(|e| invalid("Priority", &e))?);
                }
                _ => {}
            }
        }
        Ok(info)
    }
}

/// Writes one `Key: value` line for each field that is set.
impl fmt::Display for CacheInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(store_dir) = &self.store_dir {
            writeln!(f, "StoreDir: {store_dir}")?;
        }
        if let Some(want_mass_query) = self.want_mass_query {
            writeln!(f, "WantMassQuery: {}", u8::from(want_mass_query))?;
        }
        if let Some(priority) = self.priority {
            writeln!(f, "Priority: {priority}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn info(
        store_dir: Option<&str>,
        want_mass_query: Option<bool>,
        priority: Option<i32>,
    ) -> CacheInfo {
        CacheInfo {
            store_dir: store_dir.map(|s| StoreDir::new(s).unwrap()),
            want_mass_query,
            priority,
        }
    }

    #[rstest]
    #[case::cache_nixos_org(
        "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 40\n",
        info(Some("/nix/store"), Some(true), Some(40))
    )]
    #[case::empty("", info(None, None, None))]
    #[case::whitespace_and_crlf(
        "StoreDir:/nix/store  \r\nPriority:\t-5\r\n",
        info(Some("/nix/store"), None, Some(-5))
    )]
    #[case::only_one_is_true("WantMassQuery: true\n", info(None, Some(false), None))]
    #[case::value_with_colon("StoreDir: /a:b\n", info(Some("/a:b"), None, None))]
    #[case::last_wins("Priority: 10\nPriority: 20\n", info(None, None, Some(20)))]
    #[case::skips_unknown_and_malformed(
        "garbage\n\nStoreDir : /x\nFoo: bar\nPriority: 1",
        info(None, None, Some(1))
    )]
    fn parse(#[case] text: &str, #[case] expected: CacheInfo) {
        assert_eq!(text.parse::<CacheInfo>().unwrap(), expected);
    }

    #[rstest]
    #[case("Priority: high")]
    #[case("Priority:")]
    #[case("Priority: 99999999999")]
    fn parse_invalid_priority(#[case] text: &str) {
        let err = text.parse::<CacheInfo>().unwrap_err();
        assert!(matches!(
            err,
            CacheInfoParseError::InvalidField {
                line: 1,
                field: "Priority",
                ..
            }
        ));
    }

    #[rstest]
    #[case(info(None, None, None), "")]
    #[case(
        info(Some("/nix/store"), Some(true), Some(30)),
        "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 30\n"
    )]
    #[case(info(None, Some(false), Some(-1)), "WantMassQuery: 0\nPriority: -1\n")]
    fn display_round_trips(#[case] info: CacheInfo, #[case] text: &str) {
        assert_eq!(info.to_string(), text);
        assert_eq!(text.parse::<CacheInfo>().unwrap(), info);
    }
}
