#![no_main]

use std::io::Cursor;

use bytes::Bytes;
use harmonia_protocol::daemon::wire::types2::Request;
use harmonia_protocol::de::{NixRead, NixReader};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build runtime");
    rt.block_on(async {
        let mut reader = NixReader::builder()
            // Bound allocations so the fuzzer finds logic bugs, not OOMs.
            .set_max_buf_size(1 << 20)
            .build(Cursor::new(Bytes::copy_from_slice(data)));
        // Drain requests until error/EOF. Must not panic.
        while let Ok(Some(_req)) = reader.try_read_value::<Request>().await {}
    });
});
