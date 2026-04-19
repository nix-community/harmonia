//! App-wide zstd response compression.
//!
//! actix-web's `Compress` middleware hard-codes zstd level 3 and exposes no
//! tuning. NAR responses have two properties it can't exploit: the
//! uncompressed size is known up front, and large NARs contain long-range
//! repetition (near-duplicate ELF sections, vendored sources) beyond the
//! ~2 MiB level-3 window. Pledging the size and enabling long-distance
//! matching at level 1 is both smaller and faster on representative
//! closures, so this middleware replaces `Compress` entirely.
//!
//! Chunks above [`INLINE_THRESHOLD`] are compressed on the blocking pool so
//! a single large NAR can't stall the reactor.

use std::future::{Ready, ready as fut_ready};
use std::io::{self, Write};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, ready};

use actix_web::Error;
use actix_web::body::{BodySize, MessageBody};
use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready};
use actix_web::http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, HeaderMap, HeaderValue, VARY};
use actix_web::rt::task::{JoinHandle, spawn_blocking};
use actix_web::web::Bytes;
use bytes::BytesMut;
use futures_core::{Stream, future::LocalBoxFuture};

use crate::config::ZstdConfig;

/// LDM would otherwise raise windowLog to 27 (128 MiB decoder buffer); 25
/// keeps almost all of the ratio at 32 MiB.
const LDM_WINDOW_LOG_CAP: u32 = 25;

/// Responses smaller than this stay uncompressed; the zstd frame header and
/// chunked-encoding overhead would erase any win.
const MIN_COMPRESS_SIZE: u64 = 256;

/// Body chunks below this size are encoded on the reactor thread; the
/// `spawn_blocking` round-trip would dominate the actual compression work.
const INLINE_THRESHOLD: usize = 1024;

type ZstdEncoder = zstd::stream::write::Encoder<'static, Writer>;

struct Writer {
    buf: BytesMut,
}

impl Writer {
    fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(8 * 1024),
        }
    }
    fn take(&mut self) -> Bytes {
        self.buf.split().freeze()
    }
}

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Minimal `Accept-Encoding` check. We only need to know whether the client
/// will accept zstd at all; quality ranking against other codings is
/// irrelevant because zstd is the only on-the-fly encoding we offer.
fn accepts_zstd(headers: &HeaderMap) -> bool {
    headers
        .get_all(ACCEPT_ENCODING)
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|tok| {
            let mut parts = tok.trim().split(';');
            if !parts
                .next()
                .is_some_and(|c| c.trim().eq_ignore_ascii_case("zstd"))
            {
                return false;
            }
            // Honour an explicit q=0 / Q=0 opt-out among any parameters.
            parts
                .filter_map(|p| {
                    p.trim()
                        .strip_prefix("q=")
                        .or_else(|| p.trim().strip_prefix("Q="))
                })
                .find_map(|q| q.trim().parse::<f32>().ok())
                .is_none_or(|q| q > 0.0)
        })
}

fn build_encoder(cfg: &ZstdConfig, pledged_size: Option<u64>) -> io::Result<ZstdEncoder> {
    use zstd_safe::CParameter;

    let mut enc = ZstdEncoder::new(Writer::new(), cfg.level)?;
    if let Some(size) = pledged_size {
        enc.set_pledged_src_size(Some(size))?;
    }
    if cfg.long_distance_matching {
        enc.set_parameter(CParameter::EnableLongDistanceMatching(true))?;
    }
    let window_log = match cfg.window_log {
        0 if cfg.long_distance_matching => LDM_WINDOW_LOG_CAP,
        n => n,
    };
    if window_log != 0 {
        enc.set_parameter(CParameter::WindowLog(window_log))?;
    }
    Ok(enc)
}

/// View a `MessageBody` as a `Stream<Item = io::Result<Bytes>>` so the same
/// `ZstdBody` poll loop works for both NAR streams and in-memory responses.
pub(crate) struct BodyAsStream<B>(B);

impl<B> Stream for BodyAsStream<B>
where
    B: MessageBody + Unpin,
{
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().0).poll_next(cx).map_err(|e| {
            // `MessageBody::Error` only guarantees `Into<Box<dyn Error>>`,
            // which `io::Error` won't accept without `Send + Sync`; flatten
            // to a string so the encoder pipeline stays `io::Error`-typed.
            io::Error::other(e.into().to_string())
        })
    }
}

enum State {
    /// Encoder is parked; ready to accept the next input chunk or to finish.
    Idle(Box<ZstdEncoder>),
    /// Encoder is on the blocking pool consuming a chunk. Returns the encoder
    /// and any compressed bytes produced.
    Encoding(JoinHandle<io::Result<(Box<ZstdEncoder>, Bytes)>>),
    /// Input exhausted; flushing the final frame on the blocking pool.
    Finishing(JoinHandle<io::Result<Bytes>>),
    Done,
}

/// Adapts a `Stream<Bytes>` into a zstd-encoded `Stream<Bytes>`.
pub(crate) struct ZstdBody<S> {
    inner: S,
    state: State,
}

impl<S> ZstdBody<S> {
    fn new(inner: S, enc: Box<ZstdEncoder>) -> Self {
        Self {
            inner,
            state: State::Idle(enc),
        }
    }
}

impl<S> Stream for ZstdBody<S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match &mut this.state {
                State::Done => return Poll::Ready(None),

                State::Finishing(handle) => {
                    let chunk = ready!(Pin::new(handle).poll(cx))
                        .map_err(|e| io::Error::other(e.to_string()))??;
                    this.state = State::Done;
                    if chunk.is_empty() {
                        return Poll::Ready(None);
                    }
                    return Poll::Ready(Some(Ok(chunk)));
                }

                State::Encoding(handle) => {
                    let (enc, chunk) = ready!(Pin::new(handle).poll(cx))
                        .map_err(|e| io::Error::other(e.to_string()))??;
                    this.state = State::Idle(enc);
                    if !chunk.is_empty() {
                        return Poll::Ready(Some(Ok(chunk)));
                    }
                    // Output buffer still empty (encoder buffered internally);
                    // pull the next input chunk.
                }

                State::Idle(_) => match ready!(Pin::new(&mut this.inner).poll_next(cx)) {
                    Some(Ok(chunk)) => {
                        let State::Idle(mut enc) = std::mem::replace(&mut this.state, State::Done)
                        else {
                            unreachable!()
                        };
                        if chunk.len() < INLINE_THRESHOLD {
                            if let Err(e) = enc.write_all(&chunk) {
                                return Poll::Ready(Some(Err(e)));
                            }
                            let out = enc.get_mut().take();
                            this.state = State::Idle(enc);
                            if !out.is_empty() {
                                return Poll::Ready(Some(Ok(out)));
                            }
                        } else {
                            this.state = State::Encoding(spawn_blocking(move || {
                                enc.write_all(&chunk)?;
                                let out = enc.get_mut().take();
                                Ok((enc, out))
                            }));
                        }
                    }
                    Some(Err(e)) => {
                        this.state = State::Done;
                        return Poll::Ready(Some(Err(e)));
                    }
                    None => {
                        let State::Idle(enc) = std::mem::replace(&mut this.state, State::Done)
                        else {
                            unreachable!()
                        };
                        // `finish()` still has the tail block to compress.
                        this.state = State::Finishing(spawn_blocking(move || {
                            let mut writer = enc.finish()?;
                            Ok(writer.take())
                        }));
                    }
                },
            }
        }
    }
}

pub(crate) enum CompressedBody<B>
where
    B: MessageBody + Unpin,
{
    Identity(B),
    Zstd(Box<ZstdBody<BodyAsStream<B>>>),
}

impl<B> MessageBody for CompressedBody<B>
where
    B: MessageBody + Unpin,
{
    type Error = Box<dyn std::error::Error>;

    fn size(&self) -> BodySize {
        match self {
            Self::Identity(b) => b.size(),
            // Compressed length is unknown up front; chunked transfer.
            Self::Zstd(_) => BodySize::Stream,
        }
    }

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Bytes, Self::Error>>> {
        match self.get_mut() {
            Self::Identity(b) => Pin::new(b).poll_next(cx).map_err(Into::into),
            Self::Zstd(s) => Pin::new(s).poll_next(cx).map_err(Into::into),
        }
    }
}

/// App-wide zstd response compression with the tuned parameters from
/// [`ZstdConfig`]. Replaces `actix_web::middleware::Compress` so a single
/// configuration governs every route.
pub(crate) struct ZstdMiddleware {
    cfg: Rc<ZstdConfig>,
}

impl ZstdMiddleware {
    pub(crate) fn new(cfg: ZstdConfig) -> Self {
        Self { cfg: Rc::new(cfg) }
    }
}

impl<S, B> Transform<S, ServiceRequest> for ZstdMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: MessageBody + Unpin + 'static,
{
    type Response = ServiceResponse<CompressedBody<B>>;
    type Error = Error;
    type Transform = ZstdMiddlewareService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        fut_ready(Ok(ZstdMiddlewareService {
            service,
            cfg: self.cfg.clone(),
        }))
    }
}

pub(crate) struct ZstdMiddlewareService<S> {
    service: S,
    cfg: Rc<ZstdConfig>,
}

impl<S, B> Service<ServiceRequest> for ZstdMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: MessageBody + Unpin + 'static,
{
    type Response = ServiceResponse<CompressedBody<B>>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        // HEAD must mirror GET headers but the body is never sent, so a
        // pledged size would not be honoured; pass through untouched.
        let wants_zstd =
            req.method() != actix_web::http::Method::HEAD && accepts_zstd(req.headers());
        let cfg = self.cfg.clone();
        let fut = self.service.call(req);
        Box::pin(async move {
            let res = fut.await?;
            Ok(res.map_body(|head, body| {
                // A handler-set Content-Encoding also covers range responses,
                // which set `none` to keep partial content byte-exact.
                if !wants_zstd
                    || head.headers().contains_key(CONTENT_ENCODING)
                    || head.status.is_redirection()
                    || head.status == actix_web::http::StatusCode::NO_CONTENT
                {
                    return CompressedBody::Identity(body);
                }
                let pledge = match body.size() {
                    BodySize::None => return CompressedBody::Identity(body),
                    BodySize::Sized(n) if n < MIN_COMPRESS_SIZE => {
                        return CompressedBody::Identity(body);
                    }
                    BodySize::Sized(n) => Some(n),
                    BodySize::Stream => None,
                };
                match build_encoder(&cfg, pledge) {
                    Ok(enc) => {
                        head.headers_mut()
                            .insert(CONTENT_ENCODING, HeaderValue::from_static("zstd"));
                        head.headers_mut()
                            .append(VARY, HeaderValue::from_static("accept-encoding"));
                        head.no_chunking(false);
                        CompressedBody::Zstd(Box::new(ZstdBody::new(
                            BodyAsStream(body),
                            Box::new(enc),
                        )))
                    }
                    // Only reachable with an invalid `[zstd]` config.
                    Err(e) => {
                        tracing::warn!("zstd encoder init failed, serving identity: {e}");
                        CompressedBody::Identity(body)
                    }
                }
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::header::HeaderValue;
    use futures_util::{StreamExt, stream};

    #[test]
    fn accept_encoding_parse() {
        let mut h = HeaderMap::new();
        assert!(!accepts_zstd(&h));
        h.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br"));
        assert!(!accepts_zstd(&h));
        h.insert(ACCEPT_ENCODING, HeaderValue::from_static("zstd"));
        assert!(accepts_zstd(&h));
        h.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, zstd;q=0.9, br"),
        );
        assert!(accepts_zstd(&h));
        h.insert(ACCEPT_ENCODING, HeaderValue::from_static("zstd;q=0"));
        assert!(!accepts_zstd(&h));
        h.insert(ACCEPT_ENCODING, HeaderValue::from_static("zstd; Q=0"));
        assert!(!accepts_zstd(&h));
        h.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("zstd;foo=bar;q=0"),
        );
        assert!(!accepts_zstd(&h));
        // Second header line still counts.
        h.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        h.append(ACCEPT_ENCODING, HeaderValue::from_static("zstd"));
        assert!(accepts_zstd(&h));
    }

    #[actix_web::test]
    async fn round_trip() {
        // Mix of tiny and large chunks to cover both inline and blocking paths.
        let chunks: Vec<Bytes> = vec![
            Bytes::from_static(b"nix-archive-1"),
            Bytes::from(vec![0xabu8; 4 * 1024]),
            Bytes::from(
                (0..200_000u32)
                    .flat_map(|i| i.to_le_bytes())
                    .collect::<Vec<u8>>(),
            ),
            Bytes::from_static(b")"),
        ];
        let original: Vec<u8> = chunks.iter().flat_map(|b| b.iter().copied()).collect();

        let inner = stream::iter(chunks.into_iter().map(Ok::<_, io::Error>));
        let enc = build_encoder(&ZstdConfig::default(), Some(original.len() as u64)).unwrap();
        let body = ZstdBody::new(inner, Box::new(enc));
        futures_util::pin_mut!(body);

        let mut compressed = Vec::new();
        while let Some(chunk) = body.next().await {
            compressed.extend_from_slice(&chunk.unwrap());
        }
        assert!(compressed.len() < original.len());

        let decoded = zstd::stream::decode_all(&compressed[..]).unwrap();
        assert_eq!(decoded, original);
    }

    #[actix_web::test]
    async fn middleware_integration() {
        use actix_web::{App, HttpResponse, test, web};

        let big = Bytes::from(vec![b'x'; 64 * 1024]);
        let big_for_handler = big.clone();
        let app = test::init_service(
            App::new()
                .wrap(ZstdMiddleware::new(ZstdConfig::default()))
                .route(
                    "/big",
                    web::get().to(move || {
                        let b = big_for_handler.clone();
                        async move { HttpResponse::Ok().body(b) }
                    }),
                )
                .route(
                    "/tiny",
                    web::get().to(|| async { HttpResponse::Ok().body("ok") }),
                ),
        )
        .await;

        let res = test::call_service(&app, test::TestRequest::get().uri("/big").to_request()).await;
        assert!(res.headers().get(CONTENT_ENCODING).is_none());
        let body = test::read_body(res).await;
        assert_eq!(body, big);

        let res = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/big")
                .insert_header((ACCEPT_ENCODING, "zstd"))
                .to_request(),
        )
        .await;
        assert_eq!(res.headers().get(CONTENT_ENCODING).unwrap(), "zstd");
        let body = test::read_body(res).await;
        assert!(body.len() < big.len());
        assert_eq!(zstd::stream::decode_all(&body[..]).unwrap(), big);

        // Below MIN_COMPRESS_SIZE stays identity even when zstd accepted.
        let res = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/tiny")
                .insert_header((ACCEPT_ENCODING, "zstd"))
                .to_request(),
        )
        .await;
        assert!(res.headers().get(CONTENT_ENCODING).is_none());
        assert_eq!(test::read_body(res).await, "ok");
    }
}
