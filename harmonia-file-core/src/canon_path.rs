//! A canonical relative path within a file tree.
//!
//! Invariants:
//! - No leading or trailing `/`
//! - No `.` or `..` components
//! - No empty components (no double slashes)
//! - Empty string represents the root of the tree

use std::fmt;

/// A canonical relative path within a file tree.
///
/// The empty path `""` represents the root. Components are separated
/// by `/` with no leading or trailing slash and no `.`/`..`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonPath(String);

/// Error when constructing a [`CanonPath`].
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid path component: {0}")]
pub struct CanonPathError(pub String);

impl CanonPath {
    /// The root of the file tree.
    pub const ROOT: CanonPath = CanonPath(String::new());

    /// Construct a `CanonPath`, validating invariants.
    pub fn new(raw: &str) -> Result<Self, CanonPathError> {
        let trimmed = raw.trim_matches('/');
        if trimmed.is_empty() {
            return Ok(Self(String::new()));
        }
        for component in trimmed.split('/') {
            if component.is_empty() {
                return Err(CanonPathError("empty component (double slash)".into()));
            }
            if component == "." || component == ".." {
                return Err(CanonPathError(format!("'{component}' not allowed")));
            }
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Construct without validation (caller guarantees invariants).
    pub const fn new_unchecked(path: String) -> Self {
        Self(path)
    }

    /// Whether this is the root path.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The path as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Iterate over the path components.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        if self.0.is_empty() {
            // Root has no components
            "".split('/').take(0)
        } else {
            self.0.split('/').take(usize::MAX)
        }
    }

    /// Append a child name to this path.
    pub fn join(&self, name: &str) -> Result<Self, CanonPathError> {
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(CanonPathError(format!("invalid child name: '{name}'")));
        }
        if self.0.is_empty() {
            Ok(Self(name.to_owned()))
        } else {
            Ok(Self(format!("{}/{name}", self.0)))
        }
    }

    /// Split into parent path and final component.
    /// Returns `None` for the root path.
    pub fn parent_and_name(&self) -> Option<(CanonPath, &str)> {
        if self.0.is_empty() {
            return None;
        }
        match self.0.rfind('/') {
            Some(pos) => Some((CanonPath(self.0[..pos].to_owned()), &self.0[pos + 1..])),
            None => Some((CanonPath::ROOT, &self.0)),
        }
    }
}

impl fmt::Debug for CanonPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CanonPath({:?})", self.0)
    }
}

impl fmt::Display for CanonPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            write!(f, ".")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

impl TryFrom<&str> for CanonPath {
    type Error = CanonPathError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root() {
        let p = CanonPath::new("").unwrap();
        assert!(p.is_root());
        assert_eq!(p.as_str(), "");
    }

    #[test]
    fn simple() {
        let p = CanonPath::new("foo/bar").unwrap();
        assert_eq!(p.as_str(), "foo/bar");
        let comps: Vec<_> = p.components().collect();
        assert_eq!(comps, ["foo", "bar"]);
    }

    #[test]
    fn strips_slashes() {
        let p = CanonPath::new("/foo/bar/").unwrap();
        assert_eq!(p.as_str(), "foo/bar");
    }

    #[test]
    fn rejects_dots() {
        assert!(CanonPath::new("foo/./bar").is_err());
        assert!(CanonPath::new("foo/../bar").is_err());
    }

    #[test]
    fn rejects_double_slash() {
        assert!(CanonPath::new("foo//bar").is_err());
    }

    #[test]
    fn join_works() {
        let root = CanonPath::ROOT;
        let foo = root.join("foo").unwrap();
        assert_eq!(foo.as_str(), "foo");
        let bar = foo.join("bar").unwrap();
        assert_eq!(bar.as_str(), "foo/bar");
    }

    #[test]
    fn parent_and_name() {
        let p = CanonPath::new("foo/bar/baz").unwrap();
        let (parent, name) = p.parent_and_name().unwrap();
        assert_eq!(parent.as_str(), "foo/bar");
        assert_eq!(name, "baz");

        let (pp, pn) = parent.parent_and_name().unwrap();
        assert_eq!(pp.as_str(), "foo");
        assert_eq!(pn, "bar");

        let (ppp, ppn) = pp.parent_and_name().unwrap();
        assert!(ppp.is_root());
        assert_eq!(ppn, "foo");

        assert!(ppp.parent_and_name().is_none());
    }
}
