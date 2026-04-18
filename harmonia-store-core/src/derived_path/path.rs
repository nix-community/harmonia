use std::ops::Deref as _;
use std::sync::Arc;

#[cfg(any(test, feature = "test"))]
use proptest::prelude::{Arbitrary, BoxedStrategy};
use serde::{Deserialize, Serialize};

use crate::store_path::{
    FromStoreDirStr, ParseStorePathError, StoreDir, StoreDirDisplay, StorePath, StorePathError,
};

use super::{OutputName, OutputSpec};

trait DisplaySep {
    fn fmt(&self, sep: char, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

impl<D> DisplaySep for &D
where
    D: DisplaySep,
{
    fn fmt(&self, sep: char, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(sep, f)
    }
}

impl<D> DisplaySep for Arc<D>
where
    D: DisplaySep,
{
    fn fmt(&self, sep: char, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(sep, f)
    }
}

trait FromStrSep: Sized {
    fn from_str_sep(sep: char, s: &str) -> Result<Self, StorePathError>;
}

struct DisplayPath<'d, D>(char, &'d D);
impl<D> std::fmt::Display for DisplayPath<'_, D>
where
    D: DisplaySep,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.1.fmt(self.0, f)
    }
}
impl<D> StoreDirDisplay for DisplayPath<'_, D>
where
    D: DisplaySep,
{
    fn fmt(&self, store_dir: &StoreDir, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{store_dir}/{self}")
    }
}
// Used indirectly through FromStoreDirStr trait implementation
#[allow(dead_code)]
struct ParsePath<const C: char, D>(pub D);
impl<const C: char, D> FromStoreDirStr for ParsePath<C, D>
where
    D: FromStrSep,
{
    type Error = ParseStorePathError;

    fn from_store_dir_str(store_dir: &StoreDir, s: &str) -> Result<Self, Self::Error> {
        store_dir
            .strip_prefix(s)
            .and_then(|base| D::from_str_sep(C, base))
            .map(ParsePath)
            .map_err(|e| ParseStorePathError::new(s, e))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SingleDerivedPath {
    Built {
        #[serde(rename = "drvPath", with = "serde_arc")]
        drv_path: Arc<SingleDerivedPath>,
        output: OutputName,
    },
    Opaque(StorePath),
}

mod serde_arc {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S, T>(value: &Arc<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        (**value).serialize(serializer)
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Arc<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        T::deserialize(deserializer).map(Arc::new)
    }
}

#[cfg(any(test, feature = "test"))]
impl Arbitrary for SingleDerivedPath {
    type Parameters = ();
    type Strategy = BoxedStrategy<SingleDerivedPath>;

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::*;
        let opaque = any::<StorePath>().prop_map(SingleDerivedPath::Opaque);
        let leaf = prop_oneof![
            4 => opaque.clone(),
            1 => opaque.prop_recursive(6, 1, 1, |inner| {
                (any::<OutputName>(), inner).prop_map(|(output, drv_path)| {
                    SingleDerivedPath::Built {
                        drv_path: Arc::new(drv_path),
                        output,
                    }
                })
            })
        ];
        leaf.boxed()
    }
}

impl SingleDerivedPath {
    pub fn to_legacy_format(&self) -> impl StoreDirDisplay + std::fmt::Display + '_ {
        DisplayPath('!', self)
    }

    /// Returns the root (innermost) store path by recursively traversing Built variants.
    ///
    /// For an Opaque path, returns the store path directly.
    /// For a Built path, recursively follows the drv_path until reaching an Opaque path.
    pub fn root_path(&self) -> &StorePath {
        match self {
            SingleDerivedPath::Opaque(store_path) => store_path,
            SingleDerivedPath::Built { drv_path, .. } => drv_path.root_path(),
        }
    }
}

impl DisplaySep for SingleDerivedPath {
    fn fmt(&self, sep: char, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SingleDerivedPath::Opaque(store_path) => write!(f, "{}", store_path),
            SingleDerivedPath::Built { drv_path, output } => {
                write!(f, "{}{}{}", DisplayPath(sep, drv_path.deref()), sep, output)
            }
        }
    }
}

impl StoreDirDisplay for SingleDerivedPath {
    fn fmt(&self, store_dir: &StoreDir, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{store_dir}/{self}")
    }
}

impl std::fmt::Display for SingleDerivedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", DisplayPath('^', self))
    }
}

impl FromStrSep for SingleDerivedPath {
    fn from_str_sep(sep: char, s: &str) -> Result<Self, StorePathError> {
        let mut it = s.rsplitn(2, sep);
        let last = it.next().unwrap();
        if let Some(prefix) = it.next() {
            let drv_path = SingleDerivedPath::from_str_sep(sep, prefix)?;
            let output = last.parse()?;
            Ok(SingleDerivedPath::Built {
                drv_path: Arc::new(drv_path),
                output,
            })
        } else {
            Ok(SingleDerivedPath::Opaque(
                last.parse().map_err(|e: ParseStorePathError| e.error)?,
            ))
        }
    }
}

impl std::str::FromStr for SingleDerivedPath {
    type Err = ParseStorePathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str_sep('^', s).map_err(|e| ParseStorePathError::new(s, e))
    }
}

impl FromStoreDirStr for SingleDerivedPath {
    type Error = ParseStorePathError;

    fn from_store_dir_str(store_dir: &StoreDir, s: &str) -> Result<Self, Self::Error> {
        store_dir
            .strip_prefix(s)
            .and_then(|base| Self::from_str_sep('^', base))
            .map_err(|e| ParseStorePathError::new(s, e))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DerivedPath {
    Built {
        #[serde(rename = "drvPath", with = "serde_arc")]
        drv_path: Arc<SingleDerivedPath>,
        outputs: OutputSpec,
    },
    Opaque(StorePath),
}

#[cfg(any(test, feature = "test"))]
impl Arbitrary for DerivedPath {
    type Parameters = ();
    type Strategy = BoxedStrategy<DerivedPath>;

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::*;
        prop_oneof![
            any::<StorePath>().prop_map(DerivedPath::Opaque),
            (any::<SingleDerivedPath>(), any::<OutputSpec>()).prop_map(|(drv_path, outputs)| {
                DerivedPath::Built {
                    drv_path: Arc::new(drv_path),
                    outputs,
                }
            })
        ]
        .boxed()
    }
}

impl DerivedPath {
    pub fn to_legacy_format(&self) -> impl StoreDirDisplay + std::fmt::Display + '_ {
        DisplayPath('!', self)
    }
}

impl DisplaySep for DerivedPath {
    fn fmt(&self, sep: char, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DerivedPath::Opaque(store_path) => write!(f, "{}", store_path),
            DerivedPath::Built { drv_path, outputs } => {
                write!(f, "{}{}{}", DisplayPath(sep, drv_path), sep, outputs)
            }
        }
    }
}

impl StoreDirDisplay for DerivedPath {
    fn fmt(&self, store_dir: &StoreDir, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{store_dir}/{self}")
    }
}

impl std::fmt::Display for DerivedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", DisplayPath('^', self))
    }
}

impl FromStrSep for DerivedPath {
    fn from_str_sep(sep: char, s: &str) -> Result<Self, StorePathError> {
        let mut it = s.rsplitn(2, sep);
        let last = it.next().unwrap();
        if let Some(prefix) = it.next() {
            let drv_path = SingleDerivedPath::from_str_sep(sep, prefix)?;
            let outputs = last.parse()?;
            Ok(DerivedPath::Built {
                drv_path: Arc::new(drv_path),
                outputs,
            })
        } else {
            Ok(DerivedPath::Opaque(
                last.parse().map_err(|e: ParseStorePathError| e.error)?,
            ))
        }
    }
}

impl std::str::FromStr for DerivedPath {
    type Err = ParseStorePathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str_sep('^', s).map_err(|e| ParseStorePathError::new(s, e))
    }
}

impl FromStoreDirStr for DerivedPath {
    type Error = ParseStorePathError;

    fn from_store_dir_str(store_dir: &StoreDir, s: &str) -> Result<Self, Self::Error> {
        store_dir
            .strip_prefix(s)
            .and_then(|base| Self::from_str_sep('^', base))
            .map_err(|e| ParseStorePathError::new(s, e))
    }
}

pub struct LegacyDerivedPath(pub DerivedPath);
impl FromStoreDirStr for LegacyDerivedPath {
    type Error = ParseStorePathError;

    fn from_store_dir_str(store_dir: &StoreDir, s: &str) -> Result<Self, Self::Error> {
        store_dir
            .strip_prefix(s)
            .and_then(|base| DerivedPath::from_str_sep('!', base))
            .map(LegacyDerivedPath)
            .map_err(|e| ParseStorePathError::new(s, e))
    }
}

#[cfg(test)]
mod unittests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    use crate::store_path::{StoreDir, StorePathError};

    fn drv() -> Arc<SingleDerivedPath> {
        Arc::new(SingleDerivedPath::Opaque(
            "00000000000000000000000000000000-test.drv".parse().unwrap(),
        ))
    }
    fn nest(inner: Arc<SingleDerivedPath>, out: &str) -> Arc<SingleDerivedPath> {
        Arc::new(SingleDerivedPath::Built {
            drv_path: inner,
            output: out.parse().unwrap(),
        })
    }
    fn built(drv_path: Arc<SingleDerivedPath>, outputs: &str) -> DerivedPath {
        DerivedPath::Built {
            drv_path,
            outputs: outputs.parse().unwrap(),
        }
    }

    /// Asserts both `display(value) == carrot` and `parse(carrot) == value` for the
    /// `^`-separated format, then derives the legacy `!`-separated string and checks
    /// the same roundtrip via `LegacyDerivedPath` / `to_legacy_format()`.
    #[rstest]
    #[case::opaque("/nix/store/00000000000000000000000000000000-test.drv",
        DerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()))]
    #[case::out(
        "/nix/store/00000000000000000000000000000000-test.drv^out",
        built(drv(), "out")
    )]
    #[case::all(
        "/nix/store/00000000000000000000000000000000-test.drv^*",
        built(drv(), "*")
    )]
    #[case::multi(
        "/nix/store/00000000000000000000000000000000-test.drv^bin,lib",
        built(drv(), "bin,lib")
    )]
    #[case::nested(
        "/nix/store/00000000000000000000000000000000-test.drv^out^bin,lib",
        built(nest(drv(), "out"), "bin,lib")
    )]
    #[case::deep(
        "/nix/store/00000000000000000000000000000000-test.drv^out^bin^lib",
        built(nest(nest(drv(), "out"), "bin"), "lib")
    )]
    fn derived_path_roundtrip(#[case] carrot: &str, #[case] value: DerivedPath) {
        let store_dir = StoreDir::default();
        // ^ format
        assert_eq!(store_dir.display(&value).to_string(), carrot);
        assert_eq!(store_dir.parse::<DerivedPath>(carrot).unwrap(), value);
        // ! (legacy) format — only the separator differs
        let legacy = carrot.replace('^', "!");
        assert_eq!(
            store_dir.display(&value.to_legacy_format()).to_string(),
            legacy
        );
        assert_eq!(
            store_dir.parse::<LegacyDerivedPath>(&legacy).unwrap().0,
            value
        );
    }

    #[rstest]
    #[case::opaque("/nix/store/00000000000000000000000000000000-test.drv",
        SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()))]
    #[case::built("/nix/store/00000000000000000000000000000000-test.drv^bin",
        Arc::into_inner(nest(drv(), "bin")).unwrap())]
    #[case::nested("/nix/store/00000000000000000000000000000000-test.drv^out^bin",
        Arc::into_inner(nest(nest(drv(), "out"), "bin")).unwrap())]
    #[case::deep("/nix/store/00000000000000000000000000000000-test.drv^out^bin^lib",
        Arc::into_inner(nest(nest(nest(drv(), "out"), "bin"), "lib")).unwrap())]
    fn single_derived_path_roundtrip(#[case] carrot: &str, #[case] value: SingleDerivedPath) {
        let store_dir = StoreDir::default();
        assert_eq!(store_dir.display(&value).to_string(), carrot);
        assert_eq!(store_dir.parse::<SingleDerivedPath>(carrot).unwrap(), value);
        let legacy = carrot.replace('^', "!");
        assert_eq!(
            store_dir.display(&value.to_legacy_format()).to_string(),
            legacy
        );
    }

    /// The opposite separator (or a mix) must be rejected. Each case is given in
    /// the `^` form along with the expected error position; the `!` form is
    /// derived by substitution and the assertion swaps the rejected byte.
    #[rstest]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out", 41)]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out!bin", 41)]
    #[case(
        "/nix/store/00000000000000000000000000000000-test.drv!out!bin^out!lib",
        3
    )]
    fn parse_wrong_separator(#[case] input: &str, #[case] err_pos: u8) {
        let store_dir = StoreDir::default();
        let swapped: String = input
            .chars()
            .map(|c| match c {
                '^' => '!',
                '!' => '^',
                c => c,
            })
            .collect();
        let err = |path: &str, sym: u8| ParseStorePathError {
            path: path.into(),
            error: StorePathError::Symbol(err_pos, sym),
        };
        // ^ parsers reject `swapped` (which has ! in the offending spot)
        assert_eq!(
            store_dir.parse::<DerivedPath>(&swapped),
            Err(err(&swapped, b'!'))
        );
        assert_eq!(
            store_dir.parse::<SingleDerivedPath>(&swapped),
            Err(err(&swapped, b'!'))
        );
        // ! parser rejects `input` (which has ^ in the offending spot)
        assert_eq!(
            store_dir.parse::<LegacyDerivedPath>(input).map(|p| p.0),
            Err(err(input, b'^'))
        );
    }

    proptest! {
        #[test]
        fn proptest_derived_path_display_parse(dp in any::<DerivedPath>()) {
            let store_dir = StoreDir::default();
            let s = store_dir.display(&dp).to_string();
            prop_assert_eq!(store_dir.parse::<DerivedPath>(&s).unwrap(), dp.clone());
            let legacy = store_dir.display(&dp.to_legacy_format()).to_string();
            prop_assert_eq!(store_dir.parse::<LegacyDerivedPath>(&legacy).unwrap().0, dp);
        }

        #[test]
        fn proptest_single_derived_path_display_parse(dp in any::<SingleDerivedPath>()) {
            let store_dir = StoreDir::default();
            let s = store_dir.display(&dp).to_string();
            prop_assert_eq!(store_dir.parse::<SingleDerivedPath>(&s).unwrap(), dp);
        }
    }

    #[test]
    fn root_path_opaque() {
        let store_path: StorePath = "00000000000000000000000000000000-test".parse().unwrap();
        let path = SingleDerivedPath::Opaque(store_path.clone());
        assert_eq!(path.root_path(), &store_path);
    }

    #[test]
    fn root_path_built() {
        let store_path: StorePath = "00000000000000000000000000000000-test".parse().unwrap();
        let path = SingleDerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(store_path.clone())),
            output: "out".parse().unwrap(),
        };
        assert_eq!(path.root_path(), &store_path);
    }

    #[test]
    fn root_path_nested() {
        let store_path: StorePath = "00000000000000000000000000000000-test".parse().unwrap();
        let inner_path = SingleDerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(store_path.clone())),
            output: "inner".parse().unwrap(),
        };
        let outer_path = SingleDerivedPath::Built {
            drv_path: Arc::new(inner_path),
            output: "outer".parse().unwrap(),
        };
        // The root_path should resolve to the innermost store path
        assert_eq!(outer_path.root_path(), &store_path);
    }
}
