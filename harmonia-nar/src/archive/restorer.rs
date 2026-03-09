use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use bstr::ByteSlice as _;
use bytes::Bytes;
use derive_more::Display;
use futures::Stream;
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWriteExt as _};
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

pub struct NarRestorer {
    path: PathBuf,
    use_case_hack: bool,
    entries: Entries,
    dir_stack: Vec<Entries>,
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
            path,
            use_case_hack,
            entries: Default::default(),
            dir_stack: Default::default(),
        }
    }

    /// Process a single NAR event, writing to the filesystem.
    async fn process_event<R>(&mut self, event: NarEvent<R>) -> Result<(), NarWriteError>
    where
        R: AsyncBufRead + Unpin,
    {
        match event {
            NarEvent::File {
                name,
                executable,
                size: _,
                mut reader,
            } => {
                let name = if self.use_case_hack {
                    self.entries.hack_name(name)
                } else {
                    name
                };

                let path = join_name(&self.path, &name)?;
                let mut options = tokio::fs::OpenOptions::new();
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
                let mut file = options
                    .open(&path)
                    .await
                    .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
                loop {
                    trace!("Writing to file {:?}", path);
                    let buf = reader
                        .fill_buf()
                        .await
                        .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
                    if buf.is_empty() {
                        break;
                    }
                    let amt = buf.len();
                    file.write_all(buf)
                        .await
                        .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
                    reader.consume(amt);
                }
                file.flush()
                    .await
                    .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
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
                #[cfg(unix)]
                {
                    tokio::fs::symlink(target_os, &path)
                        .await
                        .map_err(|err| NarWriteError::create_symlink_error(path, err))?;
                }
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
                tokio::fs::create_dir(&path)
                    .await
                    .map_err(|err| NarWriteError::create_dir_error(path, err))?;
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

    /// Consume a stream of NAR events and restore them to the filesystem.
    pub async fn restore<S, U, R>(mut self, stream: S) -> Result<(), NarWriteError>
    where
        S: Stream<Item = U>,
        U: Into<Result<NarEvent<R>, NarWriteError>>,
        R: AsyncBufRead + Send + Unpin,
    {
        use futures::StreamExt as _;
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            let event = item.into()?;
            self.process_event(event).await?;
        }
        Ok(())
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
        R: AsyncBufRead + Send + Unpin,
    {
        let restorer = NarRestorer::new_restorer(path, self.use_case_hack);
        restorer.restore(stream).await
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
    R: AsyncBufRead + Send + Unpin,
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
    use crate::test::arbitrary::archive::arb_nar_events;
    use proptest::prop_assert_eq;

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
                prop_assert_eq!(&s, &events);
                Ok(())
            })?;

        });
    }
}
