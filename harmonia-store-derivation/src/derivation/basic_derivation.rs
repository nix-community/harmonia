use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

#[cfg(test)]
use proptest::prelude::{Arbitrary, BoxedStrategy};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::derived_path::SingleDerivedPath;
use harmonia_store_path::{StorePathName, StorePathSet};

use super::{DerivationInputs, DerivationOutput, DerivationOutputs};

/// Error extracting or parsing `__json` structured attributes.
#[derive(Debug, thiserror::Error)]
pub enum StructuredAttrsError {
    #[error("invalid UTF-8 in `__json` structured attributes: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("invalid `__json` structured attributes: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

/// Structured attributes for a derivation.
///
/// When present, derivation attributes are passed to the builder as a JSON object
/// via the `__json` environment variable, rather than individual environment variables.
/// This allows passing complex data structures (arrays, nested objects, etc.) to builders.
///
/// When parsed from ATerm or the daemon wire format via [`Self::parse`], the
/// original `__json` string is kept verbatim so non-canonical encodings (such as
/// float formatting like `1e+15` vs `1000000000000000.0` or whitespace) round-trip
/// byte-identically and produce matching derivation hashes. Mutating through
/// [`Self::attrs_mut`] clears the verbatim copy so [`Self::unparse`] re-serializes
/// the updated map.
#[derive(Debug, Clone)]
pub struct StructuredAttrs {
    /// The structured attributes as a JSON object.
    ///
    /// Note: The `env` map must not contain the key `__json` when structured attrs are used,
    /// as that key is reserved for encoding the structured attributes themselves.
    attrs: serde_json::Map<String, serde_json::Value>,
    /// Raw `__json` encoding preserved from parsing, if any.
    verbatim: Option<String>,
}

impl StructuredAttrs {
    /// Environment variable name used to carry structured attributes in ATerm
    /// and daemon wire encodings.
    pub const ENV_VAR: &[u8] = b"__json";

    /// Create structured attributes from a parsed JSON object map.
    pub fn new(attrs: serde_json::Map<String, serde_json::Value>) -> Self {
        Self {
            attrs,
            verbatim: None,
        }
    }

    /// Parse structured attributes from an encoded `__json` string, retaining
    /// the original encoding verbatim for lossless ATerm/wire round-tripping.
    pub fn parse(encoded: impl Into<String>) -> Result<Self, serde_json::Error> {
        let encoded = encoded.into();
        let attrs: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&encoded)?;
        Ok(Self {
            attrs,
            verbatim: Some(encoded),
        })
    }

    /// Remove and parse `__json` from an environment map, mirroring C++ Nix's
    /// `StructuredAttrs::tryExtract`.
    pub fn try_extract(
        env: &mut BTreeMap<bytes::Bytes, bytes::Bytes>,
    ) -> Result<Option<Self>, StructuredAttrsError> {
        let Some(json_bytes) = env.remove(Self::ENV_VAR) else {
            return Ok(None);
        };
        let json_str = String::from_utf8(json_bytes.into()).map_err(|e| e.utf8_error())?;
        Ok(Some(Self::parse(json_str)?))
    }

    /// Borrow the parsed structured attributes map.
    pub fn attrs(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.attrs
    }

    /// Mutably borrow the structured attributes map, invalidating any preserved
    /// verbatim `__json` encoding.
    pub fn attrs_mut(&mut self) -> &mut serde_json::Map<String, serde_json::Value> {
        self.verbatim = None;
        &mut self.attrs
    }

    /// Consume `self` and return the underlying JSON object map.
    pub fn into_attrs(self) -> serde_json::Map<String, serde_json::Value> {
        self.attrs
    }

    /// Return the preserved verbatim `__json` encoding, if this value was parsed
    /// and has not been mutated since.
    pub fn verbatim(&self) -> Option<&str> {
        self.verbatim.as_deref()
    }

    /// Encode the structured attributes as a `__json` string, borrowing the
    /// preserved verbatim encoding when available.
    pub fn unparse(&self) -> Cow<'_, str> {
        match &self.verbatim {
            Some(raw) => Cow::Borrowed(raw.as_str()),
            None => Cow::Owned(
                serde_json::to_string(&self.attrs)
                    .expect("serde_json::Map serialization is infallible"),
            ),
        }
    }

    /// Encode the structured attributes as owned `__json` bytes.
    pub fn unparse_bytes(&self) -> bytes::Bytes {
        match self.unparse() {
            Cow::Borrowed(s) => bytes::Bytes::copy_from_slice(s.as_bytes()),
            Cow::Owned(s) => bytes::Bytes::from(s),
        }
    }
}

impl From<serde_json::Map<String, serde_json::Value>> for StructuredAttrs {
    fn from(attrs: serde_json::Map<String, serde_json::Value>) -> Self {
        Self::new(attrs)
    }
}

impl PartialEq for StructuredAttrs {
    fn eq(&self, other: &Self) -> bool {
        // Compare encoded representations (matching C++ Nix's
        // `StructuredAttrs::operator==`), since the exact `__json` bytes are
        // part of a derivation's hash identity.
        match (&self.verbatim, &other.verbatim) {
            (Some(a), Some(b)) => a == b,
            (None, None) if self.attrs != other.attrs => false,
            // Exactly one side is verbatim, so compare encodings only. Parsed
            // maps can differ even when the encodings match, because parsing
            // need not reproduce the exact `f64` that was serialized.
            _ => self.unparse() == other.unparse(),
        }
    }
}

impl Eq for StructuredAttrs {}

impl Hash for StructuredAttrs {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.unparse().hash(state);
    }
}

impl Serialize for StructuredAttrs {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.attrs.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StructuredAttrs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_json::Map::deserialize(deserializer).map(Self::new)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DerivationT<Inputs, Output = DerivationOutput> {
    /// The name of the derivation
    pub name: StorePathName,
    pub outputs: DerivationOutputs<Output>,
    pub inputs: Inputs,
    pub platform: bytes::Bytes,
    pub builder: bytes::Bytes,
    pub args: Vec<bytes::Bytes>,
    /// Environment variables passed to the builder.
    ///
    /// Note: Must not contain the key `__json` as that key is reserved
    /// for encoding structured attributes.
    pub env: BTreeMap<bytes::Bytes, bytes::Bytes>,
    /// Optional structured attributes.
    ///
    /// When present, attributes are passed to the builder as JSON via the `__json`
    /// environment variable instead of individual environment variables.
    pub structured_attrs: Option<StructuredAttrs>,
}

fn default_version() -> u32 {
    4
}

pub type BasicDerivation = DerivationT<StorePathSet>;
pub type Derivation = DerivationT<BTreeSet<SingleDerivedPath>>;

#[cfg(any(test, feature = "test"))]
pub mod arbitrary {
    use super::*;
    use crate::{
        derivation::derivation_output::arbitrary::arb_derivation_outputs,
        test::arbitrary::arb_byte_string,
    };
    use ::proptest::prelude::*;
    use proptest::sample::SizeRange;

    impl Arbitrary for BasicDerivation {
        type Parameters = ();
        type Strategy = BoxedStrategy<BasicDerivation>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_basic_derivation().boxed()
        }
    }

    prop_compose! {
        pub fn arb_basic_derivation()
        (
            name in any::<StorePathName>(),
            outputs in arb_derivation_outputs(1..15),
            inputs in any::<StorePathSet>(),
            platform in arb_byte_string(),
            builder in arb_byte_string(),
            args in proptest::collection::vec(arb_byte_string(), SizeRange::default()),
            env in proptest::collection::btree_map(arb_byte_string(), arb_byte_string(), SizeRange::default()),
            structured_attrs in proptest::option::of(arb_structured_attrs())
        ) -> BasicDerivation
        {
            DerivationT {
                name,
                outputs,
                inputs,
                platform,
                builder,
                args,
                env,
                structured_attrs,
            }
        }
    }

    fn arb_structured_attrs() -> impl Strategy<Value = StructuredAttrs> {
        // Generate a simple JSON object with string keys and various JSON values
        proptest::collection::btree_map(
            any::<String>(),
            prop_oneof![
                any::<String>().prop_map(serde_json::Value::String),
                any::<i64>().prop_map(|i| serde_json::Value::Number(i.into())),
                any::<bool>().prop_map(serde_json::Value::Bool),
                Just(serde_json::Value::Null),
            ],
            0..10,
        )
        .prop_map(|map| {
            let mut attrs = serde_json::Map::new();
            for (k, v) in map {
                attrs.insert(k, v);
            }
            StructuredAttrs::new(attrs)
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DerivationHelperT<Inputs> {
    name: String,
    outputs: DerivationOutputs,
    inputs: Inputs,
    #[serde(rename = "system")]
    platform: String,
    builder: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    structured_attrs: Option<StructuredAttrs>,
    #[serde(default = "default_version")]
    version: u32,
}

/// Trait for converting between the in-memory input representation and
/// the JSON-serializable form.
trait SerdeInputs: Sized {
    type Helper: Serialize + for<'de> Deserialize<'de>;
    fn to_helper(&self) -> Self::Helper;
    fn from_helper(h: Self::Helper) -> Self;
}

impl SerdeInputs for StorePathSet {
    type Helper = StorePathSet;
    fn to_helper(&self) -> StorePathSet {
        self.clone()
    }
    fn from_helper(h: StorePathSet) -> Self {
        h
    }
}

impl SerdeInputs for BTreeSet<SingleDerivedPath> {
    type Helper = DerivationInputs;
    fn to_helper(&self) -> DerivationInputs {
        DerivationInputs::from(self)
    }
    fn from_helper(h: DerivationInputs) -> Self {
        BTreeSet::from(&h)
    }
}

fn derivation_to_helper<Inputs: SerdeInputs>(
    drv: &DerivationT<Inputs>,
) -> Result<DerivationHelperT<Inputs::Helper>, std::str::Utf8Error> {
    let platform_str = std::str::from_utf8(&drv.platform)?;
    let builder_str = std::str::from_utf8(&drv.builder)?;
    let args_strs: Result<Vec<String>, _> = drv
        .args
        .iter()
        .map(|b| std::str::from_utf8(b).map(|s| s.to_string()))
        .collect();
    let args_strs = args_strs?;
    let env_map: Result<BTreeMap<String, String>, _> = drv
        .env
        .iter()
        .map(|(k, v)| {
            Ok((
                std::str::from_utf8(k)?.to_string(),
                std::str::from_utf8(v)?.to_string(),
            ))
        })
        .collect();
    let env_map = env_map?;

    Ok(DerivationHelperT {
        name: drv.name.to_string(),
        version: default_version(),
        outputs: drv.outputs.clone(),
        inputs: drv.inputs.to_helper(),
        platform: platform_str.to_string(),
        builder: builder_str.to_string(),
        args: args_strs,
        env: env_map,
        structured_attrs: drv.structured_attrs.clone(),
    })
}

fn helper_to_derivation<'de, Inputs: SerdeInputs, D: Deserializer<'de>>(
    helper: DerivationHelperT<Inputs::Helper>,
) -> Result<DerivationT<Inputs>, D::Error> {
    if helper.version != 4 {
        return Err(serde::de::Error::custom(format!(
            "unsupported derivation version: {}, expected 4",
            helper.version
        )));
    }
    Ok(DerivationT {
        name: helper
            .name
            .parse()
            .map_err(|e| serde::de::Error::custom(format!("invalid derivation name: {}", e)))?,
        outputs: helper.outputs,
        inputs: Inputs::from_helper(helper.inputs),
        platform: bytes::Bytes::from(helper.platform),
        builder: bytes::Bytes::from(helper.builder),
        args: helper.args.into_iter().map(bytes::Bytes::from).collect(),
        env: helper
            .env
            .into_iter()
            .map(|(k, v)| (bytes::Bytes::from(k), bytes::Bytes::from(v)))
            .collect(),
        structured_attrs: helper.structured_attrs,
    })
}

impl Serialize for BasicDerivation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        derivation_to_helper(self)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BasicDerivation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = DerivationHelperT::<StorePathSet>::deserialize(deserializer)?;
        helper_to_derivation::<StorePathSet, D>(helper)
    }
}

impl Serialize for Derivation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        derivation_to_helper(self)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Derivation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = DerivationHelperT::<DerivationInputs>::deserialize(deserializer)?;
        helper_to_derivation::<BTreeSet<SingleDerivedPath>, D>(helper)
    }
}

impl<Inputs, Output> DerivationT<Inputs, Output> {
    /// Transform outputs, keeping everything else the same.
    pub fn map_outputs<T>(self, mut f: impl FnMut(Output) -> T) -> DerivationT<Inputs, T> {
        DerivationT {
            name: self.name,
            outputs: self.outputs.into_iter().map(|(k, v)| (k, f(v))).collect(),
            inputs: self.inputs,
            platform: self.platform,
            builder: self.builder,
            args: self.args,
            env: self.env,
            structured_attrs: self.structured_attrs,
        }
    }

    /// Convert the inputs to the split-by-kind format representation
    /// ([`DerivationInputs`]), moving the other fields. This is the
    /// boundary conversion for the on-disk/wire formats (e.g. the ATerm
    /// printer).
    pub fn into_full(self) -> DerivationT<DerivationInputs, Output>
    where
        for<'a> DerivationInputs: From<&'a Inputs>,
    {
        self.map_inputs(|i| DerivationInputs::from(&i))
    }

    /// Transform inputs, keeping everything else the same.
    pub fn map_inputs<T>(self, f: impl FnOnce(Inputs) -> T) -> DerivationT<T, Output> {
        DerivationT {
            name: self.name,
            outputs: self.outputs,
            inputs: f(self.inputs),
            platform: self.platform,
            builder: self.builder,
            args: self.args,
            env: self.env,
            structured_attrs: self.structured_attrs,
        }
    }
}

impl<Inputs> DerivationT<Inputs> {
    /// Replace all occurrences of each rewrite's key with its value in builder,
    /// args, env (keys and values), and structured_attrs.
    ///
    /// This is used during derivation resolution to substitute CA output
    /// placeholders with their actual store paths.
    pub fn apply_rewrites(&mut self, rewrites: &BTreeMap<bytes::Bytes, bytes::Bytes>) {
        if rewrites.is_empty() {
            return;
        }

        fn rewrite(s: &[u8], rewrites: &BTreeMap<bytes::Bytes, bytes::Bytes>) -> bytes::Bytes {
            let mut buf = s.to_vec();
            for (from, to) in rewrites {
                assert!(!from.is_empty(), "rewrite key must not be empty");
                // Repeatedly scan for the pattern and replace all occurrences.
                // TODO: after https://github.com/tokio-rs/bytes/issues/824 is resolved, use the replace method on BytesMut.
                let mut i = 0;
                while i + from.len() <= buf.len() {
                    if buf[i..i + from.len()] == **from {
                        buf.splice(i..i + from.len(), to.iter().copied());
                        i += to.len();
                    } else {
                        i += 1;
                    }
                }
            }
            bytes::Bytes::from(buf)
        }

        self.builder = rewrite(&self.builder, rewrites);

        for arg in &mut self.args {
            *arg = rewrite(arg, rewrites);
        }

        let old_env = std::mem::take(&mut self.env);
        for (k, v) in old_env {
            self.env
                .insert(rewrite(&k, rewrites), rewrite(&v, rewrites));
        }

        if let Some(ref mut sa) = self.structured_attrs {
            let raw = sa.unparse();
            let rewritten = rewrite(raw.as_bytes(), rewrites);
            if rewritten.as_ref() != raw.as_bytes()
                && let Ok(rewritten_str) = String::from_utf8(rewritten.into())
                && let Ok(parsed) = StructuredAttrs::parse(rewritten_str)
            {
                *sa = parsed;
            }
        }
    }
}

impl Derivation {
    /// Create a new derivation with the given name, platform, and builder, which are the minimal
    /// fields for a derivation.
    pub fn new(name: StorePathName, platform: bytes::Bytes, builder: bytes::Bytes) -> Self {
        Self {
            name,
            outputs: DerivationOutputs::new(),
            inputs: BTreeSet::new(),
            platform,
            builder,
            args: Vec::new(),
            env: Default::default(),
            structured_attrs: None,
        }
    }
}

#[cfg(test)]
mod structured_attrs_tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    fn hash_of(sa: &StructuredAttrs) -> u64 {
        let mut h = DefaultHasher::new();
        sa.hash(&mut h);
        h.finish()
    }

    #[test]
    fn parse_preserves_verbatim_and_mut_clears_it() {
        for raw in [
            r#"{"x":1e+15}"#,
            r#"{"x":1e-05}"#,
            r#"{ "b": 2, "a": 1 }"#,
            r#"{"s":"\u001B"}"#,
        ] {
            let mut sa = StructuredAttrs::parse(raw).unwrap();
            assert_eq!(sa.verbatim(), Some(raw));
            assert_eq!(sa.unparse().as_ref(), raw);
            assert!(matches!(sa.unparse(), Cow::Borrowed(_)));

            sa.attrs_mut()
                .insert("added".into(), serde_json::json!(true));
            assert_eq!(sa.verbatim(), None);
            assert!(sa.unparse().contains("\"added\":true"));
        }
    }

    #[test]
    fn parse_rejects_invalid_or_non_object_json() {
        for bad in ["", "not-json", "[1,2]", "\"str\"", "42", "null"] {
            assert!(
                StructuredAttrs::parse(bad).is_err(),
                "expected error for {bad:?}"
            );
        }
    }

    #[test]
    fn eq_and_hash_consistent_across_verbatim_and_canonical() {
        let canonical_parsed = StructuredAttrs::parse(r#"{"a":1}"#).unwrap();
        let canonical_new = StructuredAttrs::new(serde_json::from_str(r#"{"a":1}"#).unwrap());
        assert_eq!(canonical_parsed, canonical_new);
        assert_eq!(hash_of(&canonical_parsed), hash_of(&canonical_new));

        let exp_parsed = StructuredAttrs::parse(r#"{"x":1e+15}"#).unwrap();
        let exp_new = StructuredAttrs::new(serde_json::from_str(r#"{"x":1e+15}"#).unwrap());
        assert_ne!(exp_parsed, exp_new);

        let neg_zero = StructuredAttrs::new(serde_json::from_str(r#"{"z":-0.0}"#).unwrap());
        let pos_zero = StructuredAttrs::new(serde_json::from_str(r#"{"z":0.0}"#).unwrap());
        let pos_zero_parsed = StructuredAttrs::parse(r#"{"z":0.0}"#).unwrap();
        assert_eq!(pos_zero, pos_zero_parsed);
        assert_eq!(hash_of(&pos_zero), hash_of(&pos_zero_parsed));
        assert_ne!(neg_zero, pos_zero);
        assert_ne!(neg_zero, pos_zero_parsed);
    }

    /// With one verbatim side, equality compares encodings only, as C++ Nix
    /// does. `serde_json`'s default float parser does not round-trip this
    /// value exactly, so the parsed maps differ although the encodings match.
    #[test]
    fn eq_compares_encodings_when_one_side_is_verbatim() {
        let mut attrs = serde_json::Map::new();
        let x = f64::from_bits(0x305f_050c_368d_cc74);
        attrs.insert("x".into(), serde_json::Value::from(x));
        let canonical = StructuredAttrs::new(attrs);
        let parsed = StructuredAttrs::parse(canonical.unparse().into_owned()).unwrap();
        assert_eq!(parsed, canonical);
        assert_eq!(canonical, parsed);
        assert_eq!(hash_of(&parsed), hash_of(&canonical));
    }

    #[test]
    fn apply_rewrites_preserves_verbatim_formatting() {
        let mut drv = Derivation::new(
            "test".parse().unwrap(),
            bytes::Bytes::from_static(b"x86_64-linux"),
            bytes::Bytes::from_static(b"/bin/sh"),
        );
        drv.structured_attrs =
            Some(StructuredAttrs::parse(r#"{"p":"/placeholder","x":1e+15}"#).unwrap());

        // Non-matching rewrite leaves verbatim intact.
        let mut no_match = BTreeMap::new();
        no_match.insert(
            bytes::Bytes::from_static(b"/unused"),
            bytes::Bytes::from_static(b"/replaced"),
        );
        drv.apply_rewrites(&no_match);
        assert_eq!(
            drv.structured_attrs.as_ref().unwrap().verbatim(),
            Some(r#"{"p":"/placeholder","x":1e+15}"#),
        );

        // Matching rewrite preserves untouched float formatting verbatim.
        let mut matched = BTreeMap::new();
        matched.insert(
            bytes::Bytes::from_static(b"/placeholder"),
            bytes::Bytes::from_static(b"/nix/store/00000000000000000000000000000000-out"),
        );
        drv.apply_rewrites(&matched);
        let sa = drv.structured_attrs.as_ref().unwrap();
        assert_eq!(
            sa.verbatim(),
            Some(r#"{"p":"/nix/store/00000000000000000000000000000000-out","x":1e+15}"#),
        );
        assert_eq!(
            sa.attrs().get("p").and_then(|v| v.as_str()),
            Some("/nix/store/00000000000000000000000000000000-out"),
        );
    }
}
