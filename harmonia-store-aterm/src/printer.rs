use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use harmonia_store_derivation::derivation::{
    DerivationInputs, DerivationT, OutputInputs, StructuredAttrs,
};
use harmonia_store_derivation::derived_path::OutputName;
use harmonia_store_path::{StoreDir, StorePath, StorePathHash, StorePathName, StorePathSet};
use harmonia_utils_base_encoding::{Base, base32, encode_for_base};
use harmonia_utils_hash::{Algorithm, Context, Sha256};

use crate::raw_output::AtermOutput;

const STORE_HASH_LEN: usize = StorePathHash::encoded_len();
const SHA256_HEX_LEN: usize = Base::Hex.input_len(Algorithm::SHA256.size());
const JSON_KEY: &[u8] = b"__json";

/// Byte sink for ATerm serialization (either a `Vec<u8>` or a streaming SHA-256 hasher).
pub(crate) trait AtermSink {
    fn push(&mut self, byte: u8);
    fn extend_from_slice(&mut self, bytes: &[u8]);
}

impl AtermSink for Vec<u8> {
    #[inline]
    fn push(&mut self, byte: u8) {
        Vec::push(self, byte);
    }

    #[inline]
    fn extend_from_slice(&mut self, bytes: &[u8]) {
        Vec::extend_from_slice(self, bytes);
    }
}

/// Buffered streaming SHA-256 sink so ATerm hashing avoids allocating an intermediate `Vec<u8>`
/// while batching small writes into 1 KiB chunks before updating the digest context.
pub(crate) struct Sha256Sink {
    ctx: Context,
    buf: [u8; 1024],
    len: usize,
}

impl Sha256Sink {
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            ctx: Context::new(Algorithm::SHA256),
            buf: [0u8; 1024],
            len: 0,
        }
    }

    #[inline]
    fn flush(&mut self) {
        if self.len > 0 {
            self.ctx.update(&self.buf[..self.len]);
            self.len = 0;
        }
    }

    pub(crate) fn finish(mut self) -> Sha256 {
        self.flush();
        self.ctx
            .finish()
            .try_into()
            .expect("SHA256 context always produces SHA256")
    }
}

impl AtermSink for Sha256Sink {
    #[inline]
    fn push(&mut self, byte: u8) {
        if self.len >= self.buf.len() {
            self.flush();
        }
        self.buf[self.len] = byte;
        self.len += 1;
    }

    #[inline]
    fn extend_from_slice(&mut self, bytes: &[u8]) {
        let avail = self.buf.len().saturating_sub(self.len);
        if bytes.len() <= avail {
            self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        } else {
            self.flush();
            if bytes.len() < self.buf.len() {
                self.buf[..bytes.len()].copy_from_slice(bytes);
                self.len = bytes.len();
            } else {
                self.ctx.update(bytes);
            }
        }
    }
}

/// How an input-derivation key is rendered in the ATerm `inputDrvs`
/// list. Normally this is a [`StorePath`] printed as an absolute path,
/// but the intermediate derivation used to compute the hash modulo
/// replaces each input derivation with its own hash modulo, rendered
/// as bare base16 (see [`crate::hash_modulo`]).
pub trait AtermInput {
    fn write_key(&self, store_dir: &StoreDir, out: &mut Vec<u8>);
}

impl AtermInput for StorePath {
    fn write_key(&self, store_dir: &StoreDir, out: &mut Vec<u8>) {
        let escaped_dir = escape_bytes_cow(store_dir.to_str().as_bytes());
        write_store_path_escaped(&escaped_dir, self, out);
    }
}

impl AtermInput for Sha256 {
    fn write_key(&self, _store_dir: &StoreDir, out: &mut Vec<u8>) {
        write_sha256_hex(self, out);
    }
}

fn escape_bytes_cow(bytes: &[u8]) -> Cow<'_, [u8]> {
    if find_escape(bytes).is_none() {
        Cow::Borrowed(bytes)
    } else {
        let mut buf = Vec::with_capacity(bytes.len() + 8);
        write_escaped_bytes(&mut buf, bytes);
        Cow::Owned(buf)
    }
}

#[inline]
fn write_store_path_escaped<S: AtermSink>(escaped_store_dir: &[u8], path: &StorePath, out: &mut S) {
    out.push(b'"');
    out.extend_from_slice(escaped_store_dir);
    let mut hash_buf = [0u8; 1 + STORE_HASH_LEN + 1];
    hash_buf[0] = b'/';
    base32::encode_mut(path.hash().as_ref(), &mut hash_buf[1..1 + STORE_HASH_LEN]);
    hash_buf[1 + STORE_HASH_LEN] = b'-';
    out.extend_from_slice(&hash_buf);
    // StorePathName only permits ASCII [0-9a-zA-Z+._?=-], none of which need ATerm escaping.
    out.extend_from_slice(path.name().as_bytes());
    out.push(b'"');
}

#[inline]
pub(crate) fn write_sha256_hex<S: AtermSink>(hash: &Sha256, out: &mut S) {
    let mut buf = [0u8; 1 + SHA256_HEX_LEN + 1];
    buf[0] = b'"';
    encode_for_base(Base::Hex)(hash.digest_bytes(), &mut buf[1..1 + SHA256_HEX_LEN]);
    buf[1 + SHA256_HEX_LEN] = b'"';
    out.extend_from_slice(&buf);
}

/// Print a derivation in Nix ATerm format.
///
/// Takes a derivation whose inputs are already in the format
/// representation ([`DerivationInputs`]), generic over the
/// input-derivation key. Convert an in-memory derivation with
/// [`DerivationT::into_full`].
pub fn print_derivation_aterm<K: AtermInput, O: AtermOutput>(
    store_dir: &StoreDir,
    drv: &DerivationT<DerivationInputs<K>, O>,
) -> Vec<u8> {
    let DerivationInputs { srcs, drvs } = &drv.inputs;
    let args_len: usize = drv.args.iter().map(|a| a.len() + 3).sum();
    let env_len: usize = drv.env.iter().map(|(k, v)| k.len() + v.len() + 8).sum();
    // Account for structured attrs (__json) plus ~25% headroom for JSON quote escaping.
    let sa_len = drv
        .structured_attrs
        .as_ref()
        .and_then(StructuredAttrs::verbatim)
        .map_or(0, |s| s.len() + (s.len() >> 2) + 16);
    let capacity = 256
        + drv.outputs.len() * 80
        + drvs.len() * 128
        + srcs.len() * 64
        + drv.platform.len()
        + drv.builder.len()
        + args_len
        + env_len
        + sa_len;
    let mut out = Vec::with_capacity(capacity);
    write_derivation(store_dir, drv, &mut out);
    out
}

/// Write a derivation in Nix ATerm format to a string buffer. See
/// [`print_derivation_aterm`].
pub fn write_derivation<K: AtermInput, O: AtermOutput>(
    store_dir: &StoreDir,
    drv: &DerivationT<DerivationInputs<K>, O>,
    out: &mut Vec<u8>,
) {
    let DerivationInputs { srcs, drvs } = &drv.inputs;
    write_derivation_parts(
        store_dir,
        drv,
        srcs,
        drvs,
        |key, sink| key.write_key(store_dir, sink),
        false,
        out,
    );
}

pub(crate) fn write_derivation_parts<S: AtermSink, I, K, O: AtermOutput>(
    store_dir: &StoreDir,
    drv: &DerivationT<I, O>,
    srcs: &StorePathSet,
    drvs: &BTreeMap<K, OutputInputs>,
    write_key: impl Fn(&K, &mut S),
    mask_outputs: bool,
    out: &mut S,
) {
    let escaped_dir = escape_bytes_cow(store_dir.to_str().as_bytes());
    let has_dynamic = drvs.values().any(|oi| !oi.dynamic_outputs.is_empty());

    if has_dynamic {
        out.extend_from_slice(b"DrvWithVersion(\"xp-dyn-drv\",");
    } else {
        out.extend_from_slice(b"Derive(");
    }

    // Outputs
    write_outputs(store_dir, &drv.name, &drv.outputs, mask_outputs, out);
    out.push(b',');

    // Input derivations and input sources
    write_input_drvs(drvs, has_dynamic, &write_key, out);
    out.push(b',');
    write_input_srcs(&escaped_dir, srcs, out);
    out.push(b',');

    // Platform
    write_escaped(out, &drv.platform);
    out.push(b',');

    // Builder
    write_escaped(out, &drv.builder);
    out.push(b',');

    // Args
    out.push(b'[');
    for (i, arg) in drv.args.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        write_escaped(out, arg);
    }
    out.extend_from_slice(b"],");

    // Env
    write_env(
        &drv.env,
        &drv.structured_attrs,
        &drv.outputs,
        mask_outputs,
        out,
    );

    out.push(b')');
}

fn write_outputs<S: AtermSink, O: AtermOutput>(
    store_dir: &StoreDir,
    drv_name: &StorePathName,
    outputs: &BTreeMap<OutputName, O>,
    mask_outputs: bool,
    out: &mut S,
) {
    out.push(b'[');
    for (i, (name, output)) in outputs.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.push(b'(');
        write_escaped(out, name.as_ref().as_bytes());
        if mask_outputs {
            out.extend_from_slice(b",\"\",\"\",\"\")");
        } else {
            out.push(b',');
            let raw = output
                .to_raw(store_dir, drv_name, name, Base::Hex)
                .expect("output path name should be valid");
            write_escaped(out, &raw.path);
            out.push(b',');
            write_escaped(out, &raw.hash_algo);
            out.push(b',');
            write_escaped(out, &raw.hash);
            out.push(b')');
        }
    }
    out.push(b']');
}

fn write_input_drvs<S: AtermSink, K>(
    drvs: &BTreeMap<K, OutputInputs>,
    versioned: bool,
    write_key: &impl Fn(&K, &mut S),
    out: &mut S,
) {
    out.push(b'[');
    for (i, (key, output_inputs)) in drvs.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.push(b'(');

        write_key(key, out);
        out.push(b',');

        write_output_inputs(output_inputs, versioned, out);

        out.push(b')');
    }
    out.push(b']');
}

fn write_output_inputs<S: AtermSink>(oi: &OutputInputs, versioned: bool, out: &mut S) {
    if versioned && !oi.dynamic_outputs.is_empty() {
        out.push(b'(');
        write_output_names(&oi.outputs, out);
        out.extend_from_slice(b",[");
        for (i, (name, child)) in oi.dynamic_outputs.iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            out.push(b'(');
            write_escaped(out, name.as_ref().as_bytes());
            out.push(b',');
            write_output_inputs(child, versioned, out);
            out.push(b')');
        }
        out.extend_from_slice(b"])");
    } else {
        write_output_names(&oi.outputs, out);
    }
}

fn write_output_names<S: AtermSink>(outputs: &BTreeSet<OutputName>, out: &mut S) {
    out.push(b'[');
    for (j, name) in outputs.iter().enumerate() {
        out.extend_from_slice(if j > 0 { b",\"" } else { b"\"" });
        write_escaped_bytes(out, name.as_ref().as_bytes());
        out.push(b'"');
    }
    out.push(b']');
}

fn write_input_srcs<S: AtermSink>(escaped_dir: &[u8], srcs: &StorePathSet, out: &mut S) {
    out.push(b'[');
    for (i, path) in srcs.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        write_store_path_escaped(escaped_dir, path, out);
    }
    out.push(b']');
}

fn write_env<S: AtermSink, O>(
    env: &BTreeMap<Bytes, Bytes>,
    structured_attrs: &Option<StructuredAttrs>,
    outputs: &BTreeMap<OutputName, O>,
    mask_outputs: bool,
    out: &mut S,
) {
    // When structured attrs are present, the ATerm format stores them as the
    // `__json` env var. Merge it into the sorted env output.
    let json_value = structured_attrs.as_ref().map(StructuredAttrs::unparse);

    out.push(b'[');
    let mut first = true;
    let mut json_emitted = json_value.is_none();
    // Both `env` and `outputs` are sorted in ascending byte order; advance `output_keys`
    // in lockstep so masking is O(|env| + |outputs|).
    let mut output_keys = outputs.keys().peekable();

    let emit_json = |first: &mut bool, out: &mut S| {
        out.extend_from_slice(if *first {
            b"(\"__json\",\""
        } else {
            b",(\"__json\",\""
        });
        *first = false;
        write_escaped_bytes(out, json_value.as_deref().unwrap().as_bytes());
        out.extend_from_slice(b"\")");
    };

    for (key, value) in env.iter() {
        // Emit __json if it sorts before the current key
        if !json_emitted && JSON_KEY < key.as_ref() {
            emit_json(&mut first, out);
            json_emitted = true;
        }

        out.extend_from_slice(if first { b"(\"" } else { b",(\"" });
        first = false;
        write_escaped_bytes(out, key);

        let masked = mask_outputs && {
            while let Some(o) = output_keys.peek() {
                if o.as_ref().as_bytes() < key.as_ref() {
                    output_keys.next();
                } else {
                    break;
                }
            }
            output_keys
                .peek()
                .is_some_and(|o| o.as_ref().as_bytes() == key.as_ref())
        };
        if masked {
            out.extend_from_slice(b"\",\"\")");
        } else {
            out.extend_from_slice(b"\",\"");
            write_escaped_bytes(out, value);
            out.extend_from_slice(b"\")");
        }
    }

    // Emit __json if it comes after all env keys
    if !json_emitted {
        emit_json(&mut first, out);
    }

    out.push(b']');
}

/// Maps each byte to the character following `\` in its ATerm escape sequence
/// (`\"`, `\\`, `\n`, `\t`, `\r`), or `0` if the byte is emitted verbatim.
const ESCAPE_LUT: [u8; 256] = {
    let mut table = [0u8; 256];
    table[b'"' as usize] = b'"';
    table[b'\\' as usize] = b'\\';
    table[b'\n' as usize] = b'n';
    table[b'\t' as usize] = b't';
    table[b'\r' as usize] = b'r';
    table
};

/// Returns a bitmask with bit 7 set in every byte lane of `word` that is
/// `< 0x20` (covering `\t`, `\n`, `\r`), `b'"'`, or `b'\\'`.
///
/// False positives arise from other control bytes `< 0x20` or borrow propagation
/// into higher lanes; callers filter every flagged lane through `ESCAPE_LUT`.
/// False negatives are impossible for all five escaped bytes.
#[inline(always)]
const fn needs_escape_mask(word: u64) -> u64 {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let lt_space = word.wrapping_sub(LO * 0x20) & !word & HI;
    let q = word ^ (LO * (b'"' as u64));
    let is_quote = q.wrapping_sub(LO) & !q & HI;
    let b = word ^ (LO * (b'\\' as u64));
    let is_bs = b.wrapping_sub(LO) & !b & HI;
    lt_space | is_quote | is_bs
}

#[inline(always)]
fn load_u64(slice: &[u8]) -> u64 {
    u64::from_le_bytes(slice.try_into().unwrap())
}

#[inline]
fn find_escape(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 8 <= bytes.len() {
        let mut mask = needs_escape_mask(load_u64(&bytes[i..i + 8]));
        while mask != 0 {
            let lane = (mask.trailing_zeros() >> 3) as usize;
            mask &= mask - 1;
            let pos = i + lane;
            if ESCAPE_LUT[bytes[pos] as usize] != 0 {
                return Some(pos);
            }
        }
        i += 8;
    }
    while i < bytes.len() {
        if ESCAPE_LUT[bytes[i] as usize] != 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[inline]
fn write_escaped<S: AtermSink>(out: &mut S, bytes: &[u8]) {
    out.push(b'"');
    write_escaped_bytes(out, bytes);
    out.push(b'"');
}

#[inline]
fn emit_escape<S: AtermSink>(out: &mut S, bytes: &[u8], pos: usize, esc: u8, start: &mut usize) {
    if pos > *start {
        out.extend_from_slice(&bytes[*start..pos]);
    }
    out.extend_from_slice(&[b'\\', esc]);
    *start = pos + 1;
}

#[inline]
fn flush_mask<S: AtermSink>(
    out: &mut S,
    bytes: &[u8],
    base: usize,
    mut mask: u64,
    start: &mut usize,
) {
    while mask != 0 {
        let lane = (mask.trailing_zeros() >> 3) as usize;
        mask &= mask - 1;
        let pos = base + lane;
        let esc = ESCAPE_LUT[bytes[pos] as usize];
        if esc != 0 {
            emit_escape(out, bytes, pos, esc, start);
        }
    }
}

fn write_escaped_bytes<S: AtermSink>(out: &mut S, bytes: &[u8]) {
    let mut start = 0;
    let mut i = 0;
    while i + 32 <= bytes.len() {
        let m0 = needs_escape_mask(load_u64(&bytes[i..i + 8]));
        let m1 = needs_escape_mask(load_u64(&bytes[i + 8..i + 16]));
        let m2 = needs_escape_mask(load_u64(&bytes[i + 16..i + 24]));
        let m3 = needs_escape_mask(load_u64(&bytes[i + 24..i + 32]));
        if (m0 | m1 | m2 | m3) != 0 {
            if m0 != 0 {
                flush_mask(out, bytes, i, m0, &mut start);
            }
            if m1 != 0 {
                flush_mask(out, bytes, i + 8, m1, &mut start);
            }
            if m2 != 0 {
                flush_mask(out, bytes, i + 16, m2, &mut start);
            }
            if m3 != 0 {
                flush_mask(out, bytes, i + 24, m3, &mut start);
            }
        }
        i += 32;
    }
    while i + 8 <= bytes.len() {
        let mask = needs_escape_mask(load_u64(&bytes[i..i + 8]));
        if mask != 0 {
            flush_mask(out, bytes, i, mask, &mut start);
        }
        i += 8;
    }
    while i < bytes.len() {
        let esc = ESCAPE_LUT[bytes[i] as usize];
        if esc != 0 {
            emit_escape(out, bytes, i, esc, &mut start);
        }
        i += 1;
    }
    if start < bytes.len() {
        out.extend_from_slice(&bytes[start..]);
    }
}

#[cfg(test)]
mod escape_tests {
    use super::write_escaped_bytes;

    fn reference(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for &b in bytes {
            match b {
                b'"' => out.extend_from_slice(b"\\\""),
                b'\\' => out.extend_from_slice(b"\\\\"),
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\t' => out.extend_from_slice(b"\\t"),
                b'\r' => out.extend_from_slice(b"\\r"),
                _ => out.push(b),
            }
        }
        out
    }

    fn escaped(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        write_escaped_bytes(&mut out, bytes);
        out
    }

    #[test]
    fn matches_reference_on_all_short_mixes() {
        // Every string of length <= 5 over the escaped bytes plus two plain bytes.
        const ALPHABET: &[u8] = b"\"\\\n\t\rax\xff";
        let mut cases = vec![Vec::new()];
        for _ in 0..5 {
            let next: Vec<Vec<u8>> = cases
                .iter()
                .flat_map(|c| ALPHABET.iter().map(move |&b| [c.as_slice(), &[b]].concat()))
                .collect();
            for c in &next {
                assert_eq!(escaped(c), reference(c), "input {c:?}");
            }
            cases = next;
        }
    }

    #[test]
    fn long_tab_and_cr_runs() {
        for input in [
            vec![b'\t'; 1 << 20],
            b"\r\t".repeat(1 << 19),
            [vec![b'\t'; 1 << 19], b"\"tail\\\n".to_vec()].concat(),
        ] {
            assert_eq!(escaped(&input), reference(&input));
        }
    }

    #[test]
    fn swar_boundaries_all_bytes_and_false_positives() {
        // Every single byte value, alone and across 8-/32-byte SWAR boundaries.
        let all_bytes: Vec<u8> = (0u8..=255).collect();
        assert_eq!(escaped(&all_bytes), reference(&all_bytes));

        for b in 0u8..=255 {
            assert_eq!(escaped(&[b]), reference(&[b]));
            // Exercise borrow-propagation false positives (control byte followed by space/ASCII/high byte).
            for next in [b' ', b'!', b'"', b'\\', b'\n', 0x7f, 0x80, 0xff] {
                let mut buf = vec![b'a'; 40];
                for pos in [0, 6, 7, 8, 15, 30, 31, 32, 38] {
                    buf.fill(b'a');
                    buf[pos] = b;
                    buf[pos + 1] = next;
                    assert_eq!(
                        escaped(&buf),
                        reference(&buf),
                        "b={b:#x} next={next:#x} pos={pos}"
                    );
                }
            }
        }

        // Lengths 0..=40 with escapes at each position.
        for len in 0..=40 {
            for &esc in b"\"\\\n\t\r\x01\x1f\x7f\x80" {
                let mut buf = vec![b'x'; len];
                assert_eq!(escaped(&buf), reference(&buf));
                if len > 0 {
                    buf[0] = esc;
                    assert_eq!(escaped(&buf), reference(&buf));
                    buf[0] = b'x';
                    buf[len - 1] = esc;
                    assert_eq!(escaped(&buf), reference(&buf));
                }
            }
        }
    }
}
