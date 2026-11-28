use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use memchr::memchr2;

use bytes::Bytes;
use harmonia_store_derivation::derivation::{
    Derivation, DerivationOutput, DerivationOutputs, StructuredAttrs,
};
use harmonia_store_derivation::derived_path::{OutputName, SingleDerivedPath};
use harmonia_store_path::{StoreDir, StorePath, StorePathName};

use crate::ParseError;
use crate::raw_output::{AtermOutput as _, BorrowedRawOutput};

/// Maximum nesting depth for dynamic derivation outputs (`DrvWithVersion`).
/// Prevents stack overflow on deeply nested inputs.
const MAX_DYNAMIC_OUTPUT_DEPTH: usize = 128;

/// ATerm derivation format version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ATermVersion {
    /// Traditional unversioned form.
    Traditional,
    /// Supports dynamic derivation inputs.
    DynamicDerivations,
}

pub(crate) struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    store_dir: &'a StoreDir,
}

fn cow_to_bytes(cow: Cow<'_, [u8]>) -> Bytes {
    match cow {
        Cow::Owned(v) => Bytes::from(v),
        Cow::Borrowed(s) => Bytes::copy_from_slice(s),
    }
}

impl<'a> Parser<'a> {
    pub(crate) fn new(input: &'a [u8], store_dir: &'a StoreDir) -> Self {
        Self {
            bytes: input,
            pos: 0,
            store_dir,
        }
    }

    pub(crate) fn parse_derivation(
        &mut self,
        name: StorePathName,
    ) -> Result<Derivation, ParseError> {
        let version = self.parse_drv_header()?;

        let outputs = self.parse_outputs(&name)?;
        self.expect_char(',')?;

        let mut inputs = BTreeSet::new();
        self.parse_input_drvs(version, &mut inputs)?;
        self.expect_char(',')?;

        self.parse_input_srcs(&mut inputs)?;
        self.expect_char(',')?;

        let platform = cow_to_bytes(self.parse_string()?);
        self.expect_char(',')?;

        let builder = cow_to_bytes(self.parse_string()?);
        self.expect_char(',')?;

        let args = self.parse_bytes_list()?;
        self.expect_char(',')?;

        let mut env = self.parse_env()?;

        self.expect_char(')')?;

        // Extract structured attrs from __json env var (matches C++ Nix's
        // StructuredAttrs::tryExtract: fails on invalid UTF-8 or non-object
        // JSON and preserves the raw encoding verbatim).
        let structured_attrs = StructuredAttrs::try_extract(&mut env)?;

        Ok(Derivation {
            name,
            outputs,
            inputs,
            platform,
            builder,
            args,
            env,
            structured_attrs,
        })
    }

    fn parse_drv_header(&mut self) -> Result<ATermVersion, ParseError> {
        if self.bytes[self.pos..].starts_with(b"Derive(") {
            self.pos += b"Derive(".len();
            Ok(ATermVersion::Traditional)
        } else if self.bytes[self.pos..].starts_with(b"DrvWithVersion(") {
            self.pos += b"DrvWithVersion(".len();
            let version_bytes = self.parse_string()?;
            let version_str = std::str::from_utf8(&version_bytes)
                .map_err(|_| ParseError::InvalidUtf8 { pos: self.pos })?;
            if version_str != "xp-dyn-drv" {
                return Err(ParseError::UnexpectedEof {
                    expected: "xp-dyn-drv",
                    pos: self.pos,
                });
            }
            self.expect_char(',')?;
            Ok(ATermVersion::DynamicDerivations)
        } else {
            Err(ParseError::UnexpectedEof {
                expected: "Derive(",
                pos: self.pos,
            })
        }
    }

    fn parse_outputs(&mut self, drv_name: &StorePathName) -> Result<DerivationOutputs, ParseError> {
        self.expect_char('[')?;
        let mut outputs = DerivationOutputs::new();

        while self.peek() != Some(b']') {
            self.expect_char('(')?;
            let name_bytes = self.parse_string()?;
            let name: OutputName = std::str::from_utf8(&name_bytes)
                .map_err(|_| ParseError::InvalidUtf8 { pos: self.pos })?
                .parse()?;
            self.expect_char(',')?;
            let path = self.parse_string()?;
            self.expect_char(',')?;
            let hash_algo = self.parse_string()?;
            self.expect_char(',')?;
            let hash = self.parse_string()?;
            self.expect_char(')')?;

            let raw = BorrowedRawOutput {
                path: &path,
                hash_algo: &hash_algo,
                hash: &hash,
            };
            let output = DerivationOutput::from_raw(raw, self.store_dir, drv_name, &name)?;
            outputs.insert(name, output);

            if self.peek() == Some(b',') {
                self.advance();
            }
        }

        self.expect_char(']')?;
        Ok(outputs)
    }

    fn parse_input_drvs(
        &mut self,
        version: ATermVersion,
        out: &mut BTreeSet<SingleDerivedPath>,
    ) -> Result<(), ParseError> {
        self.expect_char('[')?;

        let mut keys = SeenKeys::default();
        while self.peek() != Some(b']') {
            self.expect_char('(')?;
            let path = self.parse_store_path()?;
            self.expect_char(',')?;

            let drv_path = Arc::new(SingleDerivedPath::Opaque(path));
            if !keys.insert(&drv_path) {
                return Err(ParseError::DuplicateInputDrv {
                    key: format!("input derivation '{drv_path}'"),
                    pos: self.pos,
                });
            }
            self.parse_output_inputs(version, &drv_path, out, 0)?;
            self.expect_char(')')?;

            if self.peek() == Some(b',') {
                self.advance();
            }
        }

        self.expect_char(']')?;
        Ok(())
    }

    fn parse_output_inputs(
        &mut self,
        version: ATermVersion,
        drv_path: &Arc<SingleDerivedPath>,
        out: &mut BTreeSet<SingleDerivedPath>,
        depth: usize,
    ) -> Result<(), ParseError> {
        match version {
            ATermVersion::Traditional => self.parse_output_names(drv_path, out),
            ATermVersion::DynamicDerivations => match self.peek() {
                Some(b'[') => self.parse_output_names(drv_path, out),
                Some(b'(') => {
                    if depth >= MAX_DYNAMIC_OUTPUT_DEPTH {
                        return Err(ParseError::MaxDepthExceeded {
                            max_depth: MAX_DYNAMIC_OUTPUT_DEPTH,
                            pos: self.pos,
                        });
                    }
                    self.expect_char('(')?;
                    self.parse_output_names(drv_path, out)?;
                    self.expect_char(',')?;
                    self.expect_char('[')?;
                    let mut keys = SeenKeys::default();
                    while self.peek() != Some(b']') {
                        self.expect_char('(')?;
                        let name_bytes = self.parse_string()?;
                        let name_str = std::str::from_utf8(&name_bytes)
                            .map_err(|_| ParseError::InvalidUtf8 { pos: self.pos })?;
                        let output_name: OutputName = name_str.parse()?;
                        self.expect_char(',')?;
                        let nested_drv = Arc::new(SingleDerivedPath::Built {
                            drv_path: drv_path.clone(),
                            output: output_name,
                        });
                        if !keys.insert(&nested_drv) {
                            return Err(ParseError::DuplicateInputDrv {
                                key: format!("dynamic output '{nested_drv}'"),
                                pos: self.pos,
                            });
                        }
                        self.parse_output_inputs(version, &nested_drv, out, depth + 1)?;
                        self.expect_char(')')?;
                        if self.peek() == Some(b',') {
                            self.advance();
                        }
                    }
                    self.expect_char(']')?;
                    self.expect_char(')')?;
                    Ok(())
                }
                _ => Err(ParseError::UnexpectedEof {
                    expected: "[ or (",
                    pos: self.pos,
                }),
            },
        }
    }

    fn parse_output_names(
        &mut self,
        drv_path: &Arc<SingleDerivedPath>,
        out: &mut BTreeSet<SingleDerivedPath>,
    ) -> Result<(), ParseError> {
        self.expect_char('[')?;
        while self.peek() != Some(b']') {
            let name_bytes = self.parse_string()?;
            let name_str = std::str::from_utf8(&name_bytes)
                .map_err(|_| ParseError::InvalidUtf8 { pos: self.pos })?;
            out.insert(SingleDerivedPath::Built {
                drv_path: drv_path.clone(),
                output: name_str.parse::<OutputName>()?,
            });
            if self.peek() == Some(b',') {
                self.advance();
            }
        }
        self.expect_char(']')?;
        Ok(())
    }

    fn parse_input_srcs(
        &mut self,
        out: &mut BTreeSet<SingleDerivedPath>,
    ) -> Result<(), ParseError> {
        self.expect_char('[')?;

        while self.peek() != Some(b']') {
            out.insert(SingleDerivedPath::Opaque(self.parse_store_path()?));
            if self.peek() == Some(b',') {
                self.advance();
            }
        }

        self.expect_char(']')?;
        Ok(())
    }

    fn parse_env(&mut self) -> Result<BTreeMap<Bytes, Bytes>, ParseError> {
        // Pack small unescaped strings into a single shared backing buffer to avoid
        // per-entry heap allocations. Values larger than `MAX_SLAB_ENTRY_LEN` and
        // `__json` are allocated independently so `StructuredAttrs::try_extract`
        // stays zero-copy and individual oversized values are not retained by the
        // shared buffer.
        const MAX_SLAB_ENTRY_LEN: usize = 4096;

        enum EnvAtom<'a> {
            Slab(&'a [u8]),
            Standalone(Cow<'a, [u8]>),
        }

        let classify =
            |cow: Cow<'a, [u8]>, allow_slab: bool, slab_len: &mut usize| -> EnvAtom<'a> {
                match cow {
                    Cow::Borrowed(s)
                        if allow_slab && !s.is_empty() && s.len() <= MAX_SLAB_ENTRY_LEN =>
                    {
                        *slab_len += s.len();
                        EnvAtom::Slab(s)
                    }
                    other => EnvAtom::Standalone(other),
                }
            };

        self.expect_char('[')?;
        if self.peek() == Some(b']') {
            self.advance();
            return Ok(BTreeMap::new());
        }

        // `env` is the final ATerm field, so `remaining` bounds its encoded size.
        let remaining = self.bytes.len().saturating_sub(self.pos);
        let mut pairs: Vec<(EnvAtom<'a>, EnvAtom<'a>)> =
            Vec::with_capacity((remaining / 64).clamp(8, 512));
        let mut slab_len = 0usize;

        while self.peek() != Some(b']') {
            self.expect_char('(')?;
            let key_cow = self.parse_string()?;
            let allow_val_slab = key_cow.as_ref() != StructuredAttrs::ENV_VAR;
            let key = classify(key_cow, true, &mut slab_len);
            self.expect_char(',')?;
            let val = classify(self.parse_string()?, allow_val_slab, &mut slab_len);
            self.expect_char(')')?;

            pairs.push((key, val));

            if self.peek() == Some(b',') {
                self.advance();
            }
        }

        self.expect_char(']')?;

        let mut slab = Vec::with_capacity(slab_len);
        for (k, v) in &pairs {
            if let EnvAtom::Slab(s) = k {
                slab.extend_from_slice(s);
            }
            if let EnvAtom::Slab(s) = v {
                slab.extend_from_slice(s);
            }
        }
        let mut backing = Bytes::from(slab);
        let mut resolve = |atom: EnvAtom<'a>| -> Bytes {
            match atom {
                EnvAtom::Slab(s) => backing.split_to(s.len()),
                EnvAtom::Standalone(cow) => cow_to_bytes(cow),
            }
        };

        let env = pairs
            .into_iter()
            .map(|(k, v)| (resolve(k), resolve(v)))
            .collect();
        debug_assert!(backing.is_empty());
        Ok(env)
    }

    fn parse_store_path(&mut self) -> Result<StorePath, ParseError> {
        let pos = self.pos;
        let bytes = self.parse_string()?;
        let s = std::str::from_utf8(&bytes).map_err(|_| ParseError::InvalidUtf8 { pos })?;
        self.store_dir
            .parse(s)
            .map_err(|e| ParseError::StorePath { pos, source: e })
    }

    fn parse_string(&mut self) -> Result<Cow<'a, [u8]>, ParseError> {
        self.expect_char('"')?;

        let start = self.pos;

        // Single-pass SIMD scan for either closing quote or first escape.
        let first = memchr2(b'"', b'\\', &self.bytes[start..])
            .ok_or(ParseError::UnterminatedString { pos: start })?;

        // Fast path: no escapes before closing quote — borrow directly from input.
        if self.bytes[start + first] == b'"' {
            let result = &self.bytes[start..start + first];
            self.pos = start + first + 1;
            return Ok(Cow::Borrowed(result));
        }

        // Slow path: handle escape sequences starting at `start + first`.
        let mut result = Vec::with_capacity(first + 32);
        if first > 0 {
            result.extend_from_slice(&self.bytes[start..start + first]);
        }
        let mut cur = start + first;

        loop {
            // `cur` currently points at a backslash.
            cur += 1;
            if cur >= self.bytes.len() {
                return Err(ParseError::UnterminatedString { pos: start });
            }
            match self.bytes[cur] {
                b'n' => result.push(b'\n'),
                b't' => result.push(b'\t'),
                b'r' => result.push(b'\r'),
                b'\\' => result.push(b'\\'),
                b'"' => result.push(b'"'),
                other => result.push(other),
            }
            cur += 1;

            match memchr2(b'"', b'\\', &self.bytes[cur..]) {
                Some(offset) => {
                    if offset > 0 {
                        result.extend_from_slice(&self.bytes[cur..cur + offset]);
                    }
                    cur += offset;
                    if self.bytes[cur] == b'"' {
                        self.pos = cur + 1;
                        return Ok(Cow::Owned(result));
                    }
                }
                None => return Err(ParseError::UnterminatedString { pos: start }),
            }
        }
    }

    fn parse_bytes_list(&mut self) -> Result<Vec<Bytes>, ParseError> {
        self.expect_char('[')?;
        let mut items = Vec::new();

        while self.peek() != Some(b']') {
            items.push(cow_to_bytes(self.parse_string()?));
            if self.peek() == Some(b',') {
                self.advance();
            }
        }

        self.expect_char(']')?;
        Ok(items)
    }

    fn expect_char(&mut self, expected: char) -> Result<(), ParseError> {
        let expected_byte = expected as u8;
        if self.pos < self.bytes.len() && self.bytes[self.pos] == expected_byte {
            self.pos += 1;
            return Ok(());
        }

        match self.peek() {
            Some(found) => Err(ParseError::UnexpectedChar {
                expected,
                found: found as char,
                pos: self.pos,
            }),
            None => Err(ParseError::UnexpectedEof {
                expected: "character",
                pos: self.pos,
            }),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn advance(&mut self) {
        if self.pos < self.bytes.len() {
            self.pos += 1;
        }
    }
}

/// The keys seen so far at one `inputDrvs` map level, for rejecting repeats.
///
/// Entries of all levels are flattened into one `BTreeSet`, so a repeated key
/// would silently merge its outputs with the earlier entry's. Nix rejects
/// repeated keys instead. Nix writes keys in strictly ascending order, and
/// while they keep ascending a repeat is impossible, so a key costs one
/// comparison. Out-of-order input, which is still accepted, switches to a set
/// of every key at this level.
#[derive(Default)]
struct SeenKeys {
    /// Every key so far, while they have arrived in strictly ascending order.
    ascending: Vec<Arc<SingleDerivedPath>>,
    /// Every key so far, once one arrived out of order.
    unordered: Option<BTreeSet<Arc<SingleDerivedPath>>>,
}

impl SeenKeys {
    /// Record `key`, returning `false` if this level already had it.
    fn insert(&mut self, key: &Arc<SingleDerivedPath>) -> bool {
        if let Some(seen) = &mut self.unordered {
            return seen.insert(key.clone());
        }
        if self.ascending.last().is_some_and(|last| **last >= **key) {
            let mut seen: BTreeSet<_> = self.ascending.drain(..).collect();
            let fresh = seen.insert(key.clone());
            self.unordered = Some(seen);
            return fresh;
        }
        self.ascending.push(key.clone());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::empty("")]
    #[case::truncated("Derive([")]
    #[case::bad_char("Derive(x")]
    fn parse_error_cases(#[case] input: &str) {
        let store_dir = StoreDir::default();
        assert!(
            crate::parse_derivation_aterm(&store_dir, input.as_bytes(), "test".parse().unwrap())
                .is_err()
        );
    }

    const A: &str = "/nix/store/11111111111111111111111111111111-a.drv";
    const B: &str = "/nix/store/22222222222222222222222222222222-b.drv";

    fn drv_with_inputs(header: &str, input_drvs: &str) -> Vec<u8> {
        format!(
            "{header}[(\"out\",\"/nix/store/00000000000000000000000000000000-test\",\"\",\"\")],\
             [{input_drvs}],[],\"x86_64-linux\",\"/bin/sh\",[],[])"
        )
        .into_bytes()
    }

    fn parse(input: &[u8]) -> Result<Derivation, ParseError> {
        crate::parse_derivation_aterm(&StoreDir::default(), input, "test".parse().unwrap())
    }

    /// Nix rejects a repeated `inputDrvs` key at every level. Flattening into
    /// one set must not silently merge the two entries' outputs instead.
    #[rstest]
    #[case::adjacent("Derive(", &format!("(\"{A}\",[\"dev\"]),(\"{A}\",[\"out\"])"))]
    #[case::after_unsorted(
        "Derive(",
        &format!("(\"{B}\",[\"out\"]),(\"{A}\",[\"out\"]),(\"{B}\",[\"dev\"])")
    )]
    #[case::dynamic_child(
        "DrvWithVersion(\"xp-dyn-drv\",",
        &format!("(\"{A}\",([],[(\"out\",[\"a\"]),(\"out\",[\"b\"])]))")
    )]
    #[case::first_without_outputs("Derive(", &format!("(\"{A}\",[]),(\"{A}\",[\"out\"])"))]
    #[case::unsorted_first_without_outputs(
        "Derive(",
        &format!("(\"{B}\",[]),(\"{A}\",[\"out\"]),(\"{B}\",[\"out\"])")
    )]
    #[case::dynamic_child_without_outputs(
        "DrvWithVersion(\"xp-dyn-drv\",",
        &format!("(\"{A}\",([],[(\"out\",[]),(\"out\",[\"b\"])]))")
    )]
    fn duplicate_input_drv_rejected(#[case] header: &str, #[case] input_drvs: &str) {
        let err = parse(&drv_with_inputs(header, input_drvs)).unwrap_err();
        assert!(
            matches!(err, ParseError::DuplicateInputDrv { .. }),
            "unexpected error: {err}"
        );
    }

    /// Keys that are merely out of order are still accepted, as before.
    #[test]
    fn unsorted_input_drvs_accepted() {
        let sorted = parse(&drv_with_inputs(
            "Derive(",
            &format!("(\"{A}\",[\"out\"]),(\"{B}\",[\"dev\",\"out\"])"),
        ))
        .unwrap();
        let unsorted = parse(&drv_with_inputs(
            "Derive(",
            &format!("(\"{B}\",[\"out\",\"dev\"]),(\"{A}\",[\"out\"])"),
        ))
        .unwrap();
        assert_eq!(sorted.inputs.len(), 3);
        assert_eq!(sorted, unsorted);
    }

    /// Many distinct keys in descending order are accepted, and checking them
    /// stays fast.
    #[test]
    fn many_descending_input_drvs_accepted() {
        const N: usize = 20_000;
        // Nix base32 digits in ascending order.
        const DIGITS: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";
        let input_drvs: Vec<String> = (0..N)
            .rev()
            .map(|i| {
                let mut hash = [b'0'; 32];
                let mut i = i;
                for digit in hash.iter_mut().rev() {
                    *digit = DIGITS[i % 32];
                    i /= 32;
                }
                let hash = std::str::from_utf8(&hash).unwrap();
                format!("(\"/nix/store/{hash}-d.drv\",[\"out\"])")
            })
            .collect();
        let drv = parse(&drv_with_inputs("Derive(", &input_drvs.join(","))).unwrap();
        assert_eq!(drv.inputs.len(), N);
    }

    #[rstest]
    #[case::float_exp_pos(r#"{"x":1e+15}"#)]
    #[case::float_exp_neg(r#"{"x":1e-05}"#)]
    #[case::whitespace(r#"{ "b": 2, "a": 1 }"#)]
    #[case::uppercase_unicode(r#"{"s":"\u001B"}"#)]
    fn structured_attrs_verbatim_roundtrip_and_hash_modulo(#[case] raw_json: &str) {
        let store_dir = StoreDir::default();
        let name: StorePathName = "test".parse().unwrap();
        let escaped = raw_json.replace('\\', "\\\\").replace('"', "\\\"");
        let drv_str = format!(
            r#"Derive([("out","/nix/store/00000000000000000000000000000000-test","","")],[],[],"x86_64-linux","/bin/sh",[],[("__json","{escaped}"),("out","/nix/store/00000000000000000000000000000000-test")])"#
        );
        let parsed =
            crate::parse_derivation_aterm(&store_dir, drv_str.as_bytes(), name.clone()).unwrap();
        assert_eq!(
            parsed.structured_attrs.as_ref().unwrap().verbatim(),
            Some(raw_json)
        );

        let printed = crate::print_derivation_aterm(&store_dir, &parsed.clone().into_full());
        assert_eq!(printed, drv_str.as_bytes());

        let actual_hash = crate::hash_modulo::hash_modulo(
            &store_dir,
            &parsed,
            |_| -> Result<_, std::convert::Infallible> { unreachable!() },
        )
        .unwrap()
        .unwrap();
        let expected_masked = format!(
            r#"Derive([("out","","","")],[],[],"x86_64-linux","/bin/sh",[],[("__json","{escaped}"),("out","")])"#
        );
        assert_eq!(
            actual_hash,
            harmonia_utils_hash::Sha256::digest(expected_masked.as_bytes())
        );
    }

    #[rstest]
    #[case::invalid_json(
        br#"Derive([],[],[],"x86_64-linux","/bin/sh",[],[("__json","not-valid-json")])"#
    )]
    #[case::json_array(br#"Derive([],[],[],"x86_64-linux","/bin/sh",[],[("__json","[1,2]")])"#)]
    #[case::json_number(br#"Derive([],[],[],"x86_64-linux","/bin/sh",[],[("__json","42")])"#)]
    #[case::invalid_utf8(
        b"Derive([],[],[],\"x86_64-linux\",\"/bin/sh\",[],[(\"__json\",\"\xff\")])"
    )]
    fn rejects_malformed_structured_attrs(#[case] input: &[u8]) {
        let store_dir = StoreDir::default();
        assert!(crate::parse_derivation_aterm(&store_dir, input, "test".parse().unwrap()).is_err());
    }

    fn nested_dynamic_drv(depth: usize) -> String {
        let mut s = String::from(
            r#"DrvWithVersion("xp-dyn-drv",[],[("/nix/store/00000000000000000000000000000000-a.drv","#,
        );
        for _ in 0..depth {
            s.push_str(r#"([],[("a","#);
        }
        s.push_str(r#"["out"]"#);
        for _ in 0..depth {
            s.push_str(")])");
        }
        s.push_str(r#")],[],"x86_64-linux","/bin/sh",[],[])"#);
        s
    }

    #[test]
    fn dynamic_output_depth_at_limit_roundtrips() {
        let input = nested_dynamic_drv(MAX_DYNAMIC_OUTPUT_DEPTH);
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let store_dir = StoreDir::default();
                let drv = crate::parse_derivation_aterm(
                    &store_dir,
                    input.as_bytes(),
                    "test".parse().unwrap(),
                )
                .expect("nesting at MAX_DYNAMIC_OUTPUT_DEPTH should succeed");
                let printed = crate::print_derivation_aterm(&store_dir, &drv.into_full());
                assert_eq!(printed, input.as_bytes());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[rstest]
    #[case::one_over_limit(MAX_DYNAMIC_OUTPUT_DEPTH + 1)]
    #[case::deep_hostile_input(10_000)]
    fn dynamic_output_depth_exceeding_limit_errors(#[case] depth: usize) {
        let input = nested_dynamic_drv(depth);
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let store_dir = StoreDir::default();
                let err = crate::parse_derivation_aterm(
                    &store_dir,
                    input.as_bytes(),
                    "test".parse().unwrap(),
                )
                .unwrap_err();
                assert!(
                    matches!(
                        err,
                        ParseError::MaxDepthExceeded {
                            max_depth: MAX_DYNAMIC_OUTPUT_DEPTH,
                            ..
                        }
                    ),
                    "unexpected error: {err}"
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn parse_env_slab_edge_cases() {
        let store_dir = StoreDir::default();
        let name: StorePathName = "test".parse().unwrap();

        // Empty env list.
        let empty = br#"Derive([],[],[],"x86_64-linux","/bin/sh",[],[])"#;
        let drv = crate::parse_derivation_aterm(&store_dir, empty, name.clone()).unwrap();
        assert!(drv.env.is_empty());

        // Mix of empty, small borrowed, escaped, duplicate (last wins), >4 KiB, and __json.
        let big = "b".repeat(5000);
        let input = format!(
            concat!(
                r#"Derive([],[],[],"x86_64-linux","/bin/sh",[],"#,
                r#"[("__json","{{\"k\":1}}"),("big","{}"),"#,
                r#"("dup","first"),("dup","second"),"#,
                r#"("empty",""),("esc","line1
line2"),("small","val")])"#,
            ),
            big
        );
        let drv = crate::parse_derivation_aterm(&store_dir, input.as_bytes(), name).unwrap();
        assert_eq!(
            drv.structured_attrs.as_ref().unwrap().verbatim(),
            Some(r#"{"k":1}"#)
        );
        assert!(!drv.env.contains_key(StructuredAttrs::ENV_VAR));
        assert_eq!(drv.env[b"big".as_slice()].as_ref(), big.as_bytes());
        assert_eq!(drv.env[b"dup".as_slice()].as_ref(), b"second");
        assert_eq!(drv.env[b"empty".as_slice()].as_ref(), b"");
        assert_eq!(
            drv.env[b"esc".as_slice()].as_ref(),
            b"line1
line2"
        );
        assert_eq!(drv.env[b"small".as_slice()].as_ref(), b"val");
    }
}
