// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CIMD address classification, known-client table, and HTTPS fetch.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;

use super::bulkhead::{CimdBulkhead, CimdBulkheadError};
use super::urlparse::validate_cimd_url;

/// Exact CIMD document URLs treated as known clients.
///
/// Empty until vendor-published Claude/Codex URLs are filled in.
pub(crate) const KNOWN_CLIENT_CIMD_URLS: &[&str] = &[];

/// Why a resolved address must not be contacted for a CIMD fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnusableIpClass {
    Unspecified,
    Loopback,
    Private,
    LinkLocal,
    Multicast,
    ReservedDocumentation,
    Broadcast,
}

/// Map IPv4-mapped IPv6 to IPv4; leave every other address unchanged.
pub(crate) fn canonicalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        IpAddr::V4(_) => ip,
    }
}

/// Classify an address that must not be used as a CIMD fetch target.
pub(crate) fn classify_unusable_ip(ip: IpAddr) -> Option<UnusableIpClass> {
    match canonicalize_ip(ip) {
        IpAddr::V4(address) => classify_v4(address),
        IpAddr::V6(address) => classify_v6(address),
    }
}

/// True when `url` is an exact known-client CIMD document URL.
pub(crate) fn is_known_cimd_url(url: &str) -> bool {
    KNOWN_CLIENT_CIMD_URLS.contains(&url)
}

fn classify_v4(address: Ipv4Addr) -> Option<UnusableIpClass> {
    if address.is_broadcast() {
        return Some(UnusableIpClass::Broadcast);
    }
    let bits = u32::from(address);
    if in_range(
        bits,
        Ipv4Addr::new(0, 0, 0, 0),
        Ipv4Addr::new(0, 255, 255, 255),
    ) {
        return Some(UnusableIpClass::Unspecified);
    }
    if address.is_loopback() {
        return Some(UnusableIpClass::Loopback);
    }
    if in_range(
        bits,
        Ipv4Addr::new(10, 0, 0, 0),
        Ipv4Addr::new(10, 255, 255, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(100, 64, 0, 0),
        Ipv4Addr::new(100, 127, 255, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(172, 16, 0, 0),
        Ipv4Addr::new(172, 31, 255, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(192, 168, 0, 0),
        Ipv4Addr::new(192, 168, 255, 255),
    ) {
        return Some(UnusableIpClass::Private);
    }
    if in_range(
        bits,
        Ipv4Addr::new(169, 254, 0, 0),
        Ipv4Addr::new(169, 254, 255, 255),
    ) {
        return Some(UnusableIpClass::LinkLocal);
    }
    if in_range(
        bits,
        Ipv4Addr::new(224, 0, 0, 0),
        Ipv4Addr::new(239, 255, 255, 255),
    ) {
        return Some(UnusableIpClass::Multicast);
    }
    if in_range(
        bits,
        Ipv4Addr::new(192, 0, 2, 0),
        Ipv4Addr::new(192, 0, 2, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(198, 51, 100, 0),
        Ipv4Addr::new(198, 51, 100, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(203, 0, 113, 0),
        Ipv4Addr::new(203, 0, 113, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(192, 0, 0, 0),
        Ipv4Addr::new(192, 0, 0, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(192, 88, 99, 0),
        Ipv4Addr::new(192, 88, 99, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(198, 18, 0, 0),
        Ipv4Addr::new(198, 19, 255, 255),
    ) || in_range(
        bits,
        Ipv4Addr::new(240, 0, 0, 0),
        Ipv4Addr::new(255, 255, 255, 254),
    ) {
        return Some(UnusableIpClass::ReservedDocumentation);
    }
    None
}

fn classify_v6(address: Ipv6Addr) -> Option<UnusableIpClass> {
    if address.is_unspecified() {
        return Some(UnusableIpClass::Unspecified);
    }
    if address.is_loopback() {
        return Some(UnusableIpClass::Loopback);
    }
    if address.is_unique_local() {
        return Some(UnusableIpClass::Private);
    }
    if address.is_unicast_link_local() {
        return Some(UnusableIpClass::LinkLocal);
    }
    if address.is_multicast() {
        return Some(UnusableIpClass::Multicast);
    }
    let segments = address.segments();
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return Some(UnusableIpClass::ReservedDocumentation);
    }
    None
}

fn in_range(bits: u32, start: Ipv4Addr, end: Ipv4Addr) -> bool {
    let start = u32::from(start);
    let end = u32::from(end);
    start <= bits && bits <= end
}

const CIMD_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const CIMD_BODY_MAX: usize = 5 * 1024;
const CIMD_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const CIMD_HEADER_MAX: usize = 8 * 1024;

/// A validated Client ID Metadata Document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CimdDocument {
    pub(crate) client_id: String,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) client_name: Option<String>,
}

/// Why a CIMD fetch cannot complete. Messages are class-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CimdFetchError {
    Url,
    Resolve,
    UnusableAddress,
    Connect,
    Tls,
    Redirect,
    Status,
    Encoding,
    BodyCap,
    Json,
    ClientIdMismatch,
    Deadline,
    Cancelled,
    Bulkhead,
}

impl fmt::Display for CimdFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Url => "CIMD URL is invalid",
            Self::Resolve => "CIMD origin could not be resolved",
            Self::UnusableAddress => "CIMD origin resolved to an unusable address",
            Self::Connect => "CIMD origin could not be connected",
            Self::Tls => "CIMD origin TLS connection failed",
            Self::Redirect => "CIMD origin returned a redirect",
            Self::Status => "CIMD origin returned an unexpected status",
            Self::Encoding => "CIMD origin used a content encoding",
            Self::BodyCap => "CIMD document exceeds its size limit",
            Self::Json => "CIMD document is invalid",
            Self::ClientIdMismatch => "CIMD client_id does not match the fetched URL",
            Self::Deadline => "CIMD fetch exceeded its deadline",
            Self::Cancelled => "CIMD fetch was cancelled",
            Self::Bulkhead => "CIMD fetch admission is unavailable",
        })
    }
}

impl std::error::Error for CimdFetchError {}

/// Pluggable resolve/connect/TLS steps for one CIMD fetch.
pub(crate) trait CimdAttemptIo {
    type Socket;
    type Connection: AsyncRead + AsyncWrite + Unpin;

    async fn resolve(&mut self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>>;
    async fn connect(&mut self, address: SocketAddr) -> io::Result<Self::Socket>;
    async fn tls(
        &mut self,
        socket: Self::Socket,
        server_name: &str,
    ) -> io::Result<Self::Connection>;
}

struct TokioCimdAttemptIo;

impl CimdAttemptIo for TokioCimdAttemptIo {
    type Socket = tokio::net::TcpStream;
    type Connection = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;

    async fn resolve(&mut self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        tokio::net::lookup_host((host, port))
            .await
            .map(Iterator::collect)
    }

    async fn connect(&mut self, address: SocketAddr) -> io::Result<Self::Socket> {
        tokio::net::TcpStream::connect(address).await
    }

    async fn tls(
        &mut self,
        socket: Self::Socket,
        server_name: &str,
    ) -> io::Result<Self::Connection> {
        let server_name = ServerName::try_from(server_name.to_owned())
            .map_err(|_| io::Error::other("CIMD server name is invalid"))?;
        let config = cimd_tls_config()?;
        TlsConnector::from(config)
            .connect(server_name, socket)
            .await
    }
}

fn cimd_tls_config() -> io::Result<Arc<ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|_| io::Error::other("CIMD TLS configuration is unavailable"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

async fn write_cimd_request<W: AsyncWrite + Unpin>(
    mut writer: W,
    request: &[u8],
) -> io::Result<()> {
    writer.write_all(request).await
}

/// Fetch one CIMD document over WebPKI-authenticated HTTPS.
pub(crate) async fn fetch_cimd(
    url: &str,
    source: IpAddr,
    bulkhead: &Arc<CimdBulkhead>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<CimdDocument, CimdFetchError> {
    fetch_cimd_with_io(url, source, bulkhead, shutdown, TokioCimdAttemptIo).await
}

pub(crate) async fn fetch_cimd_with_io<IO: CimdAttemptIo>(
    url: &str,
    source: IpAddr,
    bulkhead: &Arc<CimdBulkhead>,
    shutdown: &mut watch::Receiver<bool>,
    mut io: IO,
) -> Result<CimdDocument, CimdFetchError> {
    let parsed = validate_cimd_url(url).map_err(|_| CimdFetchError::Url)?;
    let _known = is_known_cimd_url(url);
    let deadline = Instant::now() + CIMD_FETCH_TIMEOUT;
    let _permit = bulkhead
        .acquire(canonicalize_ip(source), deadline)
        .await
        .map_err(|error| match error {
            CimdBulkheadError::Timeout | CimdBulkheadError::Cancelled => CimdFetchError::Bulkhead,
        })?;

    let addresses = await_phase(shutdown, deadline, io.resolve(&parsed.host, 443))
        .await
        .map_err(|error| match error {
            CimdFetchError::Cancelled | CimdFetchError::Deadline => error,
            _ => CimdFetchError::Resolve,
        })?;
    if addresses.is_empty() {
        return Err(CimdFetchError::Resolve);
    }
    if addresses
        .iter()
        .any(|address| classify_unusable_ip(address.ip()).is_some())
    {
        return Err(CimdFetchError::UnusableAddress);
    }

    let connect_deadline = Instant::now() + CIMD_CONNECT_TIMEOUT;
    let connect_deadline = if connect_deadline < deadline {
        connect_deadline
    } else {
        deadline
    };
    let mut socket = None;
    for address in addresses {
        match await_phase(shutdown, connect_deadline, io.connect(address)).await {
            Ok(connected) => {
                socket = Some(connected);
                break;
            }
            Err(error @ (CimdFetchError::Cancelled | CimdFetchError::Deadline)) => {
                return Err(error);
            }
            Err(_) => continue,
        }
    }
    let socket = socket.ok_or(CimdFetchError::Connect)?;
    let mut connection = await_phase(shutdown, deadline, io.tls(socket, &parsed.host))
        .await
        .map_err(|error| match error {
            CimdFetchError::Cancelled | CimdFetchError::Deadline => error,
            _ => CimdFetchError::Tls,
        })?;

    let target = match &parsed.query {
        Some(query) => format!("{}?{query}", parsed.path),
        None => parsed.path.clone(),
    };
    let request = format!(
        "GET {target} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        parsed.host
    );
    await_phase(
        shutdown,
        deadline,
        write_cimd_request(&mut connection, request.as_bytes()),
    )
    .await
    .map_err(|error| match error {
        CimdFetchError::Cancelled | CimdFetchError::Deadline => error,
        _ => CimdFetchError::Connect,
    })?;
    await_phase(shutdown, deadline, connection.flush())
        .await
        .map_err(|error| match error {
            CimdFetchError::Cancelled | CimdFetchError::Deadline => error,
            _ => CimdFetchError::Connect,
        })?;

    let head = read_headers(&mut connection, shutdown, deadline).await?;
    let (status, headers) = parse_headers(&head)?;
    if (300..400).contains(&status) {
        return Err(CimdFetchError::Redirect);
    }
    if status != 200 {
        return Err(CimdFetchError::Status);
    }
    if header_present(&headers, "content-encoding") {
        return Err(CimdFetchError::Encoding);
    }
    let body = read_body(
        &mut connection,
        shutdown,
        deadline,
        content_length(&headers)?,
    )
    .await?;
    parse_cimd_document(url, &body)
}

async fn await_phase<T, F>(
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    future: F,
) -> Result<T, CimdFetchError>
where
    F: std::future::Future<Output = io::Result<T>>,
{
    tokio::pin!(future);
    loop {
        if shutdown_requested(shutdown) {
            return Err(CimdFetchError::Cancelled);
        }
        let remaining = remaining(deadline)?;
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    return Err(CimdFetchError::Cancelled);
                }
            }
            result = tokio::time::timeout(remaining, &mut future) => {
                return result
                    .map_err(|_| CimdFetchError::Deadline)?
                    .map_err(|_| CimdFetchError::Connect);
            }
        }
    }
}

fn shutdown_requested(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}

fn remaining(deadline: Instant) -> Result<Duration, CimdFetchError> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or(CimdFetchError::Deadline)
}

async fn read_headers<R: AsyncRead + Unpin>(
    reader: &mut R,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
) -> Result<Vec<u8>, CimdFetchError> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1];
    loop {
        if bytes.len() >= CIMD_HEADER_MAX {
            return Err(CimdFetchError::Status);
        }
        let read = await_phase(shutdown, deadline, reader.read(&mut chunk)).await?;
        if read == 0 {
            return Err(CimdFetchError::Status);
        }
        bytes.push(chunk[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            return Ok(bytes);
        }
    }
}

fn parse_headers(bytes: &[u8]) -> Result<(u16, Vec<(String, String)>), CimdFetchError> {
    let text = std::str::from_utf8(bytes).map_err(|_| CimdFetchError::Status)?;
    let text = text
        .strip_suffix("\r\n\r\n")
        .ok_or(CimdFetchError::Status)?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().ok_or(CimdFetchError::Status)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or(CimdFetchError::Status)?;
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(CimdFetchError::Status)?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    Ok((status, headers))
}

fn header_present(headers: &[(String, String)], name: &str) -> bool {
    headers.iter().any(|(header, _)| header == name)
}

fn content_length(headers: &[(String, String)]) -> Result<Option<usize>, CimdFetchError> {
    let Some((_, value)) = headers.iter().find(|(name, _)| name == "content-length") else {
        return Ok(None);
    };
    value
        .parse::<usize>()
        .map(Some)
        .map_err(|_| CimdFetchError::Status)
}

async fn read_body<R: AsyncRead + Unpin>(
    reader: &mut R,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    content_length: Option<usize>,
) -> Result<Vec<u8>, CimdFetchError> {
    if let Some(length) = content_length {
        if length > CIMD_BODY_MAX {
            return Err(CimdFetchError::BodyCap);
        }
        let mut body = vec![0_u8; length];
        await_phase(shutdown, deadline, reader.read_exact(&mut body)).await?;
        return Ok(body);
    }
    let mut body = Vec::new();
    let mut chunk = [0_u8; 512];
    loop {
        let read = await_phase(shutdown, deadline, reader.read(&mut chunk)).await?;
        if read == 0 {
            return Ok(body);
        }
        if body.len().saturating_add(read) > CIMD_BODY_MAX {
            return Err(CimdFetchError::BodyCap);
        }
        body.extend_from_slice(&chunk[..read]);
    }
}

#[derive(Deserialize)]
struct CimdBody {
    client_id: String,
    redirect_uris: Vec<String>,
    #[serde(default)]
    client_name: Option<String>,
}

fn parse_cimd_document(url: &str, body: &[u8]) -> Result<CimdDocument, CimdFetchError> {
    let parsed: CimdBody = serde_json::from_slice(body).map_err(|_| CimdFetchError::Json)?;
    if parsed.redirect_uris.is_empty() {
        return Err(CimdFetchError::Json);
    }
    if parsed.client_id != url {
        return Err(CimdFetchError::ClientIdMismatch);
    }
    Ok(CimdDocument {
        client_id: parsed.client_id,
        redirect_uris: parsed.redirect_uris,
        client_name: parsed.client_name,
    })
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::{UnusableIpClass, canonicalize_ip, classify_unusable_ip, is_known_cimd_url};

    #[test]
    fn canonicalize_maps_v4_mapped_v6_only() {
        let mapped = IpAddr::V6(Ipv6Addr::from([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 10, 1, 2, 3,
        ]));
        assert_eq!(
            canonicalize_ip(mapped),
            IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))
        );
        let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_eq!(canonicalize_ip(v6), v6);
        let v4 = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        assert_eq!(canonicalize_ip(v4), v4);
    }

    #[test]
    fn ipv4_classes_match_the_closed_table() {
        let cases = [
            (
                Ipv4Addr::new(0, 0, 0, 0),
                Some(UnusableIpClass::Unspecified),
            ),
            (
                Ipv4Addr::new(0, 1, 2, 3),
                Some(UnusableIpClass::Unspecified),
            ),
            (Ipv4Addr::new(127, 0, 0, 1), Some(UnusableIpClass::Loopback)),
            (Ipv4Addr::new(10, 0, 0, 1), Some(UnusableIpClass::Private)),
            (Ipv4Addr::new(100, 64, 0, 1), Some(UnusableIpClass::Private)),
            (Ipv4Addr::new(172, 16, 0, 1), Some(UnusableIpClass::Private)),
            (
                Ipv4Addr::new(172, 31, 255, 255),
                Some(UnusableIpClass::Private),
            ),
            (
                Ipv4Addr::new(192, 168, 1, 1),
                Some(UnusableIpClass::Private),
            ),
            (
                Ipv4Addr::new(169, 254, 1, 1),
                Some(UnusableIpClass::LinkLocal),
            ),
            (
                Ipv4Addr::new(224, 0, 0, 1),
                Some(UnusableIpClass::Multicast),
            ),
            (
                Ipv4Addr::new(192, 0, 2, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(198, 51, 100, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(203, 0, 113, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(192, 0, 0, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(192, 88, 99, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(198, 18, 0, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (
                Ipv4Addr::new(240, 0, 0, 1),
                Some(UnusableIpClass::ReservedDocumentation),
            ),
            (Ipv4Addr::BROADCAST, Some(UnusableIpClass::Broadcast)),
            (Ipv4Addr::new(8, 8, 8, 8), None),
            (Ipv4Addr::new(172, 32, 0, 1), None),
        ];
        for (address, expected) in cases {
            assert_eq!(
                classify_unusable_ip(IpAddr::V4(address)),
                expected,
                "{address}"
            );
        }
    }

    #[test]
    fn ipv6_classes_and_mapped_v4_are_classified() {
        assert_eq!(
            classify_unusable_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)),
            Some(UnusableIpClass::Unspecified)
        );
        assert_eq!(
            classify_unusable_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            Some(UnusableIpClass::Loopback)
        );
        assert_eq!(
            classify_unusable_ip("fc00::1".parse().unwrap()),
            Some(UnusableIpClass::Private)
        );
        assert_eq!(
            classify_unusable_ip("fe80::1".parse().unwrap()),
            Some(UnusableIpClass::LinkLocal)
        );
        assert_eq!(
            classify_unusable_ip("ff02::1".parse().unwrap()),
            Some(UnusableIpClass::Multicast)
        );
        assert_eq!(
            classify_unusable_ip("2001:db8::1".parse().unwrap()),
            Some(UnusableIpClass::ReservedDocumentation)
        );
        assert_eq!(
            classify_unusable_ip("2001:4860:4860::8888".parse().unwrap()),
            None
        );
        assert_eq!(
            classify_unusable_ip("::ffff:10.0.0.1".parse().unwrap()),
            Some(UnusableIpClass::Private)
        );
        assert_eq!(
            classify_unusable_ip("::ffff:8.8.8.8".parse().unwrap()),
            None
        );
    }

    #[test]
    fn known_client_table_is_empty_and_exact() {
        assert!(!is_known_cimd_url("https://claude.ai/.well-known/mcp.json"));
        assert!(!is_known_cimd_url(""));
        assert!(super::KNOWN_CLIENT_CIMD_URLS.is_empty());
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod fetch_tests {
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::watch;
    use tokio::time::Duration;

    use super::{
        CIMD_BODY_MAX, CIMD_FETCH_TIMEOUT, CimdAttemptIo, CimdFetchError, fetch_cimd_with_io,
    };
    use crate::oauth::bulkhead::CimdBulkhead;

    const URL: &str = "https://client.example/cimd.json";
    const SOURCE: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10));

    fn public_addr() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(8, 8, 8, 8), 443))
    }

    fn json_body() -> String {
        format!(
            r#"{{"client_id":"{URL}","redirect_uris":["http://127.0.0.1/callback"],"client_name":"fixture","extra":1}}"#
        )
    }

    fn http_response(status: &str, extra_headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn default_response() -> Vec<u8> {
        http_response("200 OK", "", &json_body())
    }

    struct FakeConnection {
        data: Vec<u8>,
        offset: usize,
        stall_at: Option<usize>,
    }

    impl AsyncRead for FakeConnection {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.stall_at.is_some_and(|limit| self.offset >= limit) {
                return Poll::Pending;
            }
            if self.offset >= self.data.len() {
                return Poll::Ready(Ok(()));
            }
            let remaining = self.data.len() - self.offset;
            let take = remaining.min(buf.remaining());
            buf.put_slice(&self.data[self.offset..self.offset + take]);
            self.offset += take;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for FakeConnection {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct FakeIo {
        addresses: Vec<SocketAddr>,
        resolve_calls: Arc<AtomicUsize>,
        connect_calls: Arc<AtomicUsize>,
        tls_calls: Arc<AtomicUsize>,
        tls_error: bool,
        response: Vec<u8>,
        stall_at: Option<usize>,
        stall_on: Option<&'static str>,
        delay: Duration,
    }

    impl FakeIo {
        fn ok(addresses: Vec<SocketAddr>, response: Vec<u8>) -> Self {
            Self {
                addresses,
                resolve_calls: Arc::new(AtomicUsize::new(0)),
                connect_calls: Arc::new(AtomicUsize::new(0)),
                tls_calls: Arc::new(AtomicUsize::new(0)),
                tls_error: false,
                response,
                stall_at: None,
                stall_on: None,
                delay: Duration::ZERO,
            }
        }
    }

    impl CimdAttemptIo for FakeIo {
        type Socket = ();
        type Connection = FakeConnection;

        async fn resolve(&mut self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
            self.resolve_calls.fetch_add(1, Ordering::SeqCst);
            if self.stall_on == Some("resolve") {
                std::future::pending::<()>().await;
            }
            if self.delay > Duration::ZERO {
                tokio::time::sleep(self.delay).await;
            }
            Ok(self.addresses.clone())
        }

        async fn connect(&mut self, _address: SocketAddr) -> io::Result<Self::Socket> {
            self.connect_calls.fetch_add(1, Ordering::SeqCst);
            if self.stall_on == Some("connect") {
                std::future::pending::<()>().await;
            }
            if self.delay > Duration::ZERO {
                tokio::time::sleep(self.delay).await;
            }
            Ok(())
        }

        async fn tls(
            &mut self,
            _socket: Self::Socket,
            _server_name: &str,
        ) -> io::Result<Self::Connection> {
            self.tls_calls.fetch_add(1, Ordering::SeqCst);
            if self.tls_error {
                return Err(io::Error::other("hostname verification failed"));
            }
            if self.delay > Duration::ZERO {
                tokio::time::sleep(self.delay).await;
            }
            Ok(FakeConnection {
                data: self.response.clone(),
                offset: 0,
                stall_at: self.stall_at,
            })
        }
    }

    async fn fetch(io: FakeIo) -> Result<super::CimdDocument, CimdFetchError> {
        let bulkhead = CimdBulkhead::new();
        let (_tx, mut rx) = watch::channel(false);
        fetch_cimd_with_io(URL, SOURCE, &bulkhead, &mut rx, io).await
    }

    #[tokio::test(start_paused = true)]
    async fn valid_public_control_returns_the_document() {
        let io = FakeIo::ok(vec![public_addr()], default_response());
        let resolve = Arc::clone(&io.resolve_calls);
        let document = fetch(io).await.expect("public CIMD fetches");
        assert_eq!(document.client_id, URL);
        assert_eq!(document.redirect_uris, ["http://127.0.0.1/callback"]);
        assert_eq!(document.client_name.as_deref(), Some("fixture"));
        assert_eq!(resolve.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unusable_single_addresses_never_connect() {
        let cases: [(&str, SocketAddr); 4] = [
            ("10.0.0.1:443", "10.0.0.1:443".parse().unwrap()),
            ("127.0.0.1:443", "127.0.0.1:443".parse().unwrap()),
            ("169.254.1.1:443", "169.254.1.1:443".parse().unwrap()),
            ("192.0.2.1:443", "192.0.2.1:443".parse().unwrap()),
        ];
        for (_name, address) in cases {
            let io = FakeIo::ok(vec![address], default_response());
            let connect = Arc::clone(&io.connect_calls);
            let tls = Arc::clone(&io.tls_calls);
            assert_eq!(fetch(io).await, Err(CimdFetchError::UnusableAddress));
            assert_eq!(connect.load(Ordering::SeqCst), 0);
            assert_eq!(tls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn mixed_rrset_fails_without_connecting_to_the_public_address() {
        let io = FakeIo::ok(
            vec![public_addr(), "10.0.0.1:443".parse().unwrap()],
            default_response(),
        );
        let connect = Arc::clone(&io.connect_calls);
        assert_eq!(fetch(io).await, Err(CimdFetchError::UnusableAddress));
        assert_eq!(connect.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn resolve_is_invoked_once() {
        let io = FakeIo::ok(vec![public_addr()], default_response());
        let resolve = Arc::clone(&io.resolve_calls);
        fetch(io).await.unwrap();
        assert_eq!(resolve.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn redirect_status_is_not_followed() {
        let response =
            b"HTTP/1.1 302 Found\r\nLocation: https://evil.example/\r\nContent-Length: 0\r\n\r\n"
                .to_vec();
        let io = FakeIo::ok(vec![public_addr()], response);
        let tls = Arc::clone(&io.tls_calls);
        assert_eq!(fetch(io).await, Err(CimdFetchError::Redirect));
        assert_eq!(tls.load(Ordering::SeqCst), 1);
    }

    /// Fake `tls()` I/O error only. This is not a rustls certificate-validation proof.
    #[tokio::test(start_paused = true)]
    async fn tls_hostname_failure_maps_to_tls() {
        let mut io = FakeIo::ok(vec![public_addr()], default_response());
        io.tls_error = true;
        assert_eq!(fetch(io).await, Err(CimdFetchError::Tls));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_headers_time_out() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n".to_vec();
        let stall_at = headers.len();
        let mut io = FakeIo::ok(vec![public_addr()], headers);
        io.stall_at = Some(stall_at);
        let fetch = fetch(io);
        tokio::pin!(fetch);
        tokio::task::yield_now().await;
        tokio::time::advance(CIMD_FETCH_TIMEOUT).await;
        assert_eq!(fetch.await, Err(CimdFetchError::Deadline));
    }

    #[tokio::test(start_paused = true)]
    async fn malformed_documents_are_json_errors() {
        for body in [
            "not-json",
            r#"{"client_id":"https://client.example/cimd.json"}"#,
            r#"{"client_id":"https://client.example/cimd.json","redirect_uris":[]}"#,
        ] {
            let io = FakeIo::ok(vec![public_addr()], http_response("200 OK", "", body));
            assert_eq!(fetch(io).await, Err(CimdFetchError::Json), "{body}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn client_id_must_match_the_fetched_url() {
        let body = r#"{"client_id":"https://other.example/cimd.json","redirect_uris":["http://127.0.0.1/callback"]}"#;
        let io = FakeIo::ok(vec![public_addr()], http_response("200 OK", "", body));
        assert_eq!(fetch(io).await, Err(CimdFetchError::ClientIdMismatch));
    }

    #[tokio::test(start_paused = true)]
    async fn content_length_over_cap_is_rejected_before_read() {
        let io = FakeIo::ok(
            vec![public_addr()],
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                CIMD_BODY_MAX + 1,
                "x"
            )
            .into_bytes(),
        );
        assert_eq!(fetch(io).await, Err(CimdFetchError::BodyCap));
    }

    #[tokio::test(start_paused = true)]
    async fn streamed_body_over_cap_is_rejected_mid_read() {
        let body = "x".repeat(CIMD_BODY_MAX + 1);
        let io = FakeIo::ok(
            vec![public_addr()],
            format!("HTTP/1.1 200 OK\r\n\r\n{body}").into_bytes(),
        );
        assert_eq!(fetch(io).await, Err(CimdFetchError::BodyCap));
    }

    #[tokio::test(start_paused = true)]
    async fn content_encoding_is_rejected_before_body() {
        let io = FakeIo::ok(
            vec![public_addr()],
            http_response("200 OK", "Content-Encoding: gzip\r\n", &json_body()),
        );
        assert_eq!(fetch(io).await, Err(CimdFetchError::Encoding));
    }

    #[tokio::test(start_paused = true)]
    async fn exhausted_bulkhead_returns_bulkhead() {
        let bulkhead = CimdBulkhead::new();
        let mut held = Vec::new();
        for octet in 1..=16 {
            held.push(
                bulkhead
                    .try_acquire(IpAddr::V4(Ipv4Addr::new(203, 0, 113, octet)))
                    .unwrap(),
            );
        }
        let io = FakeIo::ok(vec![public_addr()], default_response());
        let connect = Arc::clone(&io.connect_calls);
        let (_tx, mut rx) = watch::channel(false);
        let fetch = fetch_cimd_with_io(URL, SOURCE, &bulkhead, &mut rx, io);
        tokio::pin!(fetch);
        tokio::task::yield_now().await;
        tokio::time::advance(CIMD_FETCH_TIMEOUT).await;
        assert_eq!(fetch.await, Err(CimdFetchError::Bulkhead));
        assert_eq!(connect.load(Ordering::SeqCst), 0);
        drop(held);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cancels_and_releases_the_permit() {
        let bulkhead = CimdBulkhead::new();
        let mut io = FakeIo::ok(vec![public_addr()], default_response());
        io.stall_on = Some("resolve");
        let (tx, mut rx) = watch::channel(false);
        let fetch = fetch_cimd_with_io(URL, SOURCE, &bulkhead, &mut rx, io);
        tokio::pin!(fetch);
        tokio::task::yield_now().await;
        tx.send(true).unwrap();
        assert_eq!(fetch.await, Err(CimdFetchError::Cancelled));
        assert!(bulkhead.try_acquire(SOURCE).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn overall_deadline_covers_slow_phases() {
        let mut io = FakeIo::ok(vec![public_addr()], default_response());
        io.delay = Duration::from_secs(4);
        let fetch = fetch(io);
        tokio::pin!(fetch);
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(4)).await;
        assert_eq!(fetch.await, Err(CimdFetchError::Deadline));
    }

    #[tokio::test(start_paused = true)]
    async fn invalid_url_does_not_resolve() {
        let io = FakeIo::ok(vec![public_addr()], default_response());
        let resolve = Arc::clone(&io.resolve_calls);
        let bulkhead = CimdBulkhead::new();
        let (_tx, mut rx) = watch::channel(false);
        let result = fetch_cimd_with_io(
            "http://client.example/cimd.json",
            SOURCE,
            &bulkhead,
            &mut rx,
            io,
        )
        .await;
        assert_eq!(result, Err(CimdFetchError::Url));
        assert_eq!(resolve.load(Ordering::SeqCst), 0);
    }
}
