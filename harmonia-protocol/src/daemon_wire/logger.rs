use std::cmp::min;
use std::fmt;
use std::future::Future;
use std::io::Cursor;
use std::marker::PhantomData;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll, ready};

use async_stream::stream;
use bytes::Bytes;
use futures::Stream;
use futures::stream::{Empty, empty};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::oneshot;
use tracing::trace;

use crate::daemon_wire::IgnoredZero;
use crate::daemon_wire::types::Operation;
use crate::de::{NixDeserialize as NixDeserializeTrait, NixRead};
use crate::log::{Activity, ActivityResult, LogMessage, Message, StopActivity, Verbosity};
use crate::ser::{NixSerialize as NixSerializeTrait, NixWrite, NixWriter};
use crate::types::{
    DaemonError, DaemonErrorKind, DaemonInt, DaemonResult, DaemonString, RemoteError,
};
use harmonia_protocol_derive::{NixDeserialize, NixSerialize};

pub const STDERR_LAST: u64 = 0x616c7473; // 'alts' in ASCII
pub const STDERR_ERROR: u64 = 0x63787470; // 'cxtp' in ASCII
pub const STDERR_NEXT: u64 = 0x6f6c6d67; // 'olmg' in ASCII
pub const STDERR_READ: u64 = 0x64617461; // 'data' in ASCII
pub const STDERR_WRITE: u64 = 0x64617416;
pub const STDERR_START_ACTIVITY: u64 = 0x53545254; // 'STRT' in ASCII
pub const STDERR_STOP_ACTIVITY: u64 = 0x53544f50; // 'STOP' in ASCII
pub const STDERR_RESULT: u64 = 0x52534c54; // 'RSLT' in ASCII

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    TryFromPrimitive,
    IntoPrimitive,
    NixDeserialize,
    NixSerialize,
)]
#[nix(try_from = "u64", into = "u64")]
#[repr(u64)]
pub enum RawLogMessageType {
    Last = STDERR_LAST,
    Error = STDERR_ERROR,
    Next = STDERR_NEXT,
    Read = STDERR_READ,
    Write = STDERR_WRITE,
    StartActivity = STDERR_START_ACTIVITY,
    StopActivity = STDERR_STOP_ACTIVITY,
    Result = STDERR_RESULT,
}

#[derive(Debug)]
//#[cfg_attr(feature = "nixrs-derive", nix(tag = "RawLogMessageType"))]
pub enum RawLogMessage {
    Last,
    Error(LogError),
    Next(DaemonString), // If logger is JSON, invalid UTF-8 is replaced with U+FFFD
    Read(usize),
    Write(Bytes),
    StartActivity(Activity),
    StopActivity(StopActivity),
    Result(ActivityResult),
}

impl crate::daemon::de::NixDeserialize for RawLogMessage {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + crate::daemon::de::NixRead + Send,
    {
        use tracing::trace;

        if let Some(tag) = reader.try_read_value::<RawLogMessageType>().await? {
            trace!(?tag, "Read log message tag");
            match tag {
                RawLogMessageType::Last => Ok(Some(RawLogMessage::Last)),
                RawLogMessageType::Error => {
                    Ok(Some(RawLogMessage::Error(reader.read_value().await?)))
                }
                RawLogMessageType::Next => {
                    Ok(Some(RawLogMessage::Next(reader.read_value().await?)))
                }
                RawLogMessageType::Read => {
                    Ok(Some(RawLogMessage::Read(reader.read_value().await?)))
                }
                RawLogMessageType::Write => {
                    Ok(Some(RawLogMessage::Write(reader.read_value().await?)))
                }
                RawLogMessageType::StartActivity => Ok(Some(RawLogMessage::StartActivity(
                    reader.read_value().await?,
                ))),
                RawLogMessageType::StopActivity => Ok(Some(RawLogMessage::StopActivity(
                    reader.read_value().await?,
                ))),
                RawLogMessageType::Result => {
                    Ok(Some(RawLogMessage::Result(reader.read_value().await?)))
                }
            }
        } else {
            Ok(None)
        }
    }
}

impl NixSerializeTrait for RawLogMessage {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        use RawLogMessageType::*;
        match self {
            RawLogMessage::Last => writer.write_value(&Last).await?,
            RawLogMessage::Error(err) => {
                writer.write_value(&Error).await?;
                writer.write_value(err).await?;
            }
            RawLogMessage::Next(msg) => {
                writer.write_value(&Next).await?;
                writer.write_value(msg).await?;
            }
            RawLogMessage::Read(len) => {
                writer.write_value(&Read).await?;
                writer.write_value(len).await?;
            }
            RawLogMessage::Write(buf) => {
                writer.write_value(&Write).await?;
                writer.write_value(buf).await?;
            }
            RawLogMessage::StartActivity(act) => {
                if writer.version().minor() >= 20 {
                    writer.write_value(&StartActivity).await?;
                    writer.write_value(act).await?;
                } else {
                    writer.write_value(&Next).await?;
                    writer.write_value(&act.text).await?;
                }
            }
            RawLogMessage::StopActivity(act) => {
                if writer.version().minor() >= 20 {
                    writer.write_value(&StopActivity).await?;
                    writer.write_value(act).await?;
                }
            }
            RawLogMessage::Result(result) => {
                if writer.version().minor() >= 20 {
                    writer.write_value(&Result).await?;
                    writer.write_value(result).await?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct TraceLine {
    _have_pos: IgnoredZero,
    pub hint: DaemonString, // If logger is JSON, invalid UTF-8 is replaced with U+FFFD
}

fn default_exit_status() -> DaemonInt {
    1
}

#[derive(Debug, NixDeserialize, NixSerialize)]
pub struct LogError {
    #[nix(version = "26..")]
    _ty: IgnoredErrorType,
    #[nix(version = "26..")]
    pub level: Verbosity,
    #[nix(version = "26..")]
    _name: IgnoredErrorType,
    pub msg: DaemonString, // If logger is JSON, invalid UTF-8 is replaced with U+FFFD
    #[nix(version = "..=25", default = "default_exit_status")]
    pub exit_status: DaemonInt,
    #[nix(version = "26..")]
    _have_pos: IgnoredZero,
    #[nix(version = "26..")]
    pub traces: Vec<TraceLine>,
}

impl From<RemoteError> for LogError {
    fn from(value: RemoteError) -> Self {
        LogError {
            level: value.level,
            msg: value.msg,
            exit_status: value.exit_status,
            traces: value.traces,
            _ty: IgnoredErrorType,
            _name: IgnoredErrorType,
            _have_pos: IgnoredZero,
        }
    }
}

impl From<DaemonError> for LogError {
    fn from(value: DaemonError) -> Self {
        match value.kind().clone() {
            DaemonErrorKind::Remote(remote_error) => remote_error.into(),
            _ => {
                let msg = value.to_string().into_bytes().into();
                LogError {
                    msg,
                    level: Verbosity::Error,
                    exit_status: 1,
                    traces: Vec::new(),
                    _ty: IgnoredErrorType,
                    _name: IgnoredErrorType,
                    _have_pos: IgnoredZero,
                }
            }
        }
    }
}

impl From<LogError> for RemoteError {
    fn from(err: LogError) -> Self {
        RemoteError {
            level: err.level,
            msg: err.msg,
            traces: err.traces,
            exit_status: err.exit_status,
        }
    }
}

#[derive(Debug, Default, NixDeserialize, NixSerialize)]
#[nix(from_str, display)]
pub struct IgnoredErrorType;

impl fmt::Display for IgnoredErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Error")
    }
}

impl FromStr for IgnoredErrorType {
    type Err = String;
    fn from_str(_: &str) -> Result<Self, Self::Err> {
        Ok(IgnoredErrorType)
    }
}

pub trait ResultLog: Stream<Item = LogMessage> + Future {}
impl<RL> ResultLog for RL where RL: Stream<Item = LogMessage> + Future {}

pin_project! {
    struct MapOkResult<L, F, T> {
        #[pin]
        result: L,
        mapper: Option<F>,
        value: PhantomData<T>,
    }
}
impl<L, F, T> Stream for MapOkResult<L, F, T>
where
    L: Stream<Item = LogMessage>,
{
    type Item = LogMessage;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().result.poll_next(cx)
    }
}
impl<L, F, T, T2, E> Future for MapOkResult<L, F, T>
where
    L: Future<Output = Result<T, E>>,
    F: FnOnce(T) -> T2 + Send,
    T: Send,
{
    type Output = Result<T2, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = self.project();
        Poll::Ready(ready!(me.result.poll(cx)).map(|value| (me.mapper.take().unwrap())(value)))
    }
}

pin_project! {
    struct MapErrResult<L, F, E> {
        #[pin]
        result: L,
        mapper: Option<F>,
        value: PhantomData<E>,
    }
}
impl<L, F, E> Stream for MapErrResult<L, F, E>
where
    L: Stream<Item = LogMessage>,
{
    type Item = LogMessage;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().result.poll_next(cx)
    }
}
impl<L, F, T, E, E2> Future for MapErrResult<L, F, E>
where
    L: Future<Output = Result<T, E>>,
    F: FnOnce(E) -> E2 + Send,
    E: Send,
{
    type Output = Result<T, E2>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = self.project();
        Poll::Ready(ready!(me.result.poll(cx)).map_err(|err| (me.mapper.take().unwrap())(err)))
    }
}

pin_project! {
    #[project = AndThenLogResultProj]
    enum AndThenLogResult<F, R> {
        First {
            mapper: Option<F>,
        },
        Second {
            #[pin]
            result: R
        },
    }
}

pin_project! {
    struct AndThenLog<L, F, R> {
        #[pin]
        stream: L,
        #[pin]
        result: AndThenLogResult<F, R>
    }
}

impl<L, F, R> Stream for AndThenLog<L, F, R>
where
    L: Stream<Item = LogMessage>,
{
    type Item = LogMessage;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().stream.poll_next(cx)
    }
}

impl<L, F, R, T, T2, E> Future for AndThenLog<L, F, R>
where
    L: Future<Output = Result<T, E>> + Stream<Item = LogMessage>,
    F: FnOnce(T) -> R,
    R: Future<Output = Result<T2, E>>,
{
    type Output = Result<T2, E>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let mut me = self.as_mut().project();
            match me.result.as_mut().project() {
                AndThenLogResultProj::First { mapper } => {
                    let res = ready!(me.stream.poll(cx))?;
                    let mapper = mapper.take().unwrap();
                    let result = (mapper)(res);
                    me.result.set(AndThenLogResult::Second { result });
                }
                AndThenLogResultProj::Second { result } => {
                    return result.poll(cx);
                }
            }
        }
    }
}

pub trait ResultLogExt: ResultLog {
    fn map_ok<F, E, T, T2>(self, f: F) -> impl ResultLog<Output = Result<T2, E>>
    where
        Self: ResultLog<Output = Result<T, E>>,
        F: FnOnce(T) -> T2 + Send,
        T: Send,
        T2: 'static;
    fn map_err<F, E, T, E2>(self, f: F) -> impl ResultLog<Output = Result<T, E2>>
    where
        Self: ResultLog<Output = Result<T, E>>,
        F: FnOnce(E) -> E2 + Send,
        E: Send,
        E2: 'static;
    fn and_then<F, E, T, T2, Fut>(self, f: F) -> impl ResultLog<Output = Result<T2, E>>
    where
        Self: ResultLog<Output = Result<T, E>>,
        F: FnOnce(T) -> Fut + Send,
        Fut: Future<Output = Result<T2, E>> + Send,
        T2: 'static;
    fn fill_operation<T>(self, op: Operation) -> impl ResultLog<Output = Result<T, DaemonError>>
    where
        Self: ResultLog<Output = Result<T, DaemonError>>;
    fn drive_result<D, T, E>(self, driver: D) -> DriveResult<Self, D, E>
    where
        Self: ResultLog<Output = Result<T, E>> + Sized,
        D: Future<Output = Result<(), E>>;
    fn boxed_result<'a>(self) -> Pin<Box<dyn ResultLog<Output = Self::Output> + Send + 'a>>
    where
        Self: Send + 'a;
    fn boxed_local_result<'a>(self) -> Pin<Box<dyn ResultLog<Output = Self::Output> + 'a>>
    where
        Self: 'a;
}

impl<L> ResultLogExt for L
where
    L: ResultLog,
{
    fn map_ok<F, E, T, T2>(self, f: F) -> impl ResultLog<Output = Result<T2, E>>
    where
        Self: ResultLog<Output = Result<T, E>>,
        F: FnOnce(T) -> T2 + Send,
        T: Send,
        T2: 'static,
    {
        MapOkResult {
            result: self,
            mapper: Some(f),
            value: PhantomData,
        }
    }

    fn map_err<F, E, T, E2>(self, f: F) -> impl ResultLog<Output = Result<T, E2>>
    where
        Self: ResultLog<Output = Result<T, E>>,
        F: FnOnce(E) -> E2 + Send,
        E: Send,
        E2: 'static,
    {
        MapErrResult {
            result: self,
            mapper: Some(f),
            value: PhantomData,
        }
    }
    fn and_then<F, E, T, T2, Fut>(self, f: F) -> impl ResultLog<Output = Result<T2, E>>
    where
        Self: ResultLog<Output = Result<T, E>>,
        F: FnOnce(T) -> Fut + Send,
        Fut: Future<Output = Result<T2, E>> + Send,
        T2: 'static,
    {
        AndThenLog {
            stream: self,
            result: AndThenLogResult::First { mapper: Some(f) },
        }
    }
    fn drive_result<D, T, E>(self, driver: D) -> DriveResult<Self, D, E>
    where
        Self: ResultLog<Output = Result<T, E>> + Sized,
        D: Future<Output = Result<(), E>>,
    {
        DriveResult {
            result: self,
            driver,
            driving: true,
            drive_err: None,
        }
    }

    fn fill_operation<T>(self, op: Operation) -> impl ResultLog<Output = Result<T, DaemonError>>
    where
        Self: ResultLog<Output = Result<T, DaemonError>>,
    {
        self.map_err(move |err| err.fill_operation(op))
    }
    fn boxed_result<'a>(self) -> Pin<Box<dyn ResultLog<Output = Self::Output> + Send + 'a>>
    where
        Self: Send + 'a,
    {
        Box::pin(self)
    }
    fn boxed_local_result<'a>(self) -> Pin<Box<dyn ResultLog<Output = Self::Output> + 'a>>
    where
        Self: 'a,
    {
        Box::pin(self)
    }
}

pin_project! {
    #[project = FutureResultProj]
    #[derive(Default)]
    pub enum FutureResult<Fut, T, E> {
        Later {
            #[pin]
            fut: Fut,
        },
        ResolvedOk {
            #[pin]
            result: T
        },
        ResolvedErr {
            err: Option<E>,
        },
        #[default]
        Invalid,
    }
}

impl<Fut, R, E> FutureResult<Fut, R, E>
where
    Fut: Future<Output = Result<R, E>>,
{
    pub fn new(fut: Fut) -> Self {
        Self::Later { fut }
    }

    fn poll_resolved(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Pin<&mut R>, ()>> {
        match self.as_mut().project() {
            FutureResultProj::Later { fut } => match ready!(fut.poll(cx)) {
                Ok(result) => {
                    self.set(FutureResult::ResolvedOk { result });
                }
                Err(err) => {
                    self.set(FutureResult::ResolvedErr { err: Some(err) });
                }
            },
            FutureResultProj::ResolvedOk { result: _ } => {}
            FutureResultProj::ResolvedErr { err: _ } => {}
            FutureResultProj::Invalid => {}
        }
        match self.project() {
            FutureResultProj::Later { fut: _ } => unreachable!(),
            FutureResultProj::ResolvedOk { result } => Poll::Ready(Ok(result)),
            FutureResultProj::ResolvedErr { err: _ } => Poll::Ready(Err(())),
            FutureResultProj::Invalid => unreachable!(),
        }
    }
}

impl<Fut, R, E> Stream for FutureResult<Fut, R, E>
where
    Fut: Future<Output = Result<R, E>>,
    R: Stream<Item = LogMessage>,
{
    type Item = LogMessage;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Ok(res) = ready!(self.poll_resolved(cx)) {
            res.poll_next(cx)
        } else {
            Poll::Ready(None)
        }
    }
}

impl<Fut, R, T, E> Future for FutureResult<Fut, R, E>
where
    Fut: Future<Output = Result<R, E>>,
    R: Future<Output = Result<T, E>>,
{
    type Output = Result<T, E>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Ok(res) = ready!(self.as_mut().poll_resolved(cx)) {
            res.poll(cx)
        } else {
            match self.project() {
                FutureResultProj::Later { fut: _ } => unreachable!(),
                FutureResultProj::ResolvedOk { result } => result.poll(cx),
                FutureResultProj::ResolvedErr { err } => {
                    Poll::Ready(Err(err.take().expect("Polling invalid FutureRessult")))
                }
                _ => panic!("Polling invalid FutureRessult"),
            }
        }
    }
}

pub trait FutureResultExt: Future {
    /// Create a [ResultLog] with an empty log from a [Future]
    ///
    /// This takes any [Future] and makes it into a [ResultLog] implementation
    /// that just returns [Option::None] for the [Stream] part while
    /// [Future::Output] is used as the output for the [ResultLog].
    ///
    /// ```rust
    /// # use std::future::{ready, Future};
    /// # use std::pin::pin;
    /// # use futures::stream::StreamExt as _;
    /// use harmonia_protocol::daemon::FutureResultExt as _;
    /// # tokio_test::block_on(async {
    /// let result = ready(12).empty_logs();
    /// let mut rp = pin!(result);
    /// assert!(rp.next().await.is_none());
    /// assert_eq!(rp.await, 12);
    /// # })
    /// ```
    fn empty_logs(self) -> impl ResultLog<Output = Self::Output>;

    /// Create a [ResultLog] with the provided logs and with the output from
    /// the [Future]
    ///
    ///
    fn with_logs<S>(self, logs: S) -> impl ResultLog<Output = Self::Output>
    where
        S: Stream<Item = LogMessage>;

    /// Flattens a [Future] that outputs a [ResultLog] to a [ResultLog]
    ///
    /// ```rust
    /// # use std::future::{ready, Future};
    /// # use std::pin::pin;
    /// # use futures::stream::StreamExt as _;
    /// # use harmonia_protocol::daemon::DaemonResult;
    /// use harmonia_protocol::daemon::FutureResultExt as _;
    /// # tokio_test::block_on(async {
    /// let result = async {
    ///     Ok(ready(Ok(12) as DaemonResult<i32>).empty_logs())
    /// }.future_result();
    /// let mut rp = pin!(result);
    /// assert!(rp.next().await.is_none());
    /// assert_eq!(rp.await.unwrap(), 12);
    /// # })
    /// ```
    fn future_result<R, T, E>(self) -> impl ResultLog<Output = Result<T, E>>
    where
        Self: Future<Output = Result<R, E>>,
        R: ResultLog<Output = Result<T, E>>;
}

impl<F> FutureResultExt for F
where
    F: Future,
{
    fn empty_logs(self) -> impl ResultLog<Output = Self::Output> {
        ResultProcess::empty(self)
    }

    fn with_logs<S>(self, logs: S) -> impl ResultLog<Output = Self::Output>
    where
        S: Stream<Item = LogMessage>,
    {
        ResultProcess {
            stream: logs,
            result: self,
        }
    }

    fn future_result<R, T, E>(self) -> impl ResultLog<Output = Result<T, E>>
    where
        Self: Future<Output = Result<R, E>>,
        R: ResultLog<Output = Result<T, E>>,
    {
        FutureResult::new(self)
    }
}

pub trait ReadResult<R, W, SR, SW, T>: Sized {
    fn read_result(
        self,
        result: Result<(), DaemonError>,
        reader: R,
        writer: Option<W>,
        source: Option<SR>,
        sink: Option<SW>,
    ) -> impl Future<Output = DaemonResult<T>> + Send;
}

impl<R, W, SR, SW, T> ReadResult<R, W, SR, SW, T> for ()
where
    T: NixDeserializeTrait,
    R: NixRead + AsyncRead + fmt::Debug + Unpin + Send,
    DaemonError: From<<R as NixRead>::Error>,
    W: Send,
    SR: Send,
    SW: Send,
{
    async fn read_result(
        self,
        result: Result<(), DaemonError>,
        mut reader: R,
        _writer: Option<W>,
        _source: Option<SR>,
        _sink: Option<SW>,
    ) -> DaemonResult<T> {
        match result {
            Err(err) => Err(err),
            Ok(_) => Ok(reader.read_value().await?),
        }
    }
}

pub struct ResultFn<F, FFut> {
    f: F,
    _res: PhantomData<fn(FFut)>,
}

impl<R, W, SR, SW, T, F, FFut> ReadResult<R, W, SR, SW, T> for ResultFn<F, FFut>
where
    F: FnOnce(Result<(), DaemonError>, R, Option<W>, Option<SR>, Option<SW>) -> FFut + Send,
    FFut: Future<Output = DaemonResult<T>> + Send,
    R: Send,
    W: Send,
    SR: Send,
    SW: Send,
{
    async fn read_result(
        self,
        result: Result<(), DaemonError>,
        reader: R,
        writer: Option<W>,
        source: Option<SR>,
        sink: Option<SW>,
    ) -> DaemonResult<T> {
        (self.f)(result, reader, writer, source, sink).await
    }
}

pub struct ProcessStderr<R, W, SR, SW, T, TFut> {
    result: Option<Result<(), DaemonError>>,
    reader: R,
    writer: Option<W>,
    source: Option<SR>,
    sink: Option<SW>,
    read_result: TFut,
    _result_type: PhantomData<T>,
}

impl<R, T> ProcessStderr<R, NixWriter<Cursor<Vec<u8>>>, Cursor<Vec<u8>>, Cursor<Vec<u8>>, T, ()> {
    pub fn new(reader: R) -> Self {
        ProcessStderr {
            reader,
            writer: None,
            source: None,
            sink: None,
            result: None,
            read_result: (),
            _result_type: PhantomData,
        }
    }
}

impl<R, W, SR, SW, T, TFut> ProcessStderr<R, W, SR, SW, T, TFut> {
    pub fn with_source<NW, NSR>(
        self,
        writer: NW,
        source: NSR,
    ) -> ProcessStderr<R, NW, NSR, SW, T, TFut> {
        ProcessStderr {
            result: self.result,
            reader: self.reader,
            writer: Some(writer),
            source: Some(source),
            sink: self.sink,
            read_result: self.read_result,
            _result_type: PhantomData,
        }
    }
    /*
    pub fn with_sink<NSW>(self, sink: NSW) -> ProcessStderr<R, W, SR, NSW, T, TFut> {
        ProcessStderr {
            result: self.result,
            reader: self.reader,
            writer: self.writer,
            source: self.source,
            sink: Some(sink),
            read_result: self.read_result,
            _result_type: PhantomData,
        }
    }
     */

    pub fn result_fn<F, FFut>(self, f: F) -> ProcessStderr<R, W, SR, SW, T, ResultFn<F, FFut>>
    where
        F: FnOnce(Result<(), DaemonError>, R, Option<W>, Option<SR>, Option<SW>) -> FFut,
        FFut: Future<Output = DaemonResult<T>>,
    {
        ProcessStderr {
            result: self.result,
            reader: self.reader,
            writer: self.writer,
            source: self.source,
            sink: self.sink,
            read_result: ResultFn {
                f,
                _res: PhantomData,
            },
            _result_type: PhantomData,
        }
    }
}

impl<R, W, SR, SW, T, TFut> ProcessStderr<R, W, SR, SW, T, TFut>
where
    W: NixWrite + AsyncWrite + fmt::Debug + Unpin + Send,
    DaemonError: From<<W as NixWrite>::Error>,
    SR: AsyncBufRead + Unpin + Send,
{
    async fn process_read(&mut self, len: usize) -> Result<(), DaemonError> {
        if let Some(source) = self.source.as_mut() {
            let buf = source.fill_buf().await?;
            let writer = self.writer.as_mut().unwrap();
            let len = min(len, buf.len());
            writer.write_slice(&buf[..len]).await?;
            writer.flush().await?;
            source.consume(len);
            Ok(())
        } else {
            Err(DaemonErrorKind::NoSourceForLoggerRead.into())
        }
    }
}

pin_project! {
    pub struct ResultProcess<S, R> {
        #[pin]
        pub stream: S,
        #[pin]
        pub result: R,
    }
}
impl<R> ResultProcess<Empty<LogMessage>, R> {
    pub fn empty(result: R) -> Self {
        Self {
            stream: empty(),
            result,
        }
    }
}

impl<S, R> Stream for ResultProcess<S, R>
where
    S: Stream<Item = LogMessage>,
{
    type Item = LogMessage;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.project().stream.poll_next(cx)
    }
}

impl<S, R> Future for ResultProcess<S, R>
where
    S: Stream<Item = LogMessage>,
    R: Future,
{
    type Output = R::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let mut me = self.project();
        while ready!(me.stream.as_mut().poll_next(cx)).is_some() {}
        me.result.poll(cx)
    }
}

pin_project! {
    pub struct DriveResult<R, D, E> {
        #[pin]
        pub result: R,
        #[pin]
        pub driver: D,
        pub driving: bool,
        pub drive_err: Option<E>,
    }
}

impl<R, D, E> DriveResult<R, D, E>
where
    D: Future<Output = Result<(), E>>,
{
    pub fn new(result: R, driver: D) -> Self {
        Self {
            result,
            driver,
            driving: true,
            drive_err: None,
        }
    }

    fn drive(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) {
        let me = self.project();
        if *me.driving
            && let Poll::Ready(res) = me.driver.poll(cx)
        {
            *me.driving = false;
            *me.drive_err = res.err();
        }
    }
    fn take_drive(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Result<(), E> {
        self.as_mut().drive(cx);
        let me = self.project();
        if let Some(err) = me.drive_err.take() {
            Err(err)
        } else {
            Ok(())
        }
    }
}

impl<R, D, E> Stream for DriveResult<R, D, E>
where
    R: Stream<Item = LogMessage>,
    D: Future<Output = Result<(), E>>,
{
    type Item = LogMessage;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.as_mut().drive(cx);
        let me = self.project();
        if me.drive_err.is_some() {
            return Poll::Ready(None);
        }
        me.result.poll_next(cx)
    }
}

impl<R, D, T, E> Future for DriveResult<R, D, E>
where
    R: Future<Output = Result<T, E>>,
    D: Future<Output = Result<(), E>>,
{
    type Output = Result<T, E>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        self.as_mut().take_drive(cx)?;
        self.project().result.poll(cx)
    }
}

impl<R, D> AsyncRead for DriveResult<R, D, std::io::Error>
where
    R: AsyncRead,
    D: Future<Output = Result<(), std::io::Error>>,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.as_mut().take_drive(cx)?;
        self.project().result.poll_read(cx, buf)
    }
}

impl<R, D> AsyncBufRead for DriveResult<R, D, std::io::Error>
where
    R: AsyncBufRead,
    D: Future<Output = Result<(), std::io::Error>>,
{
    fn poll_fill_buf(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<&[u8]>> {
        self.as_mut().take_drive(cx)?;
        self.project().result.poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        self.project().result.consume(amt);
    }
}

impl<R, W, SR, SW, T, TFut> ProcessStderr<R, W, SR, SW, T, TFut>
where
    R: NixRead + AsyncRead + fmt::Debug + Unpin + Send,
    W: NixWrite + AsyncWrite + fmt::Debug + Unpin + Send,
    DaemonError: From<<W as NixWrite>::Error> + From<<R as NixRead>::Error>,
    SR: AsyncBufRead + Unpin + Send,
    SW: AsyncWrite + fmt::Debug + Unpin + Send,
    TFut: ReadResult<R, W, SR, SW, T> + Send,
    T: Send,
{
    pub fn stream(mut self) -> impl Stream<Item = LogMessage> + Future<Output = DaemonResult<T>> {
        let (sender, receiver) = oneshot::channel();
        ResultProcess {
            stream: stream! {
                loop {
                    trace!("Reading log message");
                    let msg = self.reader.read_value::<RawLogMessage>().await;
                    trace!(?msg, "Got log message");
                    match msg {
                        Ok(RawLogMessage::Next(text)) => {
                            yield LogMessage::Message(Message {
                                text,
                                level: Verbosity::Error,
                            });
                        }
                        Ok(RawLogMessage::Result(result)) => {
                            yield LogMessage::Result(result);
                        }
                        Ok(RawLogMessage::StartActivity(act)) => {
                            yield LogMessage::StartActivity(act);
                        }
                        Ok(RawLogMessage::StopActivity(act)) => {
                            yield LogMessage::StopActivity(act);
                        }
                        Ok(RawLogMessage::Read(len)) => {
                            if let Err(err) = self.process_read(len).await {
                                self.result = Some(Err(err));
                                break;
                            }
                        }
                        Ok(RawLogMessage::Write(buf)) => {
                            if let Some(sink) = self.sink.as_mut()
                                && let Err(err) = sink.write_all(&buf).await {
                                    self.result = Some(Err(DaemonError::from(err)));
                                    break;
                                }
                        }
                        Ok(RawLogMessage::Last) => {
                            trace!("Reading result");
                            let value = self.reader.read_value().await;
                            trace!("Read result");
                            self.result = Some(value.map_err(|err| err.into()));
                            break;
                        }
                        Ok(RawLogMessage::Error(err)) => {
                            self.result = Some(Err(err.into()));
                            break;
                        }
                        Err(err) => {
                            self.result = Some(Err(DaemonError::from(err)));
                            break;
                        }
                    }
                }
                let _ = sender.send(self);
            },
            result: async {
                let process = receiver.await.unwrap();
                process
                    .read_result
                    .read_result(
                        process.result.unwrap(),
                        process.reader,
                        process.writer,
                        process.source,
                        process.sink,
                    )
                    .await
            },
        }
    }
}
