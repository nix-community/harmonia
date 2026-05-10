#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build runtime");
    rt.block_on(async {
        // Must not panic; errors are fine.
        let _ = harmonia_file_nar::archive::read_nar(Cursor::new(data)).await;
    });
});
