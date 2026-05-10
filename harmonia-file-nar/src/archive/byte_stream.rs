//! Streaming NAR byte output from a filesystem path.

use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use super::dumper::dump_source;

/// A [`Stream`] of [`Bytes`] chunks containing NAR-encoded data for a path.
///
/// Opens the path as a [`DirSource`](harmonia_file_fd::DirSource) and
/// drives [`dump_source`] on a spawned task. The NAR bytes are streamed
/// back via a channel.
pub struct NarByteStream {
    rx: mpsc::Receiver<io::Result<Bytes>>,
}

impl NarByteStream {
    /// Create a new `NarByteStream` for the given filesystem path.
    pub fn new(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let result = Self::produce(path, tx.clone()).await;
            if let Err(e) = result {
                let _ = tx.send(Err(e)).await;
            }
        });
        Self { rx }
    }

    async fn produce(path: PathBuf, tx: mpsc::Sender<io::Result<Bytes>>) -> io::Result<()> {
        use harmonia_file_fd::DirSource;

        let source = DirSource::open_path(&path)
            .await
            .map_err(io::Error::other)?;

        // Write NAR to a duplex stream, read chunks and send to channel
        let (mut writer, mut reader) = tokio::io::duplex(64 * 1024);

        let dump_handle = tokio::spawn(async move { dump_source(&source, &mut writer).await });

        let outcome = loop {
            let mut buf = vec![0u8; 256 * 1024];
            match reader.read(&mut buf).await {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    buf.truncate(n);
                    if tx.send(Ok(Bytes::from(buf))).await.is_err() {
                        break Err(None); // receiver dropped
                    }
                }
                Err(e) => break Err(Some(e)),
            }
        };

        match outcome {
            Ok(()) => dump_handle.await.map_err(io::Error::other)??,
            // The dumper would block forever writing into the undrained duplex, so abort instead of awaiting.
            Err(reason) => {
                dump_handle.abort();
                if let Some(e) = reason {
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}

impl Stream for NarByteStream {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::test_data;
    use crate::archive::write_nar;
    use futures_util::StreamExt as _;

    async fn collect(path: PathBuf) -> Vec<u8> {
        let mut s = NarByteStream::new(path);
        let mut out = Vec::new();
        while let Some(chunk) = s.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        out
    }

    /// The byte stream must produce valid NAR matching `nix-store --dump`.
    #[tokio::test]
    async fn byte_stream_matches_nix_store_dump() {
        let dir = tempfile::Builder::new()
            .prefix("nar_byte_stream")
            .tempdir()
            .unwrap();
        let path = dir.path().join("nar");
        let case_hack = cfg!(target_os = "macos");
        test_data::create_dir_example(&path, case_hack).unwrap();

        let got = collect(path.clone()).await;
        let want = write_nar(test_data::dir_example().iter());
        assert_eq!(got, want.to_vec());
    }

    #[tokio::test]
    async fn byte_stream_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, b"Hello world!").unwrap();

        let got = collect(path).await;
        let want = write_nar(test_data::text_file().iter());
        assert_eq!(got, want.to_vec());
    }

    /// A root that is itself a symlink must be dumped as the symlink, not the
    /// directory it points at.
    #[tokio::test]
    async fn byte_stream_root_symlink_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink("target", &link).unwrap();

        let got = collect(link.clone()).await;
        let want = std::process::Command::new("nix-store")
            .arg("--dump")
            .arg(&link)
            .output()
            .expect("nix-store --dump failed");
        assert!(want.status.success());
        assert_eq!(got, want.stdout);
    }

    #[tokio::test]
    async fn byte_stream_large_file_matches_nix_store_dump() {
        const LEN: usize = 300 * 1024 + 5;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big");
        let data = (0..LEN).map(|i| (i % 251) as u8).collect::<Vec<u8>>();
        std::fs::write(&path, &data).unwrap();

        let got = collect(path.clone()).await;

        let want = std::process::Command::new("nix-store")
            .arg("--dump")
            .arg(&path)
            .output()
            .expect("nix-store --dump failed");
        assert!(want.status.success());
        assert_eq!(got, want.stdout);
    }
}
