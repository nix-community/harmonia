use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use memchr::{memchr, memchr2};

use bytes::Bytes;
use harmonia_store_derivation::derivation::{
    Derivation, DerivationOutput, DerivationOutputs, StructuredAttrs,
};
use harmonia_store_derivation::derivation::{DerivationInputs, OutputInputs};
use harmonia_store_derivation::derived_path::OutputName;
use harmonia_store_path::{StoreDir, StorePath, StorePathName, StorePathSet};

use harmonia_utils_base_encoding::Base;

use crate::ParseError;
use crate::raw_output::{AtermOutput as _, BorrowedRawOutput};

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

        let input_drvs = self.parse_input_drvs(version)?;
        self.expect_char(',')?;

        let input_srcs = self.parse_input_srcs()?;
        self.expect_char(',')?;

        let platform = cow_to_bytes(self.parse_string()?);
        self.expect_char(',')?;

        let builder = cow_to_bytes(self.parse_string()?);
        self.expect_char(',')?;

        let args: Vec<Bytes> = self
            .parse_string_list()?
            .into_iter()
            .map(cow_to_bytes)
            .collect();
        self.expect_char(',')?;

        let mut env = self.parse_env()?;

        self.expect_char(')')?;

        // Build DerivationInputs and convert to BTreeSet<SingleDerivedPath>
        let inputs_struct = DerivationInputs {
            srcs: input_srcs,
            drvs: input_drvs,
        };
        let inputs = BTreeSet::from(&inputs_struct);

        // Extract structured attrs from __json env var (matches cppnix behavior:
        // the ATerm format stores structured attrs as a JSON string in __json,
        // but the Derivation type represents them as a separate field).
        let structured_attrs = env
            .remove(&Bytes::from_static(b"__json"))
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
            let output =
                DerivationOutput::from_raw(raw, self.store_dir, drv_name, &name, Base::Hex)?;
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

    fn parse_env(&mut self) -> Result<BTreeMap<Bytes, Bytes>, ParseError> {
        self.expect_char('[')?;
        let mut env = BTreeMap::new();

        while self.peek() != Some(b']') {
            self.expect_char('(')?;
            let key = cow_to_bytes(self.parse_string()?);
            self.expect_char(',')?;
            let value = cow_to_bytes(self.parse_string()?);
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
        self.store_dir
            .parse(s)
            .map_err(|e| ParseError::StorePath { pos, source: e })
    }

    fn parse_string(&mut self) -> Result<Cow<'a, [u8]>, ParseError> {
        self.expect_char('"')?;

        let start = self.pos;

        // Find closing quote
        let end_offset = memchr(b'"', &self.bytes[start..])
            .ok_or(ParseError::UnterminatedString { pos: start })?;

        // Fast path: no escapes — borrow directly from input
        if memchr(b'\\', &self.bytes[start..start + end_offset]).is_none() {
            let result = &self.bytes[start..start + end_offset];
            self.pos = start + end_offset + 1; // skip closing quote
            return Ok(Cow::Borrowed(result));
        }

        // Slow path: handle escape sequences — must allocate
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
                        return Ok(Cow::Owned(result));
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

    fn parse_string_list(&mut self) -> Result<Vec<Cow<'a, [u8]>>, ParseError> {
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
}
