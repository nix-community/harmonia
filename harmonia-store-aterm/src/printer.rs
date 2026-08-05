use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use harmonia_store_derivation::derivation::{
    DerivationInputs, DerivationT, OutputInputs, StructuredAttrs,
};
use harmonia_store_derivation::derived_path::OutputName;
use harmonia_store_path::{StoreDir, StorePath, StorePathName, StorePathSet};

use harmonia_utils_base_encoding::Base;

use crate::raw_output::AtermOutput;

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
        let abs = self.to_absolute_path(store_dir);
        write_escaped(out, abs.to_string_lossy().as_bytes());
    }
}

impl AtermInput for harmonia_utils_hash::Sha256 {
    fn write_key(&self, _store_dir: &StoreDir, out: &mut Vec<u8>) {
        write_escaped(out, format!("{self:x}").as_bytes());
    }
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
    let mut out = Vec::new();
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
    let has_dynamic = drvs.values().any(|oi| !oi.dynamic_outputs.is_empty());

    if has_dynamic {
        out.extend_from_slice(b"DrvWithVersion(\"xp-dyn-drv\",");
    } else {
        out.extend_from_slice(b"Derive(");
    }

    // Outputs
    write_outputs(store_dir, &drv.name, &drv.outputs, out);
    out.push(b',');

    // Input derivations and input sources
    write_input_drvs(store_dir, drvs, has_dynamic, out);
    out.push(b',');
    write_input_srcs(store_dir, srcs, out);
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
    out.push(b']');
    out.push(b',');

    // Env
    write_env(&drv.env, &drv.structured_attrs, out);

    out.push(b')');
}

fn write_outputs<O: AtermOutput>(
    store_dir: &StoreDir,
    drv_name: &StorePathName,
    outputs: &BTreeMap<OutputName, O>,
    out: &mut Vec<u8>,
) {
    out.push(b'[');
    for (i, (name, output)) in outputs.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.push(b'(');

        write_escaped(out, name.as_ref().as_bytes());
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
    out.push(b']');
}

fn write_input_drvs<K: AtermInput>(
    store_dir: &StoreDir,
    drvs: &BTreeMap<K, OutputInputs>,
    versioned: bool,
    out: &mut Vec<u8>,
) {
    out.push(b'[');
    for (i, (key, output_inputs)) in drvs.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.push(b'(');

        key.write_key(store_dir, out);
        out.push(b',');

        write_output_inputs(output_inputs, versioned, out);

        out.push(b')');
    }
    out.push(b']');
}

fn write_output_inputs(oi: &OutputInputs, versioned: bool, out: &mut Vec<u8>) {
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

fn write_output_names(outputs: &BTreeSet<OutputName>, out: &mut Vec<u8>) {
    out.push(b'[');
    for (j, name) in outputs.iter().enumerate() {
        if j > 0 {
            out.push(b',');
        }
        write_escaped(out, name.as_ref().as_bytes());
    }
    out.push(b']');
}

fn write_input_srcs(store_dir: &StoreDir, srcs: &StorePathSet, out: &mut Vec<u8>) {
    out.push(b'[');
    for (i, path) in srcs.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        let abs = path.to_absolute_path(store_dir);
        write_escaped(out, abs.to_string_lossy().as_bytes());
    }
    out.push(b']');
}

fn write_env(
    env: &BTreeMap<Bytes, Bytes>,
    structured_attrs: &Option<StructuredAttrs>,
    out: &mut Vec<u8>,
) {
    // When structured attrs are present, the ATerm format stores them as the
    // `__json` env var. Merge it into the sorted env output.
    let json_key = Bytes::from("__json");
    let json_value = structured_attrs
        .as_ref()
        .map(|sa| Bytes::from(serde_json::to_string(&sa.attrs).unwrap()));

    out.push(b'[');
    let mut first = true;
    let mut json_emitted = json_value.is_none();

    for (key, value) in env.iter() {
        // Emit __json if it sorts before the current key
        if !json_emitted && json_key < *key {
            if !first {
                out.push(b',');
            }
            first = false;
            out.push(b'(');
            write_escaped(out, &json_key);
            out.push(b',');
            write_escaped(out, json_value.as_ref().unwrap());
            out.push(b')');
            json_emitted = true;
        }

        if !first {
            out.push(b',');
        }
        first = false;
        out.push(b'(');
        write_escaped(out, key);
        out.push(b',');
        write_escaped(out, value);
        out.push(b')');
    }

    // Emit __json if it comes after all env keys
    if !json_emitted {
        if !first {
            out.push(b',');
        }
        out.push(b'(');
        write_escaped(out, &json_key);
        out.push(b',');
        write_escaped(out, json_value.as_ref().unwrap());
        out.push(b')');
    }

    out.push(b']');
}

fn write_escaped(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'"');
    for &b in bytes {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            // ATerm strings are byte sequences, not UTF-8. Emit verbatim;
            // re-encoding via `b as char` would mangle bytes >= 0x80.
            _ => out.push(b),
        }
    }
    out.push(b'"');
}
