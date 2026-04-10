use std::collections::{BTreeMap, BTreeSet};

use memchr::{memchr, memchr2};

use harmonia_store_core::ByteString;
use harmonia_store_core::derivation::{
    Derivation, DerivationOutput, DerivationOutputs, StructuredAttrs,
};
use harmonia_store_core::derivation::{DerivationInputs, OutputInputs};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::{
    ContentAddress, ContentAddressMethodAlgorithm, StoreDir, StorePath, StorePathName, StorePathSet,
};
use harmonia_utils_hash::Hash;
use harmonia_utils_hash::fmt::NonSRI;

use crate::ParseError;

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

/// Raw output fields before variant resolution. We need all outputs and the env
/// parsed before we can determine Impure vs CAFloating.
struct RawOutput {
    name: String,
    path: Vec<u8>,
    hash_algo: Vec<u8>,
    hash: Vec<u8>,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(input: &'a str, store_dir: &'a StoreDir) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
            store_dir,
        }
    }

    pub(crate) fn parse_derivation(
        &mut self,
        name: StorePathName,
    ) -> Result<Derivation, ParseError> {
        let version = self.parse_drv_header()?;

        let raw_outputs = self.parse_raw_outputs()?;
        self.expect_char(',')?;

        let input_drvs = self.parse_input_drvs(version)?;
        self.expect_char(',')?;

        let input_srcs = self.parse_input_srcs()?;
        self.expect_char(',')?;

        let platform = ByteString::from(self.parse_string()?);
        self.expect_char(',')?;

        let builder = ByteString::from(self.parse_string()?);
        self.expect_char(',')?;

        let args: Vec<ByteString> = self
            .parse_string_list()?
            .into_iter()
            .map(ByteString::from)
            .collect();
        self.expect_char(',')?;

        let mut env = self.parse_env()?;

        self.expect_char(')')?;

        let outputs = resolve_outputs(raw_outputs, self.store_dir)?;

        // Build DerivationInputs and convert to BTreeSet<SingleDerivedPath>
        let inputs_struct = DerivationInputs {
            srcs: input_srcs,
            drvs: input_drvs,
        };
        let inputs = BTreeSet::from(&inputs_struct);

        // Extract structured attrs from __json env var (matches cppnix behavior:
        // the ATerm format stores structured attrs as a JSON string in __json,
        // but the Derivation type represents them as a separate field).
        let structured_attrs =
            env.remove(&ByteString::from_static(b"__json"))
                .and_then(|json_bytes| {
                    let json_str = std::str::from_utf8(&json_bytes).ok()?;
                    let attrs: serde_json::Map<String, serde_json::Value> =
                        serde_json::from_str(json_str).ok()?;
                    Some(StructuredAttrs { attrs })
                });

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

    fn parse_raw_outputs(&mut self) -> Result<Vec<RawOutput>, ParseError> {
        self.expect_char('[')?;
        let mut outputs = Vec::new();

        while self.peek() != Some(b']') {
            self.expect_char('(')?;
            let name_bytes = self.parse_string()?;
            let name = String::from_utf8(name_bytes)
                .map_err(|_| ParseError::InvalidUtf8 { pos: self.pos })?;
            self.expect_char(',')?;
            let path = self.parse_string()?;
            self.expect_char(',')?;
            let hash_algo = self.parse_string()?;
            self.expect_char(',')?;
            let hash = self.parse_string()?;
            self.expect_char(')')?;

            outputs.push(RawOutput {
                name,
                path,
                hash_algo,
                hash,
            });

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
    ) -> Result<BTreeMap<StorePath, OutputInputs>, ParseError> {
        self.expect_char('[')?;
        let mut drvs = BTreeMap::new();

        while self.peek() != Some(b']') {
            self.expect_char('(')?;
            let path = self.parse_store_path()?;
            self.expect_char(',')?;

            let output_inputs = self.parse_output_inputs(version)?;
            self.expect_char(')')?;

            drvs.insert(path, output_inputs);

            if self.peek() == Some(b',') {
                self.advance();
            }
        }

        self.expect_char(']')?;
        Ok(drvs)
    }

    fn parse_output_inputs(&mut self, version: ATermVersion) -> Result<OutputInputs, ParseError> {
        match version {
            ATermVersion::Traditional => {
                let outputs = self.parse_output_names()?;
                Ok(OutputInputs {
                    outputs,
                    dynamic_outputs: BTreeMap::new(),
                })
            }
            ATermVersion::DynamicDerivations => match self.peek() {
                Some(b'[') => {
                    let outputs = self.parse_output_names()?;
                    Ok(OutputInputs {
                        outputs,
                        dynamic_outputs: BTreeMap::new(),
                    })
                }
                Some(b'(') => {
                    self.expect_char('(')?;
                    let outputs = self.parse_output_names()?;
                    self.expect_char(',')?;
                    self.expect_char('[')?;
                    let mut dynamic_outputs = BTreeMap::new();
                    while self.peek() != Some(b']') {
                        self.expect_char('(')?;
                        let name_bytes = self.parse_string()?;
                        let name_str = std::str::from_utf8(&name_bytes)
                            .map_err(|_| ParseError::InvalidUtf8 { pos: self.pos })?;
                        let output_name: OutputName = name_str.parse()?;
                        self.expect_char(',')?;
                        let child = self.parse_output_inputs(version)?;
                        self.expect_char(')')?;
                        dynamic_outputs.insert(output_name, child);
                        if self.peek() == Some(b',') {
                            self.advance();
                        }
                    }
                    self.expect_char(']')?;
                    self.expect_char(')')?;
                    Ok(OutputInputs {
                        outputs,
                        dynamic_outputs,
                    })
                }
                _ => Err(ParseError::UnexpectedEof {
                    expected: "[ or (",
                    pos: self.pos,
                }),
            },
        }
    }

    fn parse_output_names(&mut self) -> Result<BTreeSet<OutputName>, ParseError> {
        self.expect_char('[')?;
        let mut output_names = BTreeSet::new();
        while self.peek() != Some(b']') {
            let name_bytes = self.parse_string()?;
            let name_str = std::str::from_utf8(&name_bytes)
                .map_err(|_| ParseError::InvalidUtf8 { pos: self.pos })?;
            output_names.insert(name_str.parse::<OutputName>()?);
            if self.peek() == Some(b',') {
                self.advance();
            }
        }
        self.expect_char(']')?;
        Ok(output_names)
    }

    fn parse_input_srcs(&mut self) -> Result<StorePathSet, ParseError> {
        self.expect_char('[')?;
        let mut srcs = StorePathSet::new();

        while self.peek() != Some(b']') {
            srcs.insert(self.parse_store_path()?);
            if self.peek() == Some(b',') {
                self.advance();
            }
        }

        self.expect_char(']')?;
        Ok(srcs)
    }

    fn parse_env(&mut self) -> Result<BTreeMap<ByteString, ByteString>, ParseError> {
        self.expect_char('[')?;
        let mut env = BTreeMap::new();

        while self.peek() != Some(b']') {
            self.expect_char('(')?;
            let key = ByteString::from(self.parse_string()?);
            self.expect_char(',')?;
            let value = ByteString::from(self.parse_string()?);
            self.expect_char(')')?;

            env.insert(key, value);

            if self.peek() == Some(b',') {
                self.advance();
            }
        }

        self.expect_char(']')?;
        Ok(env)
    }

    fn parse_store_path(&mut self) -> Result<StorePath, ParseError> {
        let pos = self.pos;
        let bytes = self.parse_string()?;
        let s = std::str::from_utf8(&bytes).map_err(|_| ParseError::InvalidUtf8 { pos })?;
        let base_name = self
            .store_dir
            .strip_prefix(s)
            .map_err(|e| ParseError::store_path_error(pos, s, e))?;
        base_name
            .parse::<StorePath>()
            .map_err(|e| ParseError::StorePath { pos, source: e })
    }

    fn parse_string(&mut self) -> Result<Vec<u8>, ParseError> {
        self.expect_char('"')?;

        let start = self.pos;

        // Find closing quote
        let end_offset = memchr(b'"', &self.bytes[start..])
            .ok_or(ParseError::UnterminatedString { pos: start })?;

        // Fast path: no escapes in the string
        if memchr(b'\\', &self.bytes[start..start + end_offset]).is_none() {
            let result = self.bytes[start..start + end_offset].to_vec();
            self.pos = start + end_offset + 1; // skip closing quote
            return Ok(result);
        }

        // Slow path: handle escape sequences
        let mut result = Vec::with_capacity(end_offset);
        let mut cur = self.pos;

        loop {
            match memchr2(b'"', b'\\', &self.bytes[cur..]) {
                Some(offset) => {
                    if offset > 0 {
                        result.extend_from_slice(&self.bytes[cur..cur + offset]);
                    }
                    cur += offset;

                    if self.bytes[cur] == b'"' {
                        self.pos = cur + 1;
                        return Ok(result);
                    }

                    // Backslash escape
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
                }
                None => return Err(ParseError::UnterminatedString { pos: start }),
            }
        }
    }

    fn parse_string_list(&mut self) -> Result<Vec<Vec<u8>>, ParseError> {
        self.expect_char('[')?;
        let mut items = Vec::new();

        while self.peek() != Some(b']') {
            items.push(self.parse_string()?);
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

fn resolve_outputs(
    raw: Vec<RawOutput>,
    store_dir: &StoreDir,
) -> Result<DerivationOutputs, ParseError> {
    let mut outputs = DerivationOutputs::new();

    for r in raw {
        let output_name: OutputName = r.name.parse()?;

        let variant = if !r.hash_algo.is_empty() && r.hash == b"impure" {
            // Impure: hashAlgo present, hash is literal "impure"
            let algo_str = std::str::from_utf8(&r.hash_algo)
                .map_err(|_| ParseError::InvalidUtf8 { pos: 0 })?;
            let cama: ContentAddressMethodAlgorithm = algo_str.parse()?;
            DerivationOutput::Impure(cama)
        } else if !r.hash_algo.is_empty() && !r.hash.is_empty() {
            // CAFixed: both hashAlgo and hash present (hash is actual hash value)
            let algo_str = std::str::from_utf8(&r.hash_algo)
                .map_err(|_| ParseError::InvalidUtf8 { pos: 0 })?;
            let cama: ContentAddressMethodAlgorithm = algo_str.parse()?;
            let hash_str =
                std::str::from_utf8(&r.hash).map_err(|_| ParseError::InvalidUtf8 { pos: 0 })?;
            let hash: Hash = NonSRI::<Hash>::parse(cama.algorithm(), hash_str)
                .map_err(|e| ParseError::Hash(e.to_string()))?;
            let ca = ContentAddress::from_hash(cama.method(), hash)
                .map_err(|e| ParseError::Hash(e.to_string()))?;
            DerivationOutput::CAFixed(ca)
        } else if !r.hash_algo.is_empty() {
            // hashAlgo present, hash empty → CAFloating
            let algo_str = std::str::from_utf8(&r.hash_algo)
                .map_err(|_| ParseError::InvalidUtf8 { pos: 0 })?;
            let cama: ContentAddressMethodAlgorithm = algo_str.parse()?;
            DerivationOutput::CAFloating(cama)
        }
        // path present, hashAlgo and hash empty → InputAddressed
        else if !r.path.is_empty() {
            let path_str =
                std::str::from_utf8(&r.path).map_err(|_| ParseError::InvalidUtf8 { pos: 0 })?;
            let base_name = store_dir
                .strip_prefix(path_str)
                .map_err(|e| ParseError::store_path_error(0, path_str, e))?;
            let path = base_name
                .parse::<StorePath>()
                .map_err(|e| ParseError::StorePath { pos: 0, source: e })?;
            DerivationOutput::InputAddressed(path)
        } else {
            DerivationOutput::Deferred
        };

        outputs.insert(output_name, variant);
    }

    Ok(outputs)
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
        assert!(crate::parse_derivation_aterm(&store_dir, input, "test".parse().unwrap()).is_err());
    }
}
