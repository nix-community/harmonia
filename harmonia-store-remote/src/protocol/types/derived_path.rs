use crate::protocol::types::OutputName;
use harmonia_store_core::StorePath;
use std::collections::BTreeSet;
use std::fmt;

/// Specification of which outputs to build/substitute
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputsSpec {
    /// Build/substitute all outputs
    All,
    /// Build/substitute only the specified outputs
    Names(BTreeSet<OutputName>),
}

impl OutputsSpec {
    /// Check if this spec includes the given output name
    pub fn contains(&self, name: &OutputName) -> bool {
        match self {
            Self::All => true,
            Self::Names(names) => names.contains(name),
        }
    }

    /// Returns true if this is the All variant
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    /// Parse from bytes (e.g., b"*" or b"out,bin,dev")
    pub fn parse(s: &[u8]) -> Result<Self, String> {
        if s == b"*" {
            Ok(Self::All)
        } else if s.is_empty() {
            Ok(Self::Names(BTreeSet::new()))
        } else {
            let names = s
                .split(|&b| b == b',')
                .map(OutputName::from_bytes)
                .collect::<Result<BTreeSet<_>, _>>()
                .map_err(|e| e.to_string())?;
            Ok(Self::Names(names))
        }
    }
}

impl fmt::Display for OutputsSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => write!(f, "*"),
            Self::Names(names) => {
                let names_vec: Vec<String> = names.iter().map(|n| n.to_string()).collect();
                write!(f, "{}", names_vec.join(","))
            }
        }
    }
}

/// A path that may need to be built or substituted
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedPath {
    /// An opaque store path (already built)
    Opaque(StorePath),
    /// A derivation with specific outputs to build
    Built(StorePath, OutputsSpec),
}

impl DerivedPath {
    /// Get the store path component
    pub fn path(&self) -> &StorePath {
        match self {
            Self::Opaque(path) | Self::Built(path, _) => path,
        }
    }

    /// Returns true if this is an opaque path
    pub fn is_opaque(&self) -> bool {
        matches!(self, Self::Opaque(_))
    }

    /// Returns true if this is a built path
    pub fn is_built(&self) -> bool {
        matches!(self, Self::Built(_, _))
    }

    /// Parse from bytes representation
    /// Format: b"/nix/store/..." or b"/nix/store/...!out,bin"
    pub fn parse(s: &[u8]) -> Result<Self, String> {
        if let Some(pos) = s.iter().position(|&b| b == b'!') {
            let path = StorePath::from(s[..pos].to_vec());
            let outputs = OutputsSpec::parse(&s[pos + 1..])?;
            Ok(Self::Built(path, outputs))
        } else {
            let path = StorePath::from(s.to_vec());
            Ok(Self::Opaque(path))
        }
    }
}

impl fmt::Display for DerivedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opaque(path) => write!(f, "{}", String::from_utf8_lossy(path.as_ref())),
            Self::Built(path, outputs) => {
                write!(f, "{}!{}", String::from_utf8_lossy(path.as_ref()), outputs)
            }
        }
    }
}
