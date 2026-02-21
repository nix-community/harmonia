// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! ATerm serialization and deserialization for Nix derivation (`.drv`) files.
//!
//! The ATerm format is Nix's on-disk representation for derivations:
//!
//! ```text
//! Derive([(outputs)],[(input_drvs)],[(input_srcs)],"platform","builder",[(args)],[(env)])
//! ```
//!
//! Parsing and serialization are byte-for-byte compatible with Nix's
//! `parseDerivation` / `Derivation::unparse` in `src/libstore/derivations.cc`.

use std::collections::{BTreeMap, BTreeSet};

use harmonia_store_core::ByteString;
use harmonia_store_core::derivation::{
    BasicDerivation, DerivationOutput, DerivationOutputs, OutputPathName, StructuredAttrs,
};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::{
    ContentAddress, ContentAddressMethodAlgorithm, StoreDir, StorePath, StorePathName,
};
use harmonia_utils_hash::Hash;
use harmonia_utils_hash::fmt::{Base16, CommonHash};

/// Errors that can occur during ATerm parsing.
#[derive(Debug, thiserror::Error)]
pub enum ATermError {
    #[error("unexpected end of input at position {0}")]
    UnexpectedEof(usize),
    #[error("at position {pos}: expected {expected}, got {got:?}")]
    Expected {
        pos: usize,
        expected: String,
        got: String,
    },
    #[error("invalid store path: {0}")]
    InvalidStorePath(String),
    #[error("invalid derivation output: {0}")]
    InvalidOutput(String),
    #[error("invalid structured attrs JSON: {0}")]
    InvalidStructuredAttrs(String),
}

/// Parse a `.drv` file in ATerm format into a `BasicDerivation`.
///
/// The `store_dir` is needed to validate and parse store paths.
/// The `name` is the derivation name (extracted from the `.drv` filename).
pub fn parse(store_dir: &StoreDir, input: &str, name: &str) -> Result<BasicDerivation, ATermError> {
    let mut p = Parser::new(input);

    p.expect_str("Derive(")?;

    let outputs = p.parse_outputs(store_dir)?;
    p.expect_char(',')?;
    let input_drvs = p.parse_input_drvs(store_dir)?;
    p.expect_char(',')?;
    let input_srcs = p.parse_input_srcs(store_dir)?;
    p.expect_char(',')?;
    let platform = p.parse_string()?;
    p.expect_char(',')?;
    let builder = p.parse_string()?;
    p.expect_char(',')?;
    let args = p.parse_string_list()?;
    p.expect_char(',')?;
    let (env, structured_attrs) = p.parse_env()?;
    p.expect_char(')')?;

    let drv_name = name
        .parse()
        .map_err(|e| ATermError::InvalidStorePath(format!("invalid derivation name: {e}")))?;

    // BasicDerivation.inputs = input_srcs ∪ input_drv store paths
    let mut inputs: BTreeSet<StorePath> = input_srcs;
    for drv_path in input_drvs.keys() {
        inputs.insert(drv_path.clone());
    }

    Ok(BasicDerivation {
        name: drv_name,
        outputs,
        inputs,
        platform: ByteString::from(platform),
        builder: ByteString::from(builder),
        args: args.into_iter().map(ByteString::from).collect(),
        env: env
            .into_iter()
            .map(|(k, v)| (ByteString::from(k), ByteString::from(v)))
            .collect(),
        structured_attrs,
    })
}

/// Serialize a `BasicDerivation` to ATerm format.
///
/// `input_drvs` maps input derivation paths to their requested output names.
/// Paths in `drv.inputs` that are not in `input_drvs` are serialized as input sources.
pub fn unparse(
    store_dir: &StoreDir,
    drv: &BasicDerivation,
    input_drvs: &BTreeMap<StorePath, BTreeSet<String>>,
) -> String {
    let mut s = String::with_capacity(4096);
    s.push_str("Derive(");

    // Outputs
    write_list(&mut s, drv.outputs.iter(), |s, (output_name, output)| {
        s.push('(');
        write_string(s, output_name.as_ref());
        s.push(',');
        let (path_str, method_str, hash_str) =
            encode_output_fields(output, store_dir, &drv.name, output_name, |h| {
                h.base16().bare().to_string()
            });
        write_string(s, &path_str);
        s.push(',');
        write_string(s, &method_str);
        s.push(',');
        write_string(s, &hash_str);
        s.push(')');
    });

    // Input derivations
    s.push_str(",[");
    write_comma_separated(&mut s, input_drvs.iter(), |s, (drv_path, outputs)| {
        s.push('(');
        write_string(s, &store_dir.display(drv_path).to_string());
        s.push(',');
        write_list(s, outputs.iter(), |s, out| write_string(s, out));
        s.push(')');
    });
    s.push(']');

    // Input sources: paths in drv.inputs not in input_drvs
    let drv_set: BTreeSet<&StorePath> = input_drvs.keys().collect();
    s.push(',');
    write_list(
        &mut s,
        drv.inputs.iter().filter(|p| !drv_set.contains(p)),
        |s, path| write_string(s, &store_dir.display(path).to_string()),
    );

    s.push(',');
    write_string(&mut s, std::str::from_utf8(&drv.platform).unwrap_or(""));
    s.push(',');
    write_string(&mut s, std::str::from_utf8(&drv.builder).unwrap_or(""));

    // Args
    s.push(',');
    write_list(&mut s, drv.args.iter(), |s, arg| {
        write_string(s, std::str::from_utf8(arg).unwrap_or(""))
    });

    // Environment
    s.push_str(",[");
    let mut env_entries: BTreeMap<&str, &str> = drv
        .env
        .iter()
        .filter_map(|(k, v)| Some((std::str::from_utf8(k).ok()?, std::str::from_utf8(v).ok()?)))
        .collect();

    let json_str;
    if let Some(ref sa) = drv.structured_attrs {
        json_str = serde_json::Value::Object(sa.attrs.clone()).to_string();
        env_entries.insert("__json", &json_str);
    }

    write_comma_separated(&mut s, env_entries.iter(), |s, (key, value)| {
        s.push('(');
        write_string(s, key);
        s.push(',');
        write_string(s, value);
        s.push(')');
    });
    s.push_str("])");

    s
}

// ── Shared decode/encode (reused by wire protocol in store_impls.rs) ────────

/// Decode a `DerivationOutput` from three string fields.
///
/// Both ATerm and the daemon wire protocol represent derivation outputs as
/// `(path, method, hash)` string triples with the same branching rules.
/// The only difference is hash encoding, so `parse_hash` is a callback.
pub(crate) fn decode_output_fields(
    store_dir: &StoreDir,
    path_str: &str,
    method_str: &str,
    hash_str: &str,
    parse_hash: impl FnOnce(
        &ContentAddressMethodAlgorithm,
        &str,
    ) -> Result<Hash, harmonia_utils_hash::fmt::ParseHashError>,
) -> Result<DerivationOutput, String> {
    if hash_str == "impure" {
        let algo = method_str.parse().map_err(|e| format!("{e}"))?;
        return Ok(DerivationOutput::Impure(algo));
    }
    if !method_str.is_empty() && !hash_str.is_empty() {
        let algo: ContentAddressMethodAlgorithm = method_str.parse().map_err(|e| format!("{e}"))?;
        let hash = parse_hash(&algo, hash_str).map_err(|e| format!("{hash_str}: {e}"))?;
        let ca = ContentAddress::from_hash(algo.method(), hash)
            .map_err(|e| format!("invalid CA: {e}"))?;
        return Ok(DerivationOutput::CAFixed(ca));
    }
    if !method_str.is_empty() {
        let algo = method_str.parse().map_err(|e| format!("{e}"))?;
        return Ok(DerivationOutput::CAFloating(algo));
    }
    if path_str.is_empty() {
        return Ok(DerivationOutput::Deferred);
    }
    let store_path = store_dir
        .parse::<StorePath>(path_str)
        .map_err(|e| format!("{path_str}: {e}"))?;
    Ok(DerivationOutput::InputAddressed(store_path))
}

/// Encode a `DerivationOutput` to three string fields `(path, method, hash)`.
///
/// `fmt_hash` formats the raw hash bytes; pass `|h| h.base16().bare()` for
/// ATerm and `|h| h.base32().bare()` for the wire protocol.
pub(crate) fn encode_output_fields(
    output: &DerivationOutput,
    store_dir: &StoreDir,
    drv_name: &StorePathName,
    output_name: &OutputName,
    fmt_hash: impl FnOnce(Hash) -> String,
) -> (String, String, String) {
    match output {
        DerivationOutput::InputAddressed(path) => (
            store_dir.display(path).to_string(),
            String::new(),
            String::new(),
        ),
        DerivationOutput::CAFixed(ca) => {
            let out_name = OutputPathName {
                drv_name,
                output_name,
            }
            .to_string()
            .parse()
            .expect("output path name should be valid");
            let path = store_dir.make_store_path_from_ca(out_name, *ca);
            (
                store_dir.display(&path).to_string(),
                ca.method_algorithm().to_string(),
                fmt_hash(ca.hash()),
            )
        }
        DerivationOutput::CAFloating(algo) => (String::new(), algo.to_string(), String::new()),
        DerivationOutput::Deferred => (String::new(), String::new(), String::new()),
        DerivationOutput::Impure(algo) => (String::new(), algo.to_string(), "impure".to_string()),
    }
}

// ── Serialization helpers ────────────────────────────────────────────────────

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

fn write_list<I, F>(out: &mut String, iter: I, mut write_item: F)
where
    I: Iterator,
    F: FnMut(&mut String, I::Item),
{
    out.push('[');
    write_comma_separated(out, iter, &mut write_item);
    out.push(']');
}

fn write_comma_separated<I, F>(out: &mut String, iter: I, mut write_item: F)
where
    I: Iterator,
    F: FnMut(&mut String, I::Item),
{
    let mut first = true;
    for item in iter {
        if !first {
            out.push(',');
        }
        first = false;
        write_item(out, item);
    }
}

// ── Parser ───────────────────────────────────────────────────────────────────

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn peek(&self) -> Result<char, ATermError> {
        self.remaining()
            .chars()
            .next()
            .ok_or(ATermError::UnexpectedEof(self.pos))
    }

    fn advance(&mut self, n: usize) {
        self.pos += n;
    }

    fn expect_char(&mut self, expected: char) -> Result<(), ATermError> {
        let got = self.peek()?;
        if got == expected {
            self.advance(expected.len_utf8());
            Ok(())
        } else {
            Err(ATermError::Expected {
                pos: self.pos,
                expected: format!("'{expected}'"),
                got: got.to_string(),
            })
        }
    }

    fn expect_str(&mut self, expected: &str) -> Result<(), ATermError> {
        if self.remaining().starts_with(expected) {
            self.advance(expected.len());
            Ok(())
        } else {
            let got_len = expected.len().min(self.remaining().len());
            Err(ATermError::Expected {
                pos: self.pos,
                expected: format!("{expected:?}"),
                got: self.remaining()[..got_len].to_string(),
            })
        }
    }

    fn parse_string(&mut self) -> Result<String, ATermError> {
        self.expect_char('"')?;
        let mut result = String::new();
        loop {
            let c = self.peek()?;
            self.advance(c.len_utf8());
            match c {
                '"' => return Ok(result),
                '\\' => {
                    let escaped = self.peek()?;
                    self.advance(escaped.len_utf8());
                    match escaped {
                        'n' => result.push('\n'),
                        'r' => result.push('\r'),
                        't' => result.push('\t'),
                        other => result.push(other),
                    }
                }
                other => result.push(other),
            }
        }
    }

    fn parse_list<T>(
        &mut self,
        mut parse_item: impl FnMut(&mut Self) -> Result<T, ATermError>,
    ) -> Result<Vec<T>, ATermError> {
        self.expect_char('[')?;
        let mut result = Vec::new();
        if self.peek()? == ']' {
            self.advance(1);
            return Ok(result);
        }
        loop {
            result.push(parse_item(self)?);
            match self.peek()? {
                ',' => self.advance(1),
                ']' => {
                    self.advance(1);
                    return Ok(result);
                }
                c => {
                    return Err(ATermError::Expected {
                        pos: self.pos,
                        expected: "',' or ']'".to_string(),
                        got: c.to_string(),
                    });
                }
            }
        }
    }

    fn parse_string_list(&mut self) -> Result<Vec<String>, ATermError> {
        self.parse_list(|p| p.parse_string())
    }

    fn parse_store_path(&mut self, store_dir: &StoreDir) -> Result<StorePath, ATermError> {
        let path_str = self.parse_string()?;
        store_dir
            .parse::<StorePath>(&path_str)
            .map_err(|e| ATermError::InvalidStorePath(format!("{path_str}: {e}")))
    }

    fn parse_outputs(&mut self, store_dir: &StoreDir) -> Result<DerivationOutputs, ATermError> {
        let items = self.parse_list(|p| {
            p.expect_char('(')?;
            let id = p.parse_string()?;
            p.expect_char(',')?;
            let path_str = p.parse_string()?;
            p.expect_char(',')?;
            let method_str = p.parse_string()?;
            p.expect_char(',')?;
            let hash_str = p.parse_string()?;
            p.expect_char(')')?;
            let output =
                decode_output_fields(store_dir, &path_str, &method_str, &hash_str, |algo, s| {
                    Base16::<Hash>::parse(algo.algorithm(), s)
                })
                .map_err(ATermError::InvalidOutput)?;
            let output_name = id.parse().map_err(|e| {
                ATermError::InvalidOutput(format!("invalid output name '{id}': {e}"))
            })?;
            Ok((output_name, output))
        })?;
        Ok(items.into_iter().collect())
    }

    fn parse_input_drvs(
        &mut self,
        store_dir: &StoreDir,
    ) -> Result<BTreeMap<StorePath, BTreeSet<String>>, ATermError> {
        let items = self.parse_list(|p| {
            p.expect_char('(')?;
            let drv_path = p.parse_store_path(store_dir)?;
            p.expect_char(',')?;
            let outputs: BTreeSet<String> = p.parse_string_list()?.into_iter().collect();
            p.expect_char(')')?;
            Ok((drv_path, outputs))
        })?;
        Ok(items.into_iter().collect())
    }

    fn parse_input_srcs(
        &mut self,
        store_dir: &StoreDir,
    ) -> Result<BTreeSet<StorePath>, ATermError> {
        let items = self.parse_list(|p| p.parse_store_path(store_dir))?;
        Ok(items.into_iter().collect())
    }

    fn parse_env(
        &mut self,
    ) -> Result<(BTreeMap<String, String>, Option<StructuredAttrs>), ATermError> {
        let mut env = BTreeMap::new();
        let mut structured_attrs = None;

        let pairs = self.parse_list(|p| {
            p.expect_char('(')?;
            let key = p.parse_string()?;
            p.expect_char(',')?;
            let value = p.parse_string()?;
            p.expect_char(')')?;
            Ok((key, value))
        })?;

        for (key, value) in pairs {
            if key == "__json" {
                let attrs: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&value).map_err(|e| {
                        ATermError::InvalidStructuredAttrs(format!("failed to parse __json: {e}"))
                    })?;
                structured_attrs = Some(StructuredAttrs { attrs });
            } else {
                env.insert(key, value);
            }
        }

        Ok((env, structured_attrs))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use harmonia_store_core::ByteString;
    use harmonia_store_core::derivation::{BasicDerivation, DerivationOutput};
    use harmonia_store_core::store_path::{StoreDir, StorePath};
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn string_escaping_roundtrip() {
        let mut s = String::new();
        write_string(&mut s, "hello \"world\" \\\n\r\t");
        assert_eq!(s, r#""hello \"world\" \\\n\r\t""#);

        let mut p = Parser::new(&s);
        let parsed = p.parse_string().unwrap();
        assert_eq!(parsed, "hello \"world\" \\\n\r\t");
    }

    #[test]
    fn roundtrip_nix_instantiate_drv() {
        let drv_path = "/nix/store/57rrkadw446hysn5z32imn6ymckm616y-test.drv";
        let Ok(original) = std::fs::read_to_string(drv_path) else {
            eprintln!("skipping: {drv_path} not found (nix store not available)");
            return;
        };

        let store_dir = StoreDir::default();
        let drv = parse(&store_dir, &original, "test")
            .unwrap_or_else(|e| panic!("failed to parse {drv_path}: {e}"));

        let serialized = unparse(&store_dir, &drv, &BTreeMap::new());
        assert_eq!(serialized, original, "round-trip mismatch for {drv_path}");
    }

    #[test]
    fn roundtrip_with_input_drvs() {
        let store_dir = StoreDir::default();
        let original = concat!(
            r#"Derive([("out","/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-foo","","")]"#,
            r#",[("/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-bar.drv",["out"])]"#,
            r#",["/nix/store/cccccccccccccccccccccccccccccccc-src"]"#,
            r#","x86_64-linux","/bin/sh",[],[("name","foo")])"#,
        );

        let drv = parse(&store_dir, original, "foo").unwrap();

        let bar_path =
            StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-bar.drv").unwrap();
        let mut input_drvs = BTreeMap::new();
        input_drvs.insert(bar_path, BTreeSet::from(["out".to_string()]));

        let serialized = unparse(&store_dir, &drv, &input_drvs);
        assert_eq!(serialized, original);
    }

    fn arb_utf8_string() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                4 => "[a-zA-Z0-9 /._=-]".prop_map(|s| s.chars().next().unwrap()),
                1 => Just('"'),
                1 => Just('\\'),
                1 => Just('\n'),
                1 => Just('\t'),
            ],
            0..100,
        )
        .prop_map(|chars| chars.into_iter().collect())
    }

    fn arb_aterm_roundtrippable()
    -> impl Strategy<Value = (BasicDerivation, BTreeMap<StorePath, BTreeSet<String>>)> {
        (
            proptest::collection::btree_map(
                any::<harmonia_store_core::derived_path::OutputName>(),
                any::<StorePath>().prop_map(DerivationOutput::InputAddressed),
                1..4,
            ),
            proptest::collection::btree_map(
                any::<StorePath>(),
                proptest::collection::btree_set("[a-z]{2,6}".prop_map(String::from), 1..3),
                0..4,
            ),
            proptest::collection::btree_set(any::<StorePath>(), 0..4),
            arb_utf8_string(),
            arb_utf8_string(),
            proptest::collection::vec(arb_utf8_string(), 0..5),
            proptest::collection::btree_map(
                "[a-zA-Z_][a-zA-Z0-9_]{0,20}".prop_map(String::from),
                arb_utf8_string(),
                0..10,
            ),
        )
            .prop_map(
                |(outputs, input_drvs, input_srcs, platform, builder, args, env)| {
                    let mut inputs: BTreeSet<StorePath> = input_srcs;
                    for drv_path in input_drvs.keys() {
                        inputs.insert(drv_path.clone());
                    }
                    let drv = BasicDerivation {
                        name: "test".parse().unwrap(),
                        outputs,
                        inputs,
                        platform: ByteString::from(platform),
                        builder: ByteString::from(builder),
                        args: args.into_iter().map(ByteString::from).collect(),
                        env: env
                            .into_iter()
                            .map(|(k, v)| (ByteString::from(k), ByteString::from(v)))
                            .collect(),
                        structured_attrs: None,
                    };
                    (drv, input_drvs)
                },
            )
    }

    proptest! {
        #[test]
        fn proptest_aterm_roundtrip((drv, input_drvs) in arb_aterm_roundtrippable()) {
            let store_dir = StoreDir::default();
            let serialized = unparse(&store_dir, &drv, &input_drvs);
            let parsed = parse(&store_dir, &serialized, "test")
                .unwrap_or_else(|e| panic!("failed to parse serialized ATerm: {e}\nATerm: {serialized}"));
            let reserialized = unparse(&store_dir, &parsed, &input_drvs);
            prop_assert_eq!(&serialized, &reserialized);
        }

        #[test]
        fn proptest_string_escaping(s in arb_utf8_string()) {
            let mut buf = String::new();
            write_string(&mut buf, &s);
            let mut p = Parser::new(&buf);
            let parsed = p.parse_string().unwrap();
            prop_assert_eq!(s, parsed);
        }
    }
}
