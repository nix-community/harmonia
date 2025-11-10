use std::collections::HashMap;
use std::fmt;
use std::fs::{OpenOptions, create_dir};
use std::future::Future;
use std::io::{self, BufRead as _, Write as _};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::task::{Poll, ready};

use bstr::ByteSlice as _;
use bytes::Bytes;
use derive_more::Display;
use futures::{Sink, Stream};
use pin_project_lite::pin_project;
use thiserror::Error;
use tokio::io::AsyncBufRead;
use tokio::task::{JoinHandle, spawn_blocking};
use tokio_util::io::SyncIoBridge;
use tracing::{debug, trace};

use super::{CASE_HACK_SUFFIX, NarEvent};

#[derive(Display, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub enum NarWriteOperation {
    #[display("creating directory")]
    CreateDirectory,
    #[display("creating symlink")]
    CreateSymlink,
    #[display("creating file")]
    CreateFile,
    #[display("path contains invalid UTF-8")]
    PathUTF8,
    #[display("Could not join state")]
    JoinError,
}

#[derive(Error, Debug)]
#[error("{operation} {path}: {source}")]
pub struct NarWriteError {
    operation: NarWriteOperation,
    path: PathBuf,
    #[source]
    source: io::Error,
}

impl NarWriteError {
    pub fn new(operation: NarWriteOperation, path: PathBuf, source: io::Error) -> Self {
        Self {
            operation,
            path,
            source,
        }
    }
    pub fn path_utf8_error(path: PathBuf, err: bstr::Utf8Error) -> Self {
        Self::new(
            NarWriteOperation::PathUTF8,
            path,
            io::Error::new(io::ErrorKind::InvalidData, err),
        )
    }
    pub fn create_dir_error(path: PathBuf, err: io::Error) -> Self {
        Self::new(NarWriteOperation::CreateDirectory, path, err)
    }
    pub fn create_symlink_error(path: PathBuf, err: io::Error) -> Self {
        Self::new(NarWriteOperation::CreateSymlink, path, err)
    }
    pub fn create_file_error(path: PathBuf, err: io::Error) -> Self {
        Self::new(NarWriteOperation::CreateFile, path, err)
    }
}

pin_project! {
    pub struct NarRestorer {
        root: PathBuf,
        path: PathBuf,
        #[pin]
        state: Option<JoinHandle<Result<(), NarWriteError>>>,
        use_case_hack: bool,
        entries: Entries,
        dir_stack: Vec<Entries>,
    }
}

impl NarRestorer {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self::new_restorer(path, false)
    }

    pub fn with_case_hack<P: Into<PathBuf>>(path: P) -> Self {
        Self::new_restorer(path, true)
    }

    fn new_restorer<P>(path: P, use_case_hack: bool) -> Self
    where
        P: Into<PathBuf>,
    {
        let path = path.into();
        Self {
            root: path.clone(),
            path,
            state: None,
            use_case_hack,
            entries: Default::default(),
            dir_stack: Default::default(),
        }
    }
}

fn join_name(path: &Path, name: &[u8]) -> Result<PathBuf, NarWriteError> {
    if name.is_empty() {
        Ok(path.to_owned())
    } else {
        let name_os = name.to_os_str().map_err(|err| {
            let lossy = name.to_os_str_lossy();
            let path = path.join(lossy);
            NarWriteError::path_utf8_error(path, err)
        })?;
        Ok(path.join(name_os))
    }
}

impl<R> Sink<NarEvent<R>> for NarRestorer
where
    R: AsyncBufRead + Send + 'static,
{
    type Error = NarWriteError;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let mut this = self.project();
        if let Some(state) = this.state.as_mut().as_pin_mut() {
            ready!(state.poll(cx)).map_err(|_| {
                NarWriteError::new(
                    NarWriteOperation::JoinError,
                    this.root.clone(),
                    io::Error::other("background task failed"),
                )
            })??;
        }
        this.state.set(None);
        Poll::Ready(Ok(()))
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        item: NarEvent<R>,
    ) -> Result<(), Self::Error> {
        if self.state.is_some() {
            panic!("Sending when not ready!");
        }
        match item {
            NarEvent::File {
                name,
                executable,
                size: _,
                reader,
            } => {
                let name = if self.use_case_hack {
                    self.entries.hack_name(name)
                } else {
                    name
                };

                let path = join_name(&self.path, &name)?;
                let mut options = OpenOptions::new();
                options.write(true);
                options.create_new(true);
                #[cfg(unix)]
                {
                    if executable {
                        options.mode(0o777);
                    } else {
                        options.mode(0o666);
                    }
                }
                let handle = spawn_blocking(move || {
                    let reader = pin!(reader);
                    let mut reader = SyncIoBridge::new(reader);
                    let mut writer = options
                        .open(&path)
                        .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
                    loop {
                        trace!("Writing to file {:?}", path);
                        let buf = reader
                            .fill_buf()
                            .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
                        if buf.is_empty() {
                            break;
                        }
                        let amt = buf.len();
                        writer
                            .write_all(buf)
                            .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
                        reader.consume(amt);
                    }
                    writer
                        .flush()
                        .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
                    Ok(())
                });
                self.state = Some(handle);
            }
            NarEvent::Symlink { name, target } => {
                let name = if self.use_case_hack {
                    self.entries.hack_name(name)
                } else {
                    name
                };

                let path = join_name(&self.path, &name)?;
                let target_os = target
                    .to_os_str()
                    .map_err(|err| {
                        let lossy = target.to_os_str_lossy().into_owned();
                        let path = PathBuf::from(lossy);
                        NarWriteError::path_utf8_error(path, err)
                    })?
                    .to_owned();
                self.state = Some(spawn_blocking(move || {
                    #[cfg(unix)]
                    {
                        symlink(target_os, &path)
                            .map_err(|err| NarWriteError::create_symlink_error(path, err))
                    }
                }));
            }
            NarEvent::StartDirectory { name } => {
                let name = if self.use_case_hack {
                    let name = self.entries.hack_name(name);

                    #[allow(clippy::mutable_key_type)]
                    let entries = std::mem::take(&mut self.entries);
                    self.dir_stack.push(entries);
                    name
                } else {
                    name
                };

                let path = join_name(&self.path, &name)?;
                self.path = path;
                let path = self.path.clone();
                self.state = Some(spawn_blocking(|| {
                    let path = path;
                    create_dir(&path).map_err(|err| NarWriteError::create_dir_error(path, err))
                }));
            }
            NarEvent::EndDirectory => {
                if self.use_case_hack {
                    self.entries = self.dir_stack.pop().unwrap_or_default();
                }
                self.path.pop();
            }
        }
        Ok(())
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        <Self as Sink<NarEvent<R>>>::poll_ready(self, cx)
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        <Self as Sink<NarEvent<R>>>::poll_ready(self, cx)
    }
}

struct CIString(Bytes, String);

impl PartialEq for CIString {
    fn eq(&self, other: &Self) -> bool {
        self.1.eq(&other.1)
    }
}

impl fmt::Display for CIString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bstr = bstr::BStr::new(&self.0);
        write!(f, "{bstr}")
    }
}

impl Eq for CIString {}

impl std::hash::Hash for CIString {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.1.hash(state)
    }
}

#[derive(Default)]
struct Entries(HashMap<CIString, u32>);

impl Entries {
    fn hack_name(&mut self, name: Bytes) -> Bytes {
        use std::collections::hash_map::Entry;
        use std::io::Write;

        let lower = String::from_utf8_lossy(&name).to_lowercase();
        let ci_str = CIString(name.clone(), lower);
        match self.0.entry(ci_str) {
            Entry::Occupied(mut o) => {
                let b_name = bstr::BStr::new(&name);
                debug!("case collision between '{}' and '{}'", o.key(), b_name);
                let idx = o.get() + 1;
                let mut new_name = name.to_vec();
                write!(new_name, "{CASE_HACK_SUFFIX}{idx}").unwrap();
                o.insert(idx);
                Bytes::from(new_name)
            }
            Entry::Vacant(v) => {
                v.insert(0);
                name
            }
        }
    }
}

pub struct RestoreOptions {
    use_case_hack: bool,
}

impl RestoreOptions {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        let use_case_hack = true;
        #[cfg(not(target_os = "macos"))]
        let use_case_hack = false;
        Self { use_case_hack }
    }

    pub fn use_case_hack(mut self, use_case_hack: bool) -> Self {
        self.use_case_hack = use_case_hack;
        self
    }

    pub async fn restore<S, U, R, P>(self, stream: S, path: P) -> Result<(), NarWriteError>
    where
        S: Stream<Item = U>,
        U: Into<Result<NarEvent<R>, NarWriteError>>,
        P: Into<PathBuf>,
        R: AsyncBufRead + Send + 'static,
    {
        use futures::stream::StreamExt as _;
        let restorer = NarRestorer::new_restorer(path, self.use_case_hack);
        stream.map(|item| item.into()).forward(restorer).await
    }
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn restore<S, U, R, P>(stream: S, path: P) -> Result<(), NarWriteError>
where
    S: Stream<Item = U>,
    U: Into<Result<NarEvent<R>, NarWriteError>>,
    P: Into<PathBuf>,
    R: AsyncBufRead + Send + 'static,
{
    RestoreOptions::new().restore(stream, path).await
}

#[cfg(test)]
mod unittests {
    use super::*;
    use crate::archive::{NarEvent, dump, test_data};
    use futures::stream::{StreamExt as _, TryStreamExt as _, iter};
    use rstest::rstest;
    use tempfile::Builder;

    #[test_log::test(tokio::test)]
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
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");

        let events_s = iter(events.clone().into_iter())
            .map(|e| Ok(e) as Result<test_data::TestNarEvent, NarWriteError>);
        restore(events_s, &path).await.unwrap();

        let s = dump(path)
            .and_then(NarEvent::read_file)
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(s, events);
    }
}

#[cfg(test)]
mod proptests {
    use futures::stream::iter;
    use futures::{StreamExt as _, TryStreamExt as _};
    use proptest::proptest;
    use tempfile::tempdir;

    use crate::archive::{NarEvent, NarWriteError, dump, restore, test_data};
    use crate::pretty_prop_assert_eq;
    use crate::test::arbitrary::archive::arb_nar_events;

    #[test]
    fn proptest_restore_dump() {
        let r = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        proptest!(|(events in arb_nar_events(8, 256, 10))| {
            r.block_on(async {
                let dir = tempdir()?;
                let path = dir.path().join("output");

                let event_s = iter(events.clone().into_iter())
                    .map(|e| Ok(e) as Result<test_data::TestNarEvent, NarWriteError> );
                restore(event_s, &path).await.unwrap();

                let s = dump(path)
                    .and_then(NarEvent::read_file)
                    .try_collect::<test_data::TestNarEvents>().await?;
                pretty_prop_assert_eq!(&s, &events);
                Ok(())
            })?;

        });
    }
}
