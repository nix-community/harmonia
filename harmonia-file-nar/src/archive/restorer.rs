//! NAR restore: parse NAR format and write to a [`FileSystemSink`].

use std::io;

use futures_util::StreamExt as _;
use harmonia_file_io_pure::{DirectorySink, FileSystemSink};
use harmonia_utils_io::AsyncBytesRead;
use tokio::io::AsyncWriteExt;

use super::NarEvent;
use super::parser::NarParser;

/// Restore a NAR archive from an async reader into a [`FileSystemSink`].
///
/// Uses the existing [`NarParser`] to parse the NAR format, then maps
/// the event stream to [`FileSystemSink`] calls.
pub async fn restore_to_sink<R, S>(reader: R, sink: S) -> io::Result<()>
where
    R: AsyncBytesRead + Unpin,
    S: FileSystemSink,
    S::Error: Send + Sync + 'static,
{
    let mut parser = NarParser::new(reader);
    let first = parser
        .next()
        .await
        .ok_or_else(|| io::Error::other("empty NAR"))??;
    restore_sink_event(&mut parser, first, sink).await?;
    if let Some(result) = parser.next().await {
        result?;
        return Err(io::Error::other("trailing data after NAR root"));
    }
    Ok(())
}

/// Process one NarEvent and recursively handle directory children.
async fn restore_sink_event<R, EV>(
    parser: &mut NarParser<R>,
    event: NarEvent<EV>,
    sink: impl FileSystemSink<Error: Send + Sync + 'static>,
) -> io::Result<()>
where
    R: AsyncBytesRead + Unpin,
    EV: tokio::io::AsyncRead + Unpin,
{
    match event {
        NarEvent::File {
            executable,
            mut reader,
            ..
        } => {
            let mut file_sink = sink
                .create_regular_file(executable)
                .await
                .map_err(sink_to_io)?;
            tokio::io::copy(&mut reader, &mut file_sink).await?;
            file_sink.shutdown().await?;
        }
        NarEvent::Symlink { target, .. } => {
            let target = std::str::from_utf8(&target)
                .map_err(|e| io::Error::other(format!("non-UTF-8 symlink target: {e}")))?;
            sink.create_symlink(target).await.map_err(sink_to_io)?;
        }
        NarEvent::StartDirectory { .. } => {
            let mut dir = sink.create_directory().await.map_err(sink_to_io)?;
            while let Some(result) = parser.next().await {
                let event = result?;
                if matches!(event, NarEvent::EndDirectory) {
                    return Ok(());
                }
                let name = event_name(&event)?;
                let child_sink = dir.create_child(&name).await.map_err(sink_to_io)?;
                Box::pin(restore_sink_event(parser, event, child_sink)).await?;
            }
            return Err(io::Error::other("unexpected end of NAR stream"));
        }
        NarEvent::EndDirectory => {
            return Err(io::Error::other("unexpected EndDirectory"));
        }
    }
    Ok(())
}

fn event_name<R>(event: &NarEvent<R>) -> io::Result<String> {
    let name_bytes = match event {
        NarEvent::File { name, .. }
        | NarEvent::Symlink { name, .. }
        | NarEvent::StartDirectory { name } => name,
        NarEvent::EndDirectory => return Err(io::Error::other("EndDirectory has no name")),
    };
    std::str::from_utf8(name_bytes)
        .map(|s| s.to_owned())
        .map_err(|e| io::Error::other(format!("non-UTF-8 entry name: {e}")))
}

fn sink_to_io(e: impl std::error::Error) -> io::Error {
    io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use harmonia_file_core::*;
    use harmonia_file_io_pure::*;
    use tokio::io::AsyncWriteExt;

    use super::restore_to_sink;
    use crate::archive::dumper::dump_source;

    async fn sample_tree() -> MemoryTree {
        let mut builder = MemoryTreeBuilder::new();
        {
            let mut root = builder.sink().create_directory().await.unwrap();
            {
                let mut sub = root
                    .create_child("dir")
                    .await
                    .unwrap()
                    .create_directory()
                    .await
                    .unwrap();
                sub.create_child("file")
                    .await
                    .unwrap()
                    .create_regular_file(true)
                    .await
                    .unwrap()
                    .write_all(b"hello")
                    .await
                    .unwrap();
                sub.create_child("link")
                    .await
                    .unwrap()
                    .create_symlink("/target")
                    .await
                    .unwrap();
            }
            root.create_child("readme")
                .await
                .unwrap()
                .create_regular_file(false)
                .await
                .unwrap()
                .write_all(b"world")
                .await
                .unwrap();
        }
        builder.build()
    }

    #[tokio::test]
    async fn round_trip_dump_restore() {
        let tree = sample_tree().await;
        let src = MemoryTreeSource::new(&tree);

        // Dump to NAR bytes
        let mut nar = Vec::new();
        dump_source(&src, &mut nar).await.unwrap();

        // Restore from NAR bytes into a new MemoryTree
        let mut builder = MemoryTreeBuilder::new();
        let reader = harmonia_utils_io::BytesReader::new(std::io::Cursor::new(nar));
        restore_to_sink(reader, builder.sink()).await.unwrap();
        let restored = builder.build();

        // Compare via JSON serialization
        let orig_json = serde_json::to_value(list_deep(&src).await.unwrap()).unwrap();
        let restored_src = MemoryTreeSource::new(&restored);
        let restored_json = serde_json::to_value(list_deep(&restored_src).await.unwrap()).unwrap();
        assert_eq!(orig_json, restored_json);
    }
}

/// Restore-then-dump round-trip tests using test_data fixtures.
///
/// These correspond to the old `NarRestorer` unit tests: write test
/// events as NAR via `NarWriter`, restore into a `MemoryTree` via
/// `restore_to_sink`, dump back via `dump_source`, parse back into
/// events, and compare.
#[cfg(test)]
mod fixture_tests {
    use futures_util::StreamExt as _;
    use harmonia_file_io_pure::*;
    use harmonia_utils_io::BytesReader;
    use rstest::rstest;

    use super::restore_to_sink;
    use crate::archive::dumper::dump_source;
    use crate::archive::parser::NarParser;
    use crate::archive::test_data;
    use crate::archive::write_nar;

    /// Write test events as NAR bytes, restore into MemoryTree, dump
    /// back to NAR, parse into events, compare.
    async fn round_trip_via_memory(events: test_data::TestNarEvents) -> test_data::TestNarEvents {
        // Events → NAR bytes (via NarWriter)
        let nar_bytes = write_nar(events.iter());

        // NAR bytes → MemoryTree (via restore_to_sink)
        let mut builder = MemoryTreeBuilder::new();
        let reader = BytesReader::new(std::io::Cursor::new(nar_bytes));
        restore_to_sink(reader, builder.sink()).await.unwrap();
        let tree = builder.build();

        // MemoryTree → NAR bytes (via dump_source)
        let src = MemoryTreeSource::new(&tree);
        let mut nar2 = Vec::new();
        dump_source(&src, &mut nar2).await.unwrap();

        // NAR bytes → events (via NarParser)
        let reader = BytesReader::new(std::io::Cursor::new(nar2));
        let mut parser = NarParser::new(reader);
        let mut result = test_data::TestNarEvents::new();
        while let Some(event) = parser.next().await {
            result.push(event.unwrap().read_file().await.unwrap());
        }
        result
    }

    #[tokio::test]
    #[rstest]
    #[case::text_file(test_data::text_file())]
    #[case::exec_file(test_data::exec_file())]
    #[case::empty_file(test_data::empty_file())]
    #[case::empty_file_in_dir(test_data::empty_file_in_dir())]
    #[case::empty_dir(test_data::empty_dir())]
    #[case::empty_dir_in_dir(test_data::empty_dir_in_dir())]
    #[case::symlink(test_data::symlink())]
    #[case::dir_example(test_data::dir_example())]
    #[case::case_hack_sorting(test_data::case_hack_sorting())]
    async fn test_restore(#[case] events: test_data::TestNarEvents) {
        let result = round_trip_via_memory(events.clone()).await;
        assert_eq!(result, events);
    }
}

#[cfg(test)]
mod proptests {
    use futures_util::StreamExt as _;
    use harmonia_file_io_pure::*;
    use harmonia_utils_io::BytesReader;
    use proptest::prop_assert_eq;
    use proptest::proptest;

    use super::restore_to_sink;
    use crate::archive::dumper::dump_source;
    use crate::archive::parser::NarParser;
    use crate::archive::test_data;
    use crate::archive::write_nar;
    use crate::test::arbitrary::archive::arb_nar_events;

    #[test]
    fn proptest_restore_dump() {
        let r = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        proptest!(|(events in arb_nar_events(8, 256, 10))| {
            r.block_on(async {
                // Events → NAR bytes
                let nar_bytes = write_nar(events.iter());

                // NAR bytes → MemoryTree
                let mut builder = MemoryTreeBuilder::new();
                let reader = BytesReader::new(std::io::Cursor::new(nar_bytes));
                restore_to_sink(reader, builder.sink()).await.unwrap();
                let tree = builder.build();

                // MemoryTree → NAR bytes
                let src = MemoryTreeSource::new(&tree);
                let mut nar2 = Vec::new();
                dump_source(&src, &mut nar2).await.unwrap();

                // NAR bytes → events
                let reader = BytesReader::new(std::io::Cursor::new(nar2));
                let mut parser = NarParser::new(reader);
                let mut result = test_data::TestNarEvents::new();
                while let Some(event) = parser.next().await {
                    result.push(event.unwrap().read_file().await.unwrap());
                }

                prop_assert_eq!(&result, &events);
                Ok(())
            })?;
        });
    }
}
