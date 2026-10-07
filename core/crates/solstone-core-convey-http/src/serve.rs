// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::Router;
use axum::extract::Extension;
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_FRAME_OPTIONS};
use axum::http::{Extensions, HeaderMap, HeaderValue, StatusCode, Version};
use axum::response::Response;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite};
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{Predicate, SizeAbove};
use tower_http::limit::RequestBodyLimitLayer;

use crate::identity::AccessBasis;

/// Maximum request body size accepted by the transport substrate.
///
/// Sized for SAVE (4 GiB + 1 MiB). Narrower routes pin their own extractor
/// ceilings with [`STANDARD_BODY_LIMIT`].
pub const REQUEST_BODY_LIMIT: usize = 4 * 1024 * 1024 * 1024 + 1024 * 1024;
/// Historical 128 MiB extractor ceiling for every route except SAVE.
pub const STANDARD_BODY_LIMIT: usize = 128 * 1024 * 1024;
/// Maximum number of HTTP/1 request headers accepted by the parser.
pub const MAX_HEADERS: usize = 32;
/// Maximum HTTP/1 read/write buffer size per connection.
pub const MAX_BUFFER_SIZE: usize = 64 * 1024;

/// The content security policy on every convey response that does not set its
/// own. Another site must not frame the owner's journal: a framed page's own
/// requests are same-origin, so the loopback guard cannot tell them from the
/// owner's clicks. All fetches default to this origin. The shell's inline
/// scripts and styles and local blob media require narrow exceptions.
pub const CONVEY_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; media-src 'self' blob:; form-action 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'self'";

/// Responses smaller than this are sent as they are; gzip framing outweighs the
/// saving below about a kilobyte.
pub const COMPRESSION_MIN_BYTES: u16 = 1024;

/// Whether a response's content type is text that gzip shrinks. An allowlist,
/// so media, archives and anything streamed byte-for-byte never get re-encoded;
/// event streams stay uncompressed so each event reaches the client at once.
pub fn compressible_content_type(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "text/event-stream" => false,
        "application/json"
        | "application/javascript"
        | "application/manifest+json"
        | "image/svg+xml" => true,
        other => other.starts_with("text/") || other.ends_with("+json"),
    }
}

fn compressible(_: StatusCode, _: Version, headers: &HeaderMap, _: &Extensions) -> bool {
    compressible_content_type(headers)
}

/// Construct the HTTP/1 settings for a TCP connection.
pub fn tcp_builder() -> http1::Builder {
    let mut builder = common_builder();
    builder.keep_alive(true);
    builder
}

/// Construct the HTTP/1 settings for a multiplexed stream connection.
pub fn mux_builder() -> http1::Builder {
    let mut builder = common_builder();
    // SPL CLOSE is a request half-close, not TCP connection destruction. Hyper
    // must continue writing the response after the peer has finished uploading.
    builder.keep_alive(false).half_close(true);
    builder
}

fn common_builder() -> http1::Builder {
    let mut builder = http1::Builder::new();
    builder
        .max_headers(MAX_HEADERS)
        .max_buf_size(MAX_BUFFER_SIZE);
    builder
}

/// Serve one HTTP/1 connection using accept-time access identity.
pub async fn serve_connection<I>(
    io: I,
    router: Router,
    identity: AccessBasis,
    builder: &http1::Builder,
) -> Result<(), hyper::Error>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let router = router
        .layer(axum::middleware::map_response(refuse_framing))
        // Every convey page and API answer crosses the relay as-is otherwise; a
        // journal-sized JSON state is ten times smaller gzipped. Only clients
        // that ask (Accept-Encoding) get it, and range responses never do.
        .layer(
            CompressionLayer::new()
                .no_br()
                .no_deflate()
                .no_zstd()
                .compress_when(SizeAbove::new(COMPRESSION_MIN_BYTES).and(compressible)),
        )
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT))
        .layer(Extension(identity));
    let service = TowerToHyperService::new(router);

    builder.serve_connection(TokioIo::new(io), service).await
}

async fn refuse_framing(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers
        .entry(CONTENT_SECURITY_POLICY)
        .or_insert(HeaderValue::from_static(CONVEY_CONTENT_SECURITY_POLICY));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{
        CONVEY_CONTENT_SECURITY_POLICY, REQUEST_BODY_LIMIT, mux_builder, serve_connection,
        tcp_builder,
    };
    use crate::envelope::probe_router;
    use crate::identity::{AccessBasis, Carrier, LinkedDeviceCid};

    const VALID_CID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    const REQUEST: &str = "GET /missing HTTP/1.1\r\nHost: localhost\r\n\r\n";
    const OVER_LIMIT_HEADER_COUNT: usize = 34;
    const OVERSIZED_HEADER_BYTES: usize = 64 * 1024 + 1;

    async fn response_after_one_read<B>(
        identity: AccessBasis,
        make_builder: B,
        request: &str,
    ) -> String
    where
        B: FnOnce() -> hyper::server::conn::http1::Builder,
    {
        let (server, mut client) = tokio::io::duplex(128 * 1024);
        let builder = make_builder();
        let serve = serve_connection(server, probe_router(), identity, &builder);
        let exchange = async {
            client.write_all(request.as_bytes()).await.unwrap();
            let mut bytes = [0_u8; 8192];
            let read = client.read(&mut bytes).await.unwrap();
            client.shutdown().await.unwrap();
            String::from_utf8(bytes[..read].to_vec()).unwrap()
        };
        let (served, body) = tokio::join!(serve, exchange);
        served.unwrap();
        body
    }

    async fn response_until_closed<B>(
        identity: AccessBasis,
        make_builder: B,
        request: String,
        close_write_after_request: bool,
    ) -> String
    where
        B: FnOnce() -> hyper::server::conn::http1::Builder,
    {
        let (server, mut client) = tokio::io::duplex(128 * 1024);
        let builder = make_builder();
        let serve = serve_connection(server, probe_router(), identity, &builder);
        let exchange = async {
            client.write_all(request.as_bytes()).await.unwrap();
            if close_write_after_request {
                client.shutdown().await.unwrap();
            }
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).await.unwrap();
            String::from_utf8(bytes).unwrap()
        };
        let (_, body) = tokio::join!(serve, exchange);
        body
    }

    #[tokio::test]
    async fn tcp_and_mux_stream_use_the_shared_http1_path() {
        let tcp = response_after_one_read(AccessBasis::Localhost, tcp_builder, REQUEST).await;
        let mux = response_after_one_read(AccessBasis::Localhost, mux_builder, REQUEST).await;

        assert!(tcp.starts_with("HTTP/1.1 404"));
        assert!(mux.starts_with("HTTP/1.1 404"));
        assert!(!tcp.to_ascii_lowercase().contains("connection: close"));
        assert!(mux.to_ascii_lowercase().contains("connection: close"));
    }

    // Falsified by dropping the framing layer: another site could frame the owner's
    // journal UI, and the loopback guard admits the framed page's same-origin requests.
    #[tokio::test]
    async fn every_response_refuses_to_be_framed_on_both_carriers() {
        for make_builder in [tcp_builder, mux_builder] {
            let response = response_after_one_read(
                AccessBasis::Localhost,
                make_builder,
                "GET /missing HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .await
            .to_ascii_lowercase();
            assert!(response.contains(&format!(
                "content-security-policy: {CONVEY_CONTENT_SECURITY_POLICY}\r\n"
            )));
            assert!(CONVEY_CONTENT_SECURITY_POLICY.contains("frame-ancestors 'none'"));
            assert!(CONVEY_CONTENT_SECURITY_POLICY.contains("default-src 'self'"));
            assert!(CONVEY_CONTENT_SECURITY_POLICY.contains("form-action 'self'"));
            assert!(response.contains("x-frame-options: deny\r\n"));
        }
    }

    #[tokio::test]
    async fn request_data_cannot_replace_accept_time_access_basis() {
        let response = response_until_closed(
            AccessBasis::LinkedDevice {
                carrier: Carrier::ViaSpl,
                cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
                leaf_spki: vec![0x30, 0x00],
            },
            mux_builder,
            "GET /Localhost?basis=Localhost HTTP/1.1\r\nHost: localhost\r\nX-Access-Basis: Localhost\r\nConnection: close\r\n\r\n".to_owned(),
            false,
        )
        .await;

        assert!(response.contains("LinkedDevice { carrier: ViaSpl, cid: LinkedDeviceCid"));
        assert!(!response.contains("\"detail\":\"Localhost\""));
    }

    #[tokio::test]
    async fn configured_body_header_and_buffer_bounds_are_enforced() {
        let body_response = response_until_closed(
            AccessBasis::Localhost,
            mux_builder,
            format!(
                "POST /missing HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                REQUEST_BODY_LIMIT + 1
            ),
            false,
        )
        .await;
        assert!(body_response.starts_with("HTTP/1.1 413"));

        let mut too_many_headers = String::from("GET /missing HTTP/1.1\r\nHost: localhost\r\n");
        for index in 0..(OVER_LIMIT_HEADER_COUNT - 2) {
            too_many_headers.push_str(&format!("X-Test-{index}: value\r\n"));
        }
        too_many_headers.push_str("Connection: close\r\n\r\n");
        let header_response =
            response_until_closed(AccessBasis::Localhost, mux_builder, too_many_headers, false)
                .await;
        assert!(header_response.starts_with("HTTP/1.1 431"));

        let large_header = "x".repeat(OVERSIZED_HEADER_BYTES);
        let buffer_response = response_until_closed(
            AccessBasis::Localhost,
            mux_builder,
            format!("GET /missing HTTP/1.1\r\nHost: localhost\r\nX-Large: {large_header}"),
            true,
        )
        .await;
        assert!(buffer_response.starts_with("HTTP/1.1 431"));
    }

    async fn compression_exchange(path: &str, accept_gzip: bool) -> String {
        use axum::http::header::CONTENT_TYPE;
        use axum::routing::get;

        let big = "x".repeat(4096);
        let small = "{}".to_owned();
        let router = axum::Router::new()
            .route(
                "/json",
                get({
                    let big = big.clone();
                    move || async move { ([(CONTENT_TYPE, "application/json")], big) }
                }),
            )
            .route(
                "/html",
                get({
                    let big = big.clone();
                    move || async move { ([(CONTENT_TYPE, "text/html; charset=utf-8")], big) }
                }),
            )
            .route(
                "/audio",
                get({
                    let big = big.clone();
                    move || async move { ([(CONTENT_TYPE, "audio/mp4")], big) }
                }),
            )
            .route(
                "/events",
                get({
                    let big = big.clone();
                    move || async move { ([(CONTENT_TYPE, "text/event-stream")], big) }
                }),
            )
            .route(
                "/small",
                get(move || async move { ([(CONTENT_TYPE, "application/json")], small) }),
            );
        let accept = if accept_gzip {
            "Accept-Encoding: gzip, deflate, br\r\n"
        } else {
            ""
        };
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{accept}Connection: close\r\n\r\n");

        let (server, mut client) = tokio::io::duplex(128 * 1024);
        let builder = mux_builder();
        let serve = serve_connection(server, router, AccessBasis::Localhost, &builder);
        let exchange = async {
            client.write_all(request.as_bytes()).await.unwrap();
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).await.unwrap();
            let head_end = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            String::from_utf8(bytes[..head_end].to_vec())
                .unwrap()
                .to_ascii_lowercase()
        };
        let (_, head) = tokio::join!(serve, exchange);
        head
    }

    // Falsified by removing the compression layer: a journal-sized JSON state then
    // crosses the relay at full size. Media, event streams, small answers and
    // clients that did not ask keep their bytes as they are.
    #[tokio::test]
    async fn text_answers_are_gzipped_only_for_clients_that_ask() {
        for path in ["/json", "/html"] {
            let head = compression_exchange(path, true).await;
            assert!(head.contains("content-encoding: gzip"), "{path}: {head}");
            let head = compression_exchange(path, false).await;
            assert!(!head.contains("content-encoding"), "{path}: {head}");
        }
        for path in ["/audio", "/events", "/small"] {
            let head = compression_exchange(path, true).await;
            assert!(!head.contains("content-encoding"), "{path}: {head}");
        }
    }
}
