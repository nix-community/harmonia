use std::collections::BTreeSet;
use std::str::FromStr;

use derive_more::Display;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::derived_path::OutputName;
use crate::signature::Signature;
use crate::store_path::{ParseStorePathError, StorePath, StorePathNameError};

/// Identifies a specific output of a derivation.
///
/// String form (`to_string`/`FromStr`): `<drv-basename>^<output-name>`, where
/// `<drv-basename>` is the store-path base name (without store dir).
///
/// JSON form: `{"drvPath": "<basename>", "outputName": "<name>"}`.
#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Clone, Display, Serialize, Deserialize)]
#[display("{drv_path}^{output_name}")]
#[serde(rename_all = "camelCase")]
pub struct DrvOutput {
    pub drv_path: StorePath,
    pub output_name: OutputName,
}

#[derive(Debug, PartialEq, Clone, Error)]
pub enum ParseDrvOutputError {
    #[error("derivation output {0}")]
    DrvPath(
        #[from]
        #[source]
        ParseStorePathError,
    ),
    #[error("derivation output has {0}")]
    OutputName(
        #[from]
        #[source]
        StorePathNameError,
    ),
    #[error("missing '^' in derivation output id '{0}'")]
    InvalidDerivationOutputId(String),
}

impl FromStr for DrvOutput {
    type Err = ParseDrvOutputError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((drv_path_s, output_name_s)) = s.rsplit_once('^') {
            let drv_path = drv_path_s.parse()?;
            let output_name = output_name_s.parse()?;
            Ok(DrvOutput {
                drv_path,
                output_name,
            })
        } else {
            Err(ParseDrvOutputError::InvalidDerivationOutputId(s.into()))
        }
    }
}

/// A realisation without its `DrvOutput` key.
///
/// This is what binary caches store at
/// `build-trace-v2/<drv-basename>/<output-name>.doi`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnkeyedRealisation {
    pub out_path: StorePath,
    #[serde(default)]
    pub signatures: BTreeSet<Signature>,
}

/// A `DrvOutput` together with its resolved store path and signatures.
///
/// JSON form: `{"key": <DrvOutput>, "value": <UnkeyedRealisation>}`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Deserialize, Serialize)]
pub struct Realisation {
    #[serde(rename = "key")]
    pub id: DrvOutput,
    #[serde(rename = "value")]
    pub value: UnkeyedRealisation,
}

impl Realisation {
    /// Compute the fingerprint used for signing realisations.
    ///
    /// Matches `UnkeyedRealisation::fingerprint` in upstream Nix: the full
    /// `{key,value}` JSON with `value.signatures` removed, serialized with
    /// sorted keys.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut json = serde_json::to_value(self).expect("Realisation serialization cannot fail");
        json.as_object_mut()
            .expect("Realisation must serialize as object")
            .get_mut("value")
            .expect("Realisation must have 'value'")
            .as_object_mut()
            .expect("'value' must be an object")
            .remove("signatures");
        json.to_string()
    }

    /// Sign this realisation with the given secret keys, adding the resulting
    /// signatures to `value.signatures`.
    pub fn sign(&mut self, keys: &[crate::signature::SecretKey]) {
        let fp = self.fingerprint();
        for key in keys {
            self.value.signatures.insert(key.sign(fp.as_bytes()));
        }
    }
}

#[cfg(any(test, feature = "test"))]
pub mod arbitrary {
    use crate::signature::proptests::arb_signatures;

    use super::*;
    use ::proptest::prelude::*;

    impl Arbitrary for DrvOutput {
        type Parameters = ();
        type Strategy = BoxedStrategy<DrvOutput>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_drv_output().boxed()
        }
    }

    prop_compose! {
        fn arb_drv_output()
        (
            drv_path in any::<StorePath>(),
            output_name in any::<OutputName>(),
        ) -> DrvOutput
        {
            DrvOutput { drv_path, output_name }
        }
    }

    impl Arbitrary for UnkeyedRealisation {
        type Parameters = ();
        type Strategy = BoxedStrategy<UnkeyedRealisation>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_unkeyed_realisation().boxed()
        }
    }

    prop_compose! {
        fn arb_unkeyed_realisation()
        (
            out_path in any::<StorePath>(),
            signatures in arb_signatures(),
        ) -> UnkeyedRealisation
        {
            UnkeyedRealisation { out_path, signatures }
        }
    }

    impl Arbitrary for Realisation {
        type Parameters = ();
        type Strategy = BoxedStrategy<Realisation>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_realisation().boxed()
        }
    }

    prop_compose! {
        fn arb_realisation()
        (
            id in any::<DrvOutput>(),
            value in any::<UnkeyedRealisation>(),
        ) -> Realisation
        {
            Realisation { id, value }
        }
    }
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use crate::derived_path::OutputName;

    use super::{DrvOutput, Realisation, UnkeyedRealisation};

    #[rstest]
    #[case("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv^out", DrvOutput {
        drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
        output_name: OutputName::default(),
    })]
    #[case("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv^out_put", DrvOutput {
        drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
        output_name: "out_put".parse().unwrap(),
    })]
    fn parse_drv_output(#[case] value: &str, #[case] expected: DrvOutput) {
        let actual: DrvOutput = value.parse().unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.to_string(), value);
    }

    proptest::proptest! {
        #[test]
        fn proptest_drv_output_display_parse(d in proptest::prelude::any::<DrvOutput>()) {
            let s = d.to_string();
            proptest::prop_assert_eq!(s.parse::<DrvOutput>().unwrap(), d);
        }
    }

    #[rstest]
    #[should_panic = "missing '^' in derivation output id"]
    #[case("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv")]
    #[should_panic = "derivation output has invalid name symbol '{' at position 3"]
    #[case("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv^out{put")]
    fn parse_drv_output_failure(#[case] value: &str) {
        let actual = value.parse::<DrvOutput>().unwrap_err();
        panic!("{actual}");
    }

    #[test]
    fn sign_produces_verifiable_signature() {
        let mut r = Realisation {
            id: DrvOutput {
                drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
                output_name: "foo".parse().unwrap(),
            },
            value: UnkeyedRealisation {
                out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
                signatures: Default::default(),
            },
        };
        let sk = crate::signature::SecretKey::generate("test-key".to_string()).unwrap();
        let pk = sk.to_public_key();
        r.sign(&[sk]);
        let sig = r.value.signatures.iter().next().unwrap();
        assert_eq!(sig.name(), "test-key");
        assert!(pk.verify(r.fingerprint(), sig));
    }
}
