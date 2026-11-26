use std::collections::{BTreeMap, BTreeSet};

use memchr::memmem;
#[cfg(test)]
use proptest::prelude::{Arbitrary, BoxedStrategy};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::derived_path::SingleDerivedPath;
use harmonia_store_path::{StorePathName, StorePathSet};

use super::inputs::DerivationInputsRef;
use super::{DerivationInputs, DerivationOutput, DerivationOutputs};

/// Structured attributes for a derivation.
///
/// When present, derivation attributes are passed to the builder as a JSON object
/// via the `__json` environment variable, rather than individual environment variables.
/// This allows passing complex data structures (arrays, nested objects, etc.) to builders.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StructuredAttrs {
    /// The structured attributes as a JSON object.
    ///
    /// Note: The `env` map must not contain the key `__json` when structured attrs are used,
    /// as that key is reserved for encoding the structured attributes themselves.
    pub attrs: serde_json::Map<String, serde_json::Value>,
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
            StructuredAttrs { attrs }
        })
    }
}

struct StrMapSlice<'a>(&'a [(&'a str, &'a str)]);

impl Serialize for StrMapSlice<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().copied())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DerivationRef<'a, Inputs: Serialize> {
    name: &'a str,
    outputs: &'a DerivationOutputs,
    inputs: Inputs,
    #[serde(rename = "system")]
    platform: &'a str,
    builder: &'a str,
    args: &'a [&'a str],
    env: StrMapSlice<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_attrs: Option<&'a StructuredAttrs>,
    version: u32,
}

fn serialize_derivation<'a, Inputs, SerInputs, S>(
    drv: &'a DerivationT<Inputs>,
    make_inputs: impl FnOnce(&'a Inputs) -> SerInputs,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    SerInputs: Serialize,
    S: Serializer,
{
    // Validate all UTF-8 upfront so streaming serializers never receive partial
    // output if a later arg or env entry is invalid UTF-8.
    let platform = std::str::from_utf8(&drv.platform).map_err(serde::ser::Error::custom)?;
    let builder = std::str::from_utf8(&drv.builder).map_err(serde::ser::Error::custom)?;
    let args: Vec<&str> = drv
        .args
        .iter()
        .map(|b| std::str::from_utf8(b))
        .collect::<Result<_, _>>()
        .map_err(serde::ser::Error::custom)?;
    let env: Vec<(&str, &str)> = drv
        .env
        .iter()
        .map(|(k, v)| Ok((std::str::from_utf8(k)?, std::str::from_utf8(v)?)))
        .collect::<Result<_, std::str::Utf8Error>>()
        .map_err(serde::ser::Error::custom)?;
    DerivationRef {
        name: drv.name.as_ref(),
        outputs: &drv.outputs,
        inputs: make_inputs(&drv.inputs),
        platform,
        builder,
        args: &args,
        env: StrMapSlice(&env),
        structured_attrs: drv.structured_attrs.as_ref(),
        version: default_version(),
    }
    .serialize(serializer)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DerivationDe<Inputs> {
    name: String,
    outputs: DerivationOutputs,
    inputs: Inputs,
    #[serde(rename = "system")]
    platform: String,
    builder: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    #[serde(default)]
    structured_attrs: Option<StructuredAttrs>,
    #[serde(default = "default_version")]
    version: u32,
}

fn deserialize_derivation<'de, Inputs, HelperInputs, D>(
    deserializer: D,
    convert_inputs: impl FnOnce(HelperInputs) -> Inputs,
) -> Result<DerivationT<Inputs>, D::Error>
where
    HelperInputs: Deserialize<'de>,
    D: Deserializer<'de>,
{
    let helper = DerivationDe::<HelperInputs>::deserialize(deserializer)?;
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
        inputs: convert_inputs(helper.inputs),
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
        serialize_derivation(self, |i| i, serializer)
    }
}

impl<'de> Deserialize<'de> for BasicDerivation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_derivation(deserializer, std::convert::identity)
    }
}

impl Serialize for Derivation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_derivation(self, DerivationInputsRef::from, serializer)
    }
}

impl<'de> Deserialize<'de> for Derivation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_derivation(deserializer, |h: DerivationInputs| BTreeSet::from(&h))
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

#[inline]
fn hash_pair(b1: u8, b2: u8) -> usize {
    (u16::from_le_bytes([b1, b2]).wrapping_mul(0x9e37) >> 8) as usize
}

fn replace_matches<'a>(
    haystack: &[u8],
    from_len: usize,
    mut next_match: impl FnMut(usize) -> Option<(usize, &'a bytes::Bytes)>,
) -> Option<bytes::Bytes> {
    let (first, first_to) = next_match(0)?;
    if first == 0 && from_len == haystack.len() {
        return Some(first_to.clone());
    }
    let cap = (haystack.len() - from_len).saturating_add(first_to.len());
    let mut out = Vec::with_capacity(cap);
    out.extend_from_slice(&haystack[..first]);
    out.extend_from_slice(first_to);
    let mut pos = first + from_len;
    while let Some((idx, to)) = next_match(pos) {
        out.extend_from_slice(&haystack[pos..idx]);
        out.extend_from_slice(to);
        pos = idx + from_len;
    }
    out.extend_from_slice(&haystack[pos..]);
    Some(bytes::Bytes::from(out))
}

struct DelimFixedRewriter<'a> {
    delim: u8,
    key_len: usize,
    pair_mask: [u64; 4],
    /// Sorted by key (preserved from `BTreeMap` iteration order).
    pairs: Vec<(&'a [u8], &'a bytes::Bytes)>,
}

impl<'a> DelimFixedRewriter<'a> {
    fn find_from(&self, haystack: &[u8], start: usize) -> Option<(usize, &'a bytes::Bytes)> {
        let end_limit = haystack.len().checked_sub(self.key_len)?;
        let second_off = (self.key_len - 1).min(2);
        let mut pos = start;
        while pos <= end_limit {
            let rel = memchr::memchr(self.delim, &haystack[pos..=end_limit])?;
            let idx = pos + rel;
            let h = hash_pair(haystack[idx + 1], haystack[idx + second_off]);
            if (self.pair_mask[h >> 6] & (1u64 << (h & 63))) != 0 {
                let candidate = &haystack[idx..idx + self.key_len];
                if let Ok(found) = self.pairs.binary_search_by_key(&candidate, |&(k, _)| k) {
                    return Some((idx, self.pairs[found].1));
                }
            }
            pos = idx + 1;
        }
        None
    }
}

enum Rewriter<'a> {
    /// Fast single-pass SIMD `memchr` scanner when multiple keys share a fixed
    /// length and unique leading delimiter byte (all Nix output placeholders).
    DelimFixed(DelimFixedRewriter<'a>),
    /// Single-key and general sequential fallback for overlapping or chained keys.
    Sequential(Vec<(memmem::Finder<'a>, &'a bytes::Bytes)>),
}

impl<'a> Rewriter<'a> {
    fn new(rewrites: &'a BTreeMap<bytes::Bytes, bytes::Bytes>) -> Option<Self> {
        let mut pairs: Vec<(&'a [u8], &'a bytes::Bytes)> = Vec::with_capacity(rewrites.len());
        for (from, to) in rewrites {
            assert!(!from.is_empty(), "rewrite key must not be empty");
            if from != to {
                pairs.push((from.as_ref(), to));
            }
        }
        if pairs.is_empty() {
            return None;
        }
        if pairs.len() > 1 {
            let key_len = pairs[0].0.len();
            let delim = pairs[0].0[0];
            let keys_disjoint = key_len >= 2
                && pairs.iter().all(|(from, _)| {
                    from.len() == key_len
                        && from[0] == delim
                        && memchr::memchr(delim, &from[1..]).is_none()
                });
            if keys_disjoint {
                let second_off = (key_len - 1).min(2);
                let mut pair_mask = [0u64; 4];
                for &(from, _) in &pairs {
                    let h = hash_pair(from[1], from[second_off]);
                    pair_mask[h >> 6] |= 1u64 << (h & 63);
                }
                let candidate = DelimFixedRewriter {
                    delim,
                    key_len,
                    pair_mask,
                    pairs,
                };
                let single_pass_safe = candidate.pairs.iter().all(|&(_, to)| {
                    let to = to.as_ref();
                    if to.first() != Some(&delim) || candidate.find_from(to, 0).is_some() {
                        return false;
                    }
                    let last_delim = memchr::memrchr(delim, to).unwrap();
                    let tail = &to[last_delim..];
                    if tail.len() >= key_len {
                        return true;
                    }
                    // `candidate.pairs` is sorted, so any key starting with `tail`
                    // must be the first key >= `tail`.
                    let idx = candidate.pairs.partition_point(|&(from, _)| from < tail);
                    candidate
                        .pairs
                        .get(idx)
                        .is_none_or(|&(from, _)| !from.starts_with(tail))
                });
                if single_pass_safe {
                    return Some(Self::DelimFixed(candidate));
                }
                pairs = candidate.pairs;
            }
        }
        let finders = pairs
            .into_iter()
            .map(|(from, to)| (memmem::Finder::new(from), to))
            .collect();
        Some(Self::Sequential(finders))
    }

    fn rewrite(&self, haystack: &[u8]) -> Option<bytes::Bytes> {
        match self {
            Self::DelimFixed(rw) => {
                replace_matches(haystack, rw.key_len, |pos| rw.find_from(haystack, pos))
            }
            Self::Sequential(finders) => {
                let mut buf: Option<bytes::Bytes> = None;
                for (finder, to) in finders {
                    let cur = buf.as_deref().unwrap_or(haystack);
                    let from_len = finder.needle().len();
                    if let Some(next) = replace_matches(cur, from_len, |pos| {
                        finder.find(&cur[pos..]).map(|rel| (pos + rel, *to))
                    }) {
                        buf = Some(next);
                    }
                }
                buf
            }
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
        let Some(rewriter) = Rewriter::new(rewrites) else {
            return;
        };

        if let Some(builder) = rewriter.rewrite(&self.builder) {
            self.builder = builder;
        }

        for arg in &mut self.args {
            if let Some(rewritten) = rewriter.rewrite(arg) {
                *arg = rewritten;
            }
        }

        // Rewrite values in place in a single BTreeMap traversal, deferring
        // key re-insertion to the rare case where an env key itself matched.
        let mut renamed_keys: Vec<(bytes::Bytes, bytes::Bytes)> = Vec::new();
        for (k, v) in &mut self.env {
            if let Some(rewritten) = rewriter.rewrite(v) {
                *v = rewritten;
            }
            if let Some(new_k) = rewriter.rewrite(k) {
                renamed_keys.push((k.clone(), new_k));
            }
        }
        if !renamed_keys.is_empty() {
            // Re-insert every entry in ascending original-key order so the
            // later original key wins any collision, whether between a renamed
            // and an unchanged key or between two renamed keys.
            // `renamed_keys` was collected in that same order.
            let mut renamed = renamed_keys.into_iter().peekable();
            for (k, v) in std::mem::take(&mut self.env) {
                let k = match renamed.next_if(|(old_k, _)| *old_k == k) {
                    Some((_, new_k)) => new_k,
                    None => k,
                };
                self.env.insert(k, v);
            }
        }

        if let Some(ref mut sa) = self.structured_attrs {
            let json = serde_json::to_string(&sa.attrs).unwrap();
            if let Some(rewritten) = rewriter.rewrite(json.as_bytes())
                && let Ok(attrs) = serde_json::from_slice(&rewritten)
            {
                sa.attrs = attrs;
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
mod tests {
    use super::*;
    use crate::derived_path::OutputName;
    use crate::placeholder::Placeholder;
    use bytes::Bytes;
    use harmonia_store_path::StorePath;
    use proptest::prelude::*;

    /// Reference sequential byte-by-byte rewriter matching the original algorithm.
    fn reference_rewrite(s: &[u8], rewrites: &BTreeMap<Bytes, Bytes>) -> Vec<u8> {
        let mut buf = s.to_vec();
        for (from, to) in rewrites {
            let mut i = 0;
            while i + from.len() <= buf.len() {
                if &buf[i..i + from.len()] == from.as_ref() {
                    buf.splice(i..i + from.len(), to.iter().copied());
                    i += to.len();
                } else {
                    i += 1;
                }
            }
        }
        buf
    }

    fn sample_basic() -> BasicDerivation {
        BasicDerivation {
            name: "test".parse().unwrap(),
            outputs: BTreeMap::new(),
            inputs: StorePathSet::new(),
            platform: Bytes::from_static(b"x86_64-linux"),
            builder: Bytes::from_static(b"/bin/sh"),
            args: vec![Bytes::from_static(b"-c"), Bytes::from_static(b"echo hi")],
            env: BTreeMap::from([
                (Bytes::from_static(b"A"), Bytes::from_static(b"val")),
                (Bytes::from_static(b"B"), Bytes::from_static(b"other")),
            ]),
            structured_attrs: None,
        }
    }

    #[test]
    fn apply_rewrites_delim_fixed_placeholders() {
        let sp1: StorePath = "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-a.drv".parse().unwrap();
        let sp2: StorePath = "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-b.drv".parse().unwrap();
        let ph1 = Bytes::from(
            Placeholder::ca_output(&sp1, &OutputName::default())
                .render()
                .to_string_lossy()
                .into_owned(),
        );
        let ph2 = Bytes::from(
            Placeholder::ca_output(&sp2, &OutputName::default())
                .render()
                .to_string_lossy()
                .into_owned(),
        );
        let out1 = Bytes::from_static(b"/nix/store/g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-a");
        let out2 = Bytes::from_static(b"/nix/store/7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-b");
        let rewrites = BTreeMap::from([(ph1.clone(), out1.clone()), (ph2.clone(), out2.clone())]);

        let rw = Rewriter::new(&rewrites).unwrap();
        assert!(matches!(rw, Rewriter::DelimFixed(_)));

        let mut drv = sample_basic();
        let combined = [ph1.as_ref(), b" ", ph2.as_ref(), b":", ph1.as_ref()].concat();
        drv.builder = ph1.clone();
        drv.args.push(Bytes::from(combined.clone()));
        drv.env
            .insert(Bytes::from_static(b"deps"), Bytes::from(combined));
        drv.apply_rewrites(&rewrites);

        let expected = [out1.as_ref(), b" ", out2.as_ref(), b":", out1.as_ref()].concat();
        assert_eq!(drv.builder, out1);
        assert_eq!(drv.args[2].as_ref(), expected.as_slice());
        assert_eq!(drv.env[b"deps".as_slice()].as_ref(), expected.as_slice());
        assert_eq!(drv.env[b"A".as_slice()].as_ref(), b"val");
    }

    #[test]
    fn apply_rewrites_chained_and_env_key_collisions() {
        // Sequential semantics: "aa" -> "bb" then "bb" -> "cc" turns "aa" into "cc".
        let rewrites = BTreeMap::from([
            (Bytes::from_static(b"aa"), Bytes::from_static(b"bb")),
            (Bytes::from_static(b"bb"), Bytes::from_static(b"cc")),
        ]);
        let rw = Rewriter::new(&rewrites).unwrap();
        assert!(matches!(rw, Rewriter::Sequential(_)));

        let mut drv = sample_basic();
        drv.env.insert(
            Bytes::from_static(b"key_aa"),
            Bytes::from_static(b"from_aa"),
        );
        // Collides with later unchanged key "key_cc": later key must win.
        drv.env.insert(
            Bytes::from_static(b"key_cc"),
            Bytes::from_static(b"unchanged_cc"),
        );
        // Earlier unchanged key "key_0" collided by renamed "key_aa" -> nope; test
        // rename overwriting an EARLIER unchanged key too.
        let mut drv2 = sample_basic();
        let rewrites2 = BTreeMap::from([(Bytes::from_static(b"z"), Bytes::from_static(b"a"))]);
        drv2.env
            .insert(Bytes::from_static(b"a"), Bytes::from_static(b"early"));
        drv2.env
            .insert(Bytes::from_static(b"z"), Bytes::from_static(b"late"));
        drv2.apply_rewrites(&rewrites2);
        assert_eq!(drv2.env[b"a".as_slice()].as_ref(), b"late");
        assert!(!drv2.env.contains_key(b"z".as_slice()));

        // Two renamed keys sharing a destination: the later original key wins.
        let mut drv3 = sample_basic();
        let rewrites3 = BTreeMap::from([
            (Bytes::from_static(b"a"), Bytes::from_static(b"z")),
            (Bytes::from_static(b"b"), Bytes::from_static(b"z")),
        ]);
        drv3.env
            .insert(Bytes::from_static(b"x_a"), Bytes::from_static(b"first"));
        drv3.env
            .insert(Bytes::from_static(b"x_b"), Bytes::from_static(b"second"));
        drv3.apply_rewrites(&rewrites3);
        assert_eq!(drv3.env[b"x_z".as_slice()].as_ref(), b"second");
        assert!(!drv3.env.contains_key(b"x_a".as_slice()));
        assert!(!drv3.env.contains_key(b"x_b".as_slice()));

        drv.apply_rewrites(&rewrites);
        assert!(!drv.env.contains_key(b"key_aa".as_slice()));
        assert_eq!(drv.env[b"key_cc".as_slice()].as_ref(), b"unchanged_cc");
    }

    #[test]
    fn apply_rewrites_noop_and_atomic_invalid_utf8_serialize() {
        let noop = BTreeMap::from([(Bytes::from_static(b"x"), Bytes::from_static(b"x"))]);
        assert!(Rewriter::new(&noop).is_none());

        let single = BTreeMap::from([(Bytes::from_static(b"hi"), Bytes::from_static(b"bye"))]);
        assert!(matches!(
            Rewriter::new(&single).unwrap(),
            Rewriter::Sequential(_)
        ));
        let mut drv = sample_basic();
        drv.apply_rewrites(&single);
        assert_eq!(drv.args[1].as_ref(), b"echo bye");

        // Invalid UTF-8 in args or env must fail before writing any bytes.
        let mut bad = sample_basic();
        bad.args.push(Bytes::from_static(&[0xff]));
        let mut out = Vec::new();
        assert!(serde_json::to_writer(&mut out, &bad).is_err());
        assert!(out.is_empty());

        let mut bad_env = sample_basic();
        bad_env
            .env
            .insert(Bytes::from_static(b"BAD"), Bytes::from_static(&[0xff]));
        let mut out = Vec::new();
        assert!(serde_json::to_writer(&mut out, &bad_env).is_err());
        assert!(out.is_empty());
    }

    #[test]
    #[should_panic(expected = "rewrite key must not be empty")]
    fn apply_rewrites_empty_key_panics() {
        let mut drv = sample_basic();
        let rewrites = BTreeMap::from([(Bytes::new(), Bytes::from_static(b"x"))]);
        drv.apply_rewrites(&rewrites);
    }

    proptest! {
        #[test]
        fn proptest_rewriter_matches_reference(
            haystack in prop::collection::vec(prop::sample::select(vec![b'/', b'a', b'b', b'c', b'-']), 0..64),
            entries in prop::collection::vec(
                (
                    prop::collection::vec(prop::sample::select(vec![b'/', b'a', b'b', b'c']), 1..6),
                    prop::collection::vec(prop::sample::select(vec![b'/', b'a', b'b', b'c', b'-']), 0..8),
                ),
                0..5,
            ),
        ) {
            let rewrites: BTreeMap<Bytes, Bytes> = entries
                .into_iter()
                .map(|(k, v)| (Bytes::from(k), Bytes::from(v)))
                .collect();
            let expected = reference_rewrite(&haystack, &rewrites);
            let actual = Rewriter::new(&rewrites)
                .and_then(|rw| rw.rewrite(&haystack))
                .map(|b| b.to_vec())
                .unwrap_or_else(|| haystack.clone());
            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn proptest_delim_fixed_matches_reference(
            haystack in prop::collection::vec(prop::sample::select(vec![b'/', b'a', b'b', b'c', b'-']), 0..80),
            key_bodies in prop::collection::btree_set(
                prop::collection::vec(prop::sample::select(vec![b'a', b'b', b'c']), 4),
                2..5,
            ),
            val_tails in prop::collection::vec(
                prop::collection::vec(prop::sample::select(vec![b'/', b'a', b'b', b'c', b'-']), 1..10),
                5,
            ),
        ) {
            let rewrites: BTreeMap<Bytes, Bytes> = key_bodies
                .into_iter()
                .zip(val_tails)
                .map(|(mut k, mut v)| {
                    k.insert(0, b'/');
                    v.insert(0, b'/');
                    (Bytes::from(k), Bytes::from(v))
                })
                .collect();
            let expected = reference_rewrite(&haystack, &rewrites);
            let actual = Rewriter::new(&rewrites)
                .and_then(|rw| rw.rewrite(&haystack))
                .map(|b| b.to_vec())
                .unwrap_or_else(|| haystack.clone());
            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn proptest_env_rewrites_match_reference(
            env in prop::collection::btree_map(
                prop::collection::vec(prop::sample::select(vec![b'a', b'b', b'c']), 1..4),
                prop::collection::vec(prop::sample::select(vec![b'a', b'b', b'c']), 0..4),
                0..8,
            ),
            entries in prop::collection::vec(
                (
                    prop::collection::vec(prop::sample::select(vec![b'a', b'b', b'c']), 1..3),
                    prop::collection::vec(prop::sample::select(vec![b'a', b'b', b'c']), 0..3),
                ),
                1..4,
            ),
        ) {
            let rewrites: BTreeMap<Bytes, Bytes> = entries
                .into_iter()
                .map(|(k, v)| (Bytes::from(k), Bytes::from(v)))
                .collect();
            let mut drv = sample_basic();
            drv.env = env
                .into_iter()
                .map(|(k, v)| (Bytes::from(k), Bytes::from(v)))
                .collect();
            // The original algorithm: re-insert every entry in ascending
            // original-key order, so the later original key wins collisions.
            let mut expected = BTreeMap::new();
            for (k, v) in &drv.env {
                expected.insert(
                    Bytes::from(reference_rewrite(k, &rewrites)),
                    Bytes::from(reference_rewrite(v, &rewrites)),
                );
            }
            drv.apply_rewrites(&rewrites);
            prop_assert_eq!(drv.env, expected);
        }
    }
}
