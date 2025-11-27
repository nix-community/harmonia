use std::ops::Deref as _;

#[cfg(any(test, feature = "test"))]
use proptest::prelude::{Arbitrary, BoxedStrategy};
use serde::{Deserialize, Serialize};

use crate::store_path::{FromStoreDirStr, ParseStorePathError, StoreDirDisplay, StorePath};

use super::{OutputName, OutputSpec};

trait StoreDirDisplaySep {
    fn fmt(
        &self,
        store_dir: &crate::store_path::StoreDir,
        sep: char,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result;
}

impl<D> StoreDirDisplaySep for &D
where
    D: StoreDirDisplaySep,
{
    fn fmt(
        &self,
        store_dir: &crate::store_path::StoreDir,
        sep: char,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        (**self).fmt(store_dir, sep, f)
    }
}

trait FromStoreDirStrSep: Sized {
    type Error: std::error::Error;

    fn from_store_dir_str_sep(
        store_dir: &crate::store_path::StoreDir,
        sep: char,
        s: &str,
    ) -> Result<Self, Self::Error>;
}

struct DisplayPath<'d, D>(char, &'d D);
impl<D> StoreDirDisplay for DisplayPath<'_, D>
where
    D: StoreDirDisplaySep,
{
    fn fmt(
        &self,
        store_dir: &crate::store_path::StoreDir,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        self.1.fmt(store_dir, self.0, f)
    }
}
struct ParsePath<const C: char, D>(pub D);
impl<const C: char, D> FromStoreDirStr for ParsePath<C, D>
where
    D: FromStoreDirStrSep,
    <D as FromStoreDirStrSep>::Error: std::error::Error,
{
    type Error = D::Error;

    fn from_store_dir_str(
        store_dir: &crate::store_path::StoreDir,
        s: &str,
    ) -> Result<Self, Self::Error> {
        Ok(ParsePath(
            <D as FromStoreDirStrSep>::from_store_dir_str_sep(store_dir, C, s)?,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SingleDerivedPath {
    Built {
        #[serde(rename = "drvPath")]
        drv_path: Box<SingleDerivedPath>,
        output: OutputName,
    },
    Opaque(StorePath),
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
                        drv_path: Box::new(drv_path),
                        output,
                    }
                })
            })
        ];
        leaf.boxed()
    }
}

impl SingleDerivedPath {
    pub fn to_legacy_format(&self) -> impl StoreDirDisplay + '_ {
        DisplayPath('!', self)
    }
}

impl StoreDirDisplaySep for SingleDerivedPath {
    fn fmt(
        &self,
        store_dir: &crate::store_path::StoreDir,
        sep: char,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            SingleDerivedPath::Opaque(store_path) => write!(f, "{}", store_dir.display(store_path)),
            SingleDerivedPath::Built { drv_path, output } => {
                let path = DisplayPath(sep, drv_path.deref());
                write!(f, "{}{}{}", store_dir.display(&path), sep, output)
            }
        }
    }
}

impl StoreDirDisplay for SingleDerivedPath {
    fn fmt(
        &self,
        store_dir: &crate::store_path::StoreDir,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let path = DisplayPath('^', self);
        write!(f, "{}", store_dir.display(&path))
    }
}

impl FromStoreDirStrSep for SingleDerivedPath {
    type Error = ParseStorePathError;

    fn from_store_dir_str_sep(
        store_dir: &crate::store_path::StoreDir,
        sep: char,
        s: &str,
    ) -> Result<Self, Self::Error> {
        let mut it = s.rsplitn(2, sep);
        let path = it.next().unwrap();
        if let Some(prefix) = it.next() {
            let drv_path = SingleDerivedPath::from_store_dir_str_sep(store_dir, sep, prefix)?;
            let output = path
                .parse()
                .map_err(|error: crate::store_path::StorePathNameError| {
                    ParseStorePathError::new(s, error)
                })?;
            Ok(SingleDerivedPath::Built {
                drv_path: Box::new(drv_path),
                output,
            })
        } else {
            Ok(SingleDerivedPath::Opaque(store_dir.parse(path)?))
        }
    }
}

impl FromStoreDirStr for SingleDerivedPath {
    type Error = ParseStorePathError;

    fn from_store_dir_str(
        store_dir: &crate::store_path::StoreDir,
        s: &str,
    ) -> Result<Self, Self::Error> {
        SingleDerivedPath::from_store_dir_str_sep(store_dir, '^', s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DerivedPath {
    Built {
        #[serde(rename = "drvPath")]
        drv_path: SingleDerivedPath,
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
            (any::<SingleDerivedPath>(), any::<OutputSpec>())
                .prop_map(|(drv_path, outputs)| { DerivedPath::Built { drv_path, outputs } })
        ]
        .boxed()
    }
}

impl DerivedPath {
    pub fn to_legacy_format(&self) -> impl StoreDirDisplay + '_ {
        DisplayPath('!', self)
    }
}

impl StoreDirDisplaySep for DerivedPath {
    fn fmt(
        &self,
        store_dir: &crate::store_path::StoreDir,
        sep: char,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            DerivedPath::Opaque(store_path) => write!(f, "{}", store_dir.display(store_path)),
            DerivedPath::Built { drv_path, outputs } => {
                let path = DisplayPath(sep, drv_path);
                write!(f, "{}{}{}", store_dir.display(&path), sep, outputs)
            }
        }
    }
}

impl StoreDirDisplay for DerivedPath {
    fn fmt(
        &self,
        store_dir: &crate::store_path::StoreDir,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let path = DisplayPath('^', self);
        write!(f, "{}", store_dir.display(&path))
    }
}

impl FromStoreDirStrSep for DerivedPath {
    type Error = ParseStorePathError;

    fn from_store_dir_str_sep(
        store_dir: &crate::store_path::StoreDir,
        sep: char,
        s: &str,
    ) -> Result<Self, Self::Error> {
        let mut it = s.rsplitn(2, sep);
        let path = it.next().unwrap();
        if let Some(prefix) = it.next() {
            let drv_path = SingleDerivedPath::from_store_dir_str_sep(store_dir, sep, prefix)?;
            let outputs = path
                .parse()
                .map_err(|error| ParseStorePathError::new(s, error))?;
            Ok(DerivedPath::Built { drv_path, outputs })
        } else {
            Ok(DerivedPath::Opaque(store_dir.parse(path)?))
        }
    }
}

impl FromStoreDirStr for DerivedPath {
    type Error = ParseStorePathError;

    fn from_store_dir_str(
        store_dir: &crate::store_path::StoreDir,
        s: &str,
    ) -> Result<Self, Self::Error> {
        DerivedPath::from_store_dir_str_sep(store_dir, '^', s)
    }
}

pub struct LegacyDerivedPath(pub DerivedPath);
impl FromStoreDirStr for LegacyDerivedPath {
    type Error = ParseStorePathError;

    fn from_store_dir_str(
        store_dir: &crate::store_path::StoreDir,
        s: &str,
    ) -> Result<Self, Self::Error> {
        Ok(LegacyDerivedPath(DerivedPath::from_store_dir_str_sep(
            store_dir, '!', s,
        )?))
    }
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use super::*;
    use crate::store_path::{StoreDir, StorePathError};

    #[rstest]
    #[case("/nix/store/00000000000000000000000000000000-test.drv", Ok(DerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "out".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^*", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "*".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^bin,lib", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "bin,lib".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out^bin,lib", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
            output: "out".parse().unwrap(),
        },
        outputs: "bin,lib".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out^bin^lib", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Built {
                drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
                output: "out".parse().unwrap(),
            }),
            output: "bin".parse().unwrap(),
        },
        outputs: "lib".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!out", Err(ParseStorePathError {
        path: "/nix/store/00000000000000000000000000000000-test.drv!out".into(),
        error: StorePathError::Symbol(41, b'!'),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!out^bin", Err(ParseStorePathError {
        path: "/nix/store/00000000000000000000000000000000-test.drv!out".into(),
        error: StorePathError::Symbol(41, b'!'),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out^bin!out^lib", Err(ParseStorePathError {
        path: "/nix/store/00000000000000000000000000000000-test.drv^out^bin!out".into(),
        error: StorePathError::Symbol(3, b'!'),
    }))]
    fn parse_path(#[case] input: &str, #[case] expected: Result<DerivedPath, ParseStorePathError>) {
        let store_dir = StoreDir::default();
        let actual: Result<DerivedPath, _> = store_dir.parse(input);
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("/nix/store/00000000000000000000000000000000-test.drv", Ok(DerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!out", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "out".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!*", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "*".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!bin,lib", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "bin,lib".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!out!bin,lib", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
            output: "out".parse().unwrap(),
        },
        outputs: "bin,lib".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!out!bin!lib", Ok(DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Built {
                drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
                output: "out".parse().unwrap(),
            }),
            output: "bin".parse().unwrap(),
        },
        outputs: "lib".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out", Err(ParseStorePathError {
        path: "/nix/store/00000000000000000000000000000000-test.drv^out".into(),
        error: StorePathError::Symbol(41, b'^'),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out!bin", Err(ParseStorePathError {
        path: "/nix/store/00000000000000000000000000000000-test.drv^out".into(),
        error: StorePathError::Symbol(41, b'^'),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!out!bin^out!lib", Err(ParseStorePathError {
        path: "/nix/store/00000000000000000000000000000000-test.drv!out!bin^out".into(),
        error: StorePathError::Symbol(3, b'^'),
    }))]
    fn parse_legacy_path(
        #[case] input: &str,
        #[case] expected: Result<DerivedPath, ParseStorePathError>,
    ) {
        let store_dir = StoreDir::default();
        let actual: Result<LegacyDerivedPath, _> = store_dir.parse(input);
        assert_eq!(actual.map(|p| p.0), expected);
    }

    #[rstest]
    #[case("/nix/store/00000000000000000000000000000000-test.drv", Ok(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^bin", Ok(SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
        output: "bin".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out^bin", Ok(SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
            output: "out".parse().unwrap(),
        }),
        output: "bin".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out^bin^lib", Ok(SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Built {
                drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
                output: "out".parse().unwrap(),
            }),
            output: "bin".parse().unwrap(),
        }),
        output: "lib".parse().unwrap(),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!out", Err(ParseStorePathError {
        path: "/nix/store/00000000000000000000000000000000-test.drv!out".into(),
        error: StorePathError::Symbol(41, b'!'),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv!out^bin", Err(ParseStorePathError {
        path: "/nix/store/00000000000000000000000000000000-test.drv!out".into(),
        error: StorePathError::Symbol(41, b'!'),
    }))]
    #[case("/nix/store/00000000000000000000000000000000-test.drv^out^bin!out^lib", Err(ParseStorePathError {
        path: "/nix/store/00000000000000000000000000000000-test.drv^out^bin!out".into(),
        error: StorePathError::Symbol(3, b'!'),
    }))]
    fn parse_single_path(
        #[case] input: &str,
        #[case] expected: Result<SingleDerivedPath, ParseStorePathError>,
    ) {
        let store_dir = StoreDir::default();
        let actual: Result<SingleDerivedPath, _> = store_dir.parse(input);
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case(DerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()), "/nix/store/00000000000000000000000000000000-test.drv")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "out".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv^out")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "*".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv^*")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "bin,lib".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv^bin,lib")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
            output: "out".parse().unwrap(),
        },
        outputs: "bin,lib".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv^out^bin,lib")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Built {
                drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
                output: "out".parse().unwrap(),
            }),
            output: "bin".parse().unwrap(),
        },
        outputs: "lib".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv^out^bin^lib")]
    fn display_path(#[case] value: DerivedPath, #[case] expected: &str) {
        let store_dir = StoreDir::default();
        assert_eq!(store_dir.display(&value).to_string(), expected);
    }

    #[rstest]
    #[case(DerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()), "/nix/store/00000000000000000000000000000000-test.drv")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "out".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv!out")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "*".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv!*")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()),
        outputs: "bin,lib".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv!bin,lib")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
            output: "out".parse().unwrap(),
        },
        outputs: "bin,lib".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv!out!bin,lib")]
    #[case(DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Built {
                drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
                output: "out".parse().unwrap(),
            }),
            output: "bin".parse().unwrap(),
        },
        outputs: "lib".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv!out!bin!lib")]
    fn display_legacy_path(#[case] value: DerivedPath, #[case] expected: &str) {
        let store_dir = StoreDir::default();
        assert_eq!(
            store_dir.display(&value.to_legacy_format()).to_string(),
            expected
        );
    }

    #[rstest]
    #[case(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()), "/nix/store/00000000000000000000000000000000-test.drv")]
    #[case(SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
        output: "bin".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv^bin")]
    #[case(SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
            output: "out".parse().unwrap(),
        }),
        output: "bin".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv^out^bin")]
    #[case(SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Built {
                drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
                output: "out".parse().unwrap(),
            }),
            output: "bin".parse().unwrap(),
        }),
        output: "lib".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv^out^bin^lib")]
    fn display_single_path(#[case] value: SingleDerivedPath, #[case] expected: &str) {
        let store_dir = StoreDir::default();
        assert_eq!(store_dir.display(&value).to_string(), expected);
    }

    #[rstest]
    #[case(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap()), "/nix/store/00000000000000000000000000000000-test.drv")]
    #[case(SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
        output: "bin".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv!bin")]
    #[case(SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
            output: "out".parse().unwrap(),
        }),
        output: "bin".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv!out!bin")]
    #[case(SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Built {
                drv_path: Box::new(SingleDerivedPath::Opaque("00000000000000000000000000000000-test.drv".parse().unwrap())),
                output: "out".parse().unwrap(),
            }),
            output: "bin".parse().unwrap(),
        }),
        output: "lib".parse().unwrap(),
    }, "/nix/store/00000000000000000000000000000000-test.drv!out!bin!lib")]
    fn display_single_legacy_path(#[case] value: SingleDerivedPath, #[case] expected: &str) {
        let store_dir = StoreDir::default();
        assert_eq!(
            store_dir.display(&value.to_legacy_format()).to_string(),
            expected
        );
    }
}
