#![no_main]

use harmonia_store_aterm::parse_derivation_aterm;
use harmonia_store_core::store_path::{StoreDir, StorePathName};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    let store_dir = StoreDir::default();
    let name: StorePathName = "fuzz-0".parse().expect("static name parses");
    // Must not panic; errors are fine.
    let _ = parse_derivation_aterm(&store_dir, input, name);
});
