#![no_main]

use harmonia_store_aterm::{parse_derivation_aterm, print_derivation_aterm};
use harmonia_store_path::{StoreDir, StorePathName};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let store_dir = StoreDir::default();
    let name: StorePathName = "fuzz-0".parse().expect("static name parses");
    // Must not panic; errors are fine.
    let Ok(drv) = parse_derivation_aterm(&store_dir, data, name.clone()) else {
        return;
    };
    // Round-trip: print the parsed derivation and re-parse. The result must
    // be identical, otherwise the printer or parser is lossy/buggy.
    let printed = print_derivation_aterm(&store_dir, &drv);
    let mut reparsed =
        parse_derivation_aterm(&store_dir, &printed, name).expect("reparse printed derivation");
    // Known limitation: structured_attrs are stored as serde_json::Value, and
    // serializing then re-parsing JSON does not preserve exact float
    // representations. C++ Nix stores the raw __json string and avoids this.
    reparsed.structured_attrs = drv.structured_attrs.clone();
    assert_eq!(drv, reparsed, "round-trip mismatch");
});
