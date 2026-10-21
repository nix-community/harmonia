//! Parsing a deeply nested NAR must not keep a read buffer alive per open
//! directory. Separate binary because of the global allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::{self, Cursor};
use std::pin::pin;

use bytes::Bytes;
use futures_util::{Stream, TryStreamExt as _, stream};
use harmonia_file_nar::archive::{test_data::TestNarEvents, write_nar};
use harmonia_file_nar::{NarEvent, parse_nar, parse_nar_stream};
use harmonia_utils_io::DEFAULT_RESERVED_BUF_SIZE;
use tokio::io::AsyncBufRead;

struct PeakAlloc;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn track(delta: isize) {
    let live = LIVE.get() + delta;
    LIVE.set(live);
    PEAK.set(PEAK.get().max(live));
}

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for PeakAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        track(layout.size() as isize);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        track(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

const DEPTH: usize = 512;
// Large enough that each directory's last entry name lands in a new buffer.
const FILE_SIZE: usize = 2 * DEFAULT_RESERVED_BUF_SIZE;

/// Builds a NAR of `DEPTH` nested directories, each holding a file "a" and
/// the next directory "b".
fn deep_nar() -> Bytes {
    let contents = Bytes::from(vec![b'x'; FILE_SIZE]);
    let mut events = TestNarEvents::new();
    for level in 0..DEPTH {
        let name: &'static [u8] = if level == 0 { b"" } else { b"b" };
        events.push(NarEvent::StartDirectory {
            name: Bytes::from_static(name),
        });
        events.push(NarEvent::File {
            name: Bytes::from_static(b"a"),
            executable: false,
            size: FILE_SIZE as u64,
            reader: Cursor::new(contents.clone()),
        });
    }
    events.extend((0..DEPTH).map(|_| NarEvent::EndDirectory));
    write_nar(&events)
}

async fn peak_heap<R>(events: impl Stream<Item = io::Result<NarEvent<R>>>) -> isize
where
    R: AsyncBufRead + Unpin,
{
    let start = LIVE.get();
    PEAK.set(start);
    let mut events = pin!(events);
    while let Some(event) = events.try_next().await.unwrap() {
        if let NarEvent::File { mut reader, .. } = event {
            tokio::io::copy_buf(&mut reader, &mut tokio::io::sink())
                .await
                .unwrap();
        }
    }
    PEAK.get() - start
}

// One pinned buffer or chunk per directory would be DEPTH * FILE_SIZE = 8 MiB.
const MAX_PEAK: isize = 1024 * 1024;

#[tokio::test]
async fn deep_nar_does_not_pin_a_buffer_per_directory() {
    let nar = deep_nar();
    let peak = peak_heap(parse_nar(Cursor::new(nar))).await;
    assert!(peak < MAX_PEAK, "peak heap while parsing: {peak} bytes");
}

#[tokio::test]
async fn deep_nar_stream_does_not_pin_a_chunk_per_directory() {
    let nar = deep_nar();
    let chunks = nar
        .chunks(FILE_SIZE)
        .map(|chunk| Ok(Bytes::copy_from_slice(chunk)));
    let peak = peak_heap(parse_nar_stream(stream::iter(chunks))).await;
    assert!(peak < MAX_PEAK, "peak heap while parsing: {peak} bytes");
}
