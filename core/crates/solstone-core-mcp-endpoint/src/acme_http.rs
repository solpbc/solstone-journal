// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error as StdError;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use axum::http;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::{TokioExecutor, TokioIo};
use instant_acme::{BodyWrapper, BytesResponse, HttpClient};
#[cfg(any(test, feature = "test-hooks"))]
use rustls::pki_types::CertificateDer;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

/// The stream type delivered by [`McpEndpointConnector`].
#[allow(clippy::large_enum_variant)]
pub enum McpEndpointStream {
    Tls(TokioIo<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>),
    Plain(TokioIo<tokio::net::TcpStream>),
}

impl Connection for McpEndpointStream {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl hyper::rt::Read for McpEndpointStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl hyper::rt::Write for McpEndpointStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        match self.get_mut() {
            Self::Tls(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Self::Tls(stream) => Pin::new(stream).poll_flush(cx),
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Self::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// A dedicated connector that performs TCP connection and TLS handshake with pure Rust ring.
#[derive(Clone)]
pub struct McpEndpointConnector {
    tls_config: Arc<ClientConfig>,
}

impl tower::Service<http::Uri> for McpEndpointConnector {
    type Response = McpEndpointStream;
    type Error = Box<dyn StdError + Send + Sync>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: http::Uri) -> Self::Future {
        let host = match uri.host() {
            Some(h) => h.to_owned(),
            None => {
                return Box::pin(async { Err("URI missing host for ACME connection".into()) });
            }
        };
        let is_https = uri.scheme_str() != Some("http");
        let port = uri.port_u16().unwrap_or(if is_https { 443 } else { 80 });
        let tls_config = Arc::clone(&self.tls_config);

        Box::pin(async move {
            let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
            let _ = tcp.set_nodelay(true);
            if is_https {
                let server_name = ServerName::try_from(host)?;
                let tls = TlsConnector::from(tls_config)
                    .connect(server_name, tcp)
                    .await?;
                Ok(McpEndpointStream::Tls(TokioIo::new(tls)))
            } else {
                Ok(McpEndpointStream::Plain(TokioIo::new(tcp)))
            }
        })
    }
}

/// Production HTTP/1 client for ACME exchanges using ring and webpki-roots.
pub struct McpEndpointHttpClient {
    client: hyper_util::client::legacy::Client<McpEndpointConnector, BodyWrapper<bytes::Bytes>>,
}

impl McpEndpointHttpClient {
    /// Create the production ACME client.
    pub fn new() -> Self {
        Self::with_roots(RootCertStore::empty())
    }

    /// Create an ACME client with additional trusted root certificates.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn with_extra_roots(extra_roots: Vec<CertificateDer<'static>>) -> Self {
        let mut roots = RootCertStore::empty();
        for cert in extra_roots {
            let _ = roots.add(cert);
        }
        Self::with_roots(roots)
    }

    fn with_roots(mut roots: RootCertStore) -> Self {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring supports TLS 1.2 and 1.3")
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];

        let connector = McpEndpointConnector {
            tls_config: Arc::new(config),
        };
        let client =
            hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector);
        Self { client }
    }
}

impl Default for McpEndpointHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient for McpEndpointHttpClient {
    fn request(
        &self,
        req: http::Request<BodyWrapper<bytes::Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>> {
        let client = self.client.clone();
        with_request_deadline(async move {
            let res = client
                .request(req)
                .await
                .map_err(|e| instant_acme::Error::Other(Box::new(e)))?;
            Ok(BytesResponse::from(res))
        })
    }
}

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Bound connection, response headers and body by one request deadline.
pub(crate) fn with_request_deadline(
    response: impl Future<Output = Result<BytesResponse, instant_acme::Error>> + Send + 'static,
) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>> {
    let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
    Box::pin(async move {
        let response = tokio::time::timeout_at(deadline, response)
            .await
            .map_err(|error| instant_acme::Error::Other(Box::new(error)))??;
        Ok(BytesResponse {
            parts: response.parts,
            body: Box::new(DeadlineBody {
                inner: response.body,
                deadline,
            }),
        })
    })
}

struct DeadlineBody {
    inner: Box<dyn instant_acme::BytesBody>,
    deadline: tokio::time::Instant,
}

#[async_trait::async_trait]
impl instant_acme::BytesBody for DeadlineBody {
    async fn into_bytes(
        &mut self,
    ) -> Result<bytes::Bytes, Box<dyn StdError + Send + Sync + 'static>> {
        tokio::time::timeout_at(self.deadline, self.inner.into_bytes())
            .await
            .map_err(|error| -> Box<dyn StdError + Send + Sync> { Box::new(error) })?
    }
}

/// A wrapper around an [`HttpClient`] that captures the raw `Retry-After` header.
#[derive(Clone)]
pub struct RawRetryAfterCapturingClient {
    inner: Arc<dyn HttpClient>,
    slot: Arc<Mutex<Option<String>>>,
}

impl RawRetryAfterCapturingClient {
    pub fn new(inner: Arc<dyn HttpClient>) -> Self {
        Self {
            inner,
            slot: Arc::new(Mutex::new(None)),
        }
    }

    pub fn clear_retry_after(&self) {
        if let Ok(mut guard) = self.slot.lock() {
            *guard = None;
        }
    }

    pub fn captured_retry_after(&self) -> Option<String> {
        self.slot.lock().ok().and_then(|guard| guard.clone())
    }
}

impl HttpClient for RawRetryAfterCapturingClient {
    fn request(
        &self,
        req: http::Request<BodyWrapper<bytes::Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>> {
        let fut = self.inner.request(req);
        let slot = Arc::clone(&self.slot);
        Box::pin(async move {
            let rsp = fut.await?;
            let header = rsp
                .parts
                .headers
                .get(http::header::RETRY_AFTER)
                .and_then(|h| h.to_str().ok())
                .map(str::to_owned);
            if let Ok(mut guard) = slot.lock() {
                *guard = header;
            }
            Ok(rsp)
        })
    }
}

/// A wrapper around an [`Arc<dyn HttpClient>`] implementing [`HttpClient`].
#[derive(Clone)]
pub struct ArcHttpClient(pub Arc<dyn HttpClient>);

impl HttpClient for ArcHttpClient {
    fn request(
        &self,
        req: http::Request<BodyWrapper<bytes::Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>> {
        self.0.request(req)
    }
}

/// The result of parsing a raw `Retry-After` header value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AriRetry {
    pub delay_seconds: i64,
    pub retry_at: i64,
}

/// Parse an RFC 9110 `Retry-After` header value (delta-seconds or HTTP-date).
///
/// Clamp rules:
/// - Minimum: 1s
/// - Missing or malformed: 1 hour (3600s)
/// - Maximum: 24 hours (86400s)
pub fn parse_retry_after(header: Option<&str>, now_unix: i64) -> AriRetry {
    const DEFAULT_DELAY: i64 = 3600; // 1 hour
    const MIN_DELAY: i64 = 1;
    const MAX_DELAY: i64 = 86400; // 24 hours

    let delay = match header.map(str::trim).filter(|s| !s.is_empty()) {
        None => DEFAULT_DELAY,
        Some(s) => {
            if let Ok(secs) = s.parse::<u64>() {
                i64::try_from(secs).unwrap_or(MAX_DELAY)
            } else if let Ok(dt) = jiff::fmt::rfc2822::parse(s) {
                let target = dt.timestamp().as_second();
                (target - now_unix).max(0)
            } else {
                DEFAULT_DELAY
            }
        }
    };
    let clamped = delay.clamp(MIN_DELAY, MAX_DELAY);
    AriRetry {
        delay_seconds: clamped,
        retry_at: now_unix.saturating_add(clamped),
    }
}

/// Parse an RFC 9110 `Retry-After` header value for ACME issuance.
///
/// Unlike the ARI refresh parser (which caps at 24 hours), issuance Retry-After
/// may specify a multi-day delay or HTTP-date. Only an absurd value above 30 days
/// is capped.
///
/// Returns `Some(retry_at_unix)` if a valid delta-seconds or HTTP-date was provided.
/// Missing or malformed values return `None` (calling code falls back to 30s..3600s doubling).
pub fn parse_issuance_retry_after(header: Option<&str>, now_unix: i64) -> Option<i64> {
    const MIN_DELAY: i64 = 1;
    const MAX_DELAY: i64 = 30 * 86400; // 30 days

    let s = header.map(str::trim).filter(|s| !s.is_empty())?;
    let delay = if let Ok(secs) = s.parse::<u64>() {
        i64::try_from(secs).unwrap_or(MAX_DELAY)
    } else if let Ok(dt) = jiff::fmt::rfc2822::parse(s) {
        let target = dt.timestamp().as_second();
        target.saturating_sub(now_unix)
    } else {
        return None;
    };

    let clamped = delay.clamp(MIN_DELAY, MAX_DELAY);
    Some(now_unix.saturating_add(clamped))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CancelProbe(Arc<std::sync::atomic::AtomicBool>);

    impl Drop for CancelProbe {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct PendingBody(CancelProbe);

    #[async_trait::async_trait]
    impl instant_acme::BytesBody for PendingBody {
        async fn into_bytes(
            &mut self,
        ) -> Result<bytes::Bytes, Box<dyn StdError + Send + Sync + 'static>> {
            let _ = &self.0;
            std::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn request_deadline_cancels_stalled_response_headers() {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = CancelProbe(Arc::clone(&cancelled));
        let response = with_request_deadline(async move {
            let _probe = probe;
            std::future::pending().await
        })
        .await;
        assert!(matches!(response, Err(instant_acme::Error::Other(error))
            if error.is::<tokio::time::error::Elapsed>()));
        assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn request_deadline_covers_headers_and_body_with_one_budget() {
        let start = tokio::time::Instant::now();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = CancelProbe(Arc::clone(&cancelled));
        let mut response = with_request_deadline(async move {
            tokio::time::sleep(REQUEST_TIMEOUT - std::time::Duration::from_secs(1)).await;
            let (parts, ()) = http::Response::builder()
                .status(http::StatusCode::TOO_MANY_REQUESTS)
                .header(http::header::RETRY_AFTER, "604800")
                .body(())
                .unwrap()
                .into_parts();
            Ok(BytesResponse {
                parts,
                body: Box::new(PendingBody(probe)),
            })
        })
        .await
        .unwrap();
        assert_eq!(response.parts.status, http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.parts.headers[http::header::RETRY_AFTER], "604800");
        let error = response.body.into_bytes().await.unwrap_err();
        assert!(error.is::<tokio::time::error::Elapsed>());
        assert_eq!(start.elapsed(), REQUEST_TIMEOUT);
        drop(response);
        assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn request_deadline_preserves_successful_response_bytes() {
        let mut response = with_request_deadline(async move {
            let response = http::Response::builder()
                .status(http::StatusCode::OK)
                .body(BodyWrapper::from(b"response body".to_vec()))
                .unwrap();
            Ok(BytesResponse::from(response))
        })
        .await
        .unwrap();
        assert_eq!(response.parts.status, http::StatusCode::OK);
        assert_eq!(response.body.into_bytes().await.unwrap(), "response body");
    }

    #[test]
    fn parse_retry_after_handles_all_prescribed_shapes() {
        let now = 1_700_000_000_i64;

        // Missing
        let r = parse_retry_after(None, now);
        assert_eq!(r.delay_seconds, 3600);
        assert_eq!(r.retry_at, now + 3600);

        // Empty / whitespace
        let r = parse_retry_after(Some("   "), now);
        assert_eq!(r.delay_seconds, 3600);
        assert_eq!(r.retry_at, now + 3600);

        // Garbage
        let r = parse_retry_after(Some("not-a-number"), now);
        assert_eq!(r.delay_seconds, 3600);
        assert_eq!(r.retry_at, now + 3600);

        // Delta-seconds within bounds
        let r = parse_retry_after(Some("120"), now);
        assert_eq!(r.delay_seconds, 120);
        assert_eq!(r.retry_at, now + 120);

        // Minimum clamp (0 or negative)
        let r = parse_retry_after(Some("0"), now);
        assert_eq!(r.delay_seconds, 1);
        assert_eq!(r.retry_at, now + 1);

        // Maximum clamp (absurd interval, e.g. 10 days)
        let r = parse_retry_after(Some("864000"), now);
        assert_eq!(r.delay_seconds, 86400);
        assert_eq!(r.retry_at, now + 86400);

        // Valid RFC 2822 HTTP-date (future)
        let future_dt = jiff::Timestamp::new(now + 7200, 0)
            .unwrap()
            .to_zoned_datetime(jiff::tz::TimeZone::UTC);
        let date_str = jiff::fmt::rfc2822::to_string(&future_dt).unwrap();
        let r = parse_retry_after(Some(&date_str), now);
        assert_eq!(r.delay_seconds, 7200);
        assert_eq!(r.retry_at, now + 7200);

        // HTTP-date in the past -> clamped to 1s
        let past_dt = jiff::Timestamp::new(now - 300, 0)
            .unwrap()
            .to_zoned_datetime(jiff::tz::TimeZone::UTC);
        let date_str = jiff::fmt::rfc2822::to_string(&past_dt).unwrap();
        let r = parse_retry_after(Some(&date_str), now);
        assert_eq!(r.delay_seconds, 1);
        assert_eq!(r.retry_at, now + 1);
    }

    #[test]
    fn parse_issuance_retry_after_handles_all_prescribed_shapes() {
        let now = 1_700_000_000_i64;

        // Missing or malformed returns None
        assert_eq!(parse_issuance_retry_after(None, now), None);
        assert_eq!(parse_issuance_retry_after(Some("   "), now), None);
        assert_eq!(parse_issuance_retry_after(Some("not-a-number"), now), None);

        // Delta-seconds within bounds
        assert_eq!(
            parse_issuance_retry_after(Some("120"), now),
            Some(now + 120)
        );

        // Minimum clamp (0s -> 1s)
        assert_eq!(parse_issuance_retry_after(Some("0"), now), Some(now + 1));

        // Multi-day delta (10 days) is NOT capped at 24h
        assert_eq!(
            parse_issuance_retry_after(Some("864000"), now),
            Some(now + 864000)
        );

        // Absurd delta (> 30 days) is capped at 30 days
        let thirty_days = 30 * 86400;
        assert_eq!(
            parse_issuance_retry_after(Some("3456000"), now),
            Some(now + thirty_days)
        );

        // Valid RFC 2822 HTTP-date (7 days in future)
        let future_dt = jiff::Timestamp::new(now + 7 * 86400, 0)
            .unwrap()
            .to_zoned_datetime(jiff::tz::TimeZone::UTC);
        let date_str = jiff::fmt::rfc2822::to_string(&future_dt).unwrap();
        assert_eq!(
            parse_issuance_retry_after(Some(&date_str), now),
            Some(now + 7 * 86400)
        );

        // HTTP-date in the past -> clamped to 1s
        let past_dt = jiff::Timestamp::new(now - 300, 0)
            .unwrap()
            .to_zoned_datetime(jiff::tz::TimeZone::UTC);
        let date_str = jiff::fmt::rfc2822::to_string(&past_dt).unwrap();
        assert_eq!(
            parse_issuance_retry_after(Some(&date_str), now),
            Some(now + 1)
        );
    }
}
