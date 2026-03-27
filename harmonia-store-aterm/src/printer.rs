use std::collections::BTreeMap;

use harmonia_store_core::ByteString;
use harmonia_store_core::derivation::{
    DerivationInputs, DerivationOutput, DerivationT, OutputInputs, StructuredAttrs,
};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::{StoreDir, StorePath, StorePathName, StorePathSet};
use harmonia_utils_hash::fmt::CommonHash as _;

/// Print a derivation in Nix ATerm format.
pub fn print_derivation_aterm<I>(store_dir: &StoreDir, drv: &DerivationT<I>) -> String
where
    for<'a> DerivationInputs: From<&'a I>,
{
    let mut out = String::new();
    write_derivation(store_dir, drv, &mut out);
    out
}

/// Write a derivation in Nix ATerm format to a string buffer.
pub fn write_derivation<I>(store_dir: &StoreDir, drv: &DerivationT<I>, out: &mut String)
where
    for<'a> DerivationInputs: From<&'a I>,
{
    out.push_str("Derive(");

    // Outputs
    write_outputs(store_dir, &drv.name, &drv.outputs, out);
    out.push(',');

    // Input derivations and input sources
    let inputs = DerivationInputs::from(&drv.inputs);
    write_input_drvs(store_dir, &inputs.drvs, out);
    out.push(',');
    write_input_srcs(store_dir, &inputs.srcs, out);
    out.push(',');

    // Platform
    write_escaped(out, &drv.platform);
    out.push(',');

    // Builder
    write_escaped(out, &drv.builder);
    out.push(',');

    // Args
    out.push('[');
    for (i, arg) in drv.args.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_escaped(out, arg);
    }
    out.push(']');
    out.push(',');

    // Env
    write_env(&drv.env, &drv.structured_attrs, out);

    out.push(')');
}

fn write_outputs(
    store_dir: &StoreDir,
    drv_name: &StorePathName,
    outputs: &BTreeMap<OutputName, DerivationOutput>,
    out: &mut String,
) {
    out.push('[');
    for (i, (name, output)) in outputs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('(');

        // Output name
        write_escaped(out, name.as_ref().as_bytes());
        out.push(',');

        // (path, hashAlgo, hash) depend on the variant
        match output {
            DerivationOutput::InputAddressed(path) => {
                // path present, hashAlgo and hash empty
                let abs = path.to_absolute_path(store_dir);
                write_escaped(out, abs.to_string_lossy().as_bytes());
                out.push_str(",\"\",\"\"");
            }
            DerivationOutput::CAFixed(ca) => {
                // Nix includes the computed path for CA fixed outputs
                let cama = ca.method_algorithm();
                let hash = ca.hash();
                if let Ok(Some(path)) = output.path(store_dir, drv_name, name) {
                    let abs = path.to_absolute_path(store_dir);
                    write_escaped(out, abs.to_string_lossy().as_bytes());
                } else {
                    out.push_str("\"\"");
                }
                out.push(',');
                write_escaped(out, cama.to_string().as_bytes());
                out.push(',');
                let hash_hex = hash.as_base16().as_bare().to_string();
                write_escaped(out, hash_hex.as_bytes());
            }
            DerivationOutput::CAFloating(cama) => {
                // path empty, hashAlgo present, hash empty
                out.push_str("\"\",");
                write_escaped(out, cama.to_string().as_bytes());
                out.push_str(",\"\"");
            }
            DerivationOutput::Impure(cama) => {
                // path empty, hashAlgo present, hash is literal "impure"
                out.push_str("\"\",");
                write_escaped(out, cama.to_string().as_bytes());
                out.push_str(",\"impure\"");
            }
            DerivationOutput::Deferred => {
                // All empty
                out.push_str("\"\",\"\",\"\"");
            }
        }

        out.push(')');
    }
    out.push(']');
}

fn write_input_drvs(
    store_dir: &StoreDir,
    drvs: &BTreeMap<StorePath, OutputInputs>,
    out: &mut String,
) {
    out.push('[');
    for (i, (path, output_inputs)) in drvs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('(');

        let abs = path.to_absolute_path(store_dir);
        write_escaped(out, abs.to_string_lossy().as_bytes());
        out.push(',');

        // Output names
        out.push('[');
        for (j, name) in output_inputs.outputs.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            write_escaped(out, name.as_ref().as_bytes());
        }
        out.push(']');

        out.push(')');
    }
    out.push(']');
}

fn write_input_srcs(store_dir: &StoreDir, srcs: &StorePathSet, out: &mut String) {
    out.push('[');
    for (i, path) in srcs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let abs = path.to_absolute_path(store_dir);
        write_escaped(out, abs.to_string_lossy().as_bytes());
    }
    out.push(']');
}

fn write_env(
    env: &BTreeMap<ByteString, ByteString>,
    structured_attrs: &Option<StructuredAttrs>,
    out: &mut String,
) {
    // When structured attrs are present, the ATerm format stores them as the
    // `__json` env var. Merge it into the sorted env output.
    let json_key = ByteString::from("__json");
    let json_value = structured_attrs
        .as_ref()
        .map(|sa| ByteString::from(serde_json::to_string(&sa.attrs).unwrap()));

    out.push('[');
    let mut first = true;
    let mut json_emitted = json_value.is_none();

    for (key, value) in env.iter() {
        // Emit __json if it sorts before the current key
        if !json_emitted && json_key < *key {
            if !first {
                out.push(',');
            }
            first = false;
            out.push('(');
            write_escaped(out, &json_key);
            out.push(',');
            write_escaped(out, json_value.as_ref().unwrap());
            out.push(')');
            json_emitted = true;
        }

        if !first {
            out.push(',');
        }
        first = false;
        out.push('(');
        write_escaped(out, key);
        out.push(',');
        write_escaped(out, value);
        out.push(')');
    }

    // Emit __json if it comes after all env keys
    if !json_emitted {
        if !first {
            out.push(',');
        }
        out.push('(');
        write_escaped(out, &json_key);
        out.push(',');
        write_escaped(out, json_value.as_ref().unwrap());
        out.push(')');
    }

    out.push(']');
}

fn write_escaped(out: &mut String, bytes: &[u8]) {
    out.push('"');
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            _ => out.push(b as char),
        }
    }
    out.push('"');
}
