// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fail-closed HTTPS URL validation, unusable-IP classification, and
//! DNS-rebinding-proof SSRF-safe HTTPS client.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

pub const MAX_HTTPS_URL_BYTES: usize = 2048;
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const DEFAULT_MAX_HEADER_BYTES: usize = 16 * 1024;
pub const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;
pub const CIMD_MAX_BODY_BYTES: usize = 5 * 1024;

/// A validated HTTPS URL with a public DNS hostname on default port 443.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHttpsUrl {
    pub host: String,
    pub path: String,
    pub query: Option<String>,
}

impl ParsedHttpsUrl {
    /// Canonical origin string (`https://<host>`).
    pub fn origin(&self) -> String {
        format!("https://{}", self.host)
    }

    /// Request target (`<path>` or `<path>?<query>`).
    pub fn request_target(&self) -> String {
        match &self.query {
            Some(query) => format!("{}?{query}", self.path),
            None => self.path.clone(),
        }
    }

    /// URL without query or fragment (RFC 9449 DPoP `htu`).
    pub fn without_query(&self) -> String {
        format!("https://{}{}", self.host, self.path)
    }
}

/// Why an HTTPS URL failed fail-closed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UrlError;

impl fmt::Display for UrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid HTTPS URL")
    }
}

impl std::error::Error for UrlError {}

/// Parse and admit one fail-closed HTTPS URL with a DNS hostname (no IP literals,
/// no userinfo, no fragments, no non-443 ports, visible ASCII only).
pub fn validate_https_url(raw: &str) -> Result<ParsedHttpsUrl, UrlError> {
    if raw.is_empty() || raw.len() > MAX_HTTPS_URL_BYTES {
        return Err(UrlError);
    }
    if raw.contains('#') || raw.bytes().any(|byte| !byte.is_ascii_graphic()) {
        return Err(UrlError);
    }
    let rest = raw.strip_prefix("https://").ok_or(UrlError)?;
    if rest.contains('@') {
        return Err(UrlError);
    }
    let (authority, path_query) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(UrlError);
    }
    let (host, port) = split_host_port(authority).ok_or(UrlError)?;
    if !is_valid_dns_hostname(host) {
        return Err(UrlError);
    }
    if port.is_some_and(|p| p != 443) {
        return Err(UrlError);
    }
    let (path, query) = split_path_query(path_query).ok_or(UrlError)?;
    Ok(ParsedHttpsUrl {
        host: host.to_owned(),
        path,
        query,
    })
}

fn split_host_port(authority: &str) -> Option<(&str, Option<u16>)> {
    if authority.starts_with('[') {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port))
            if !host.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let port = port.parse::<u16>().ok()?;
            Some((host, Some(port)))
        }
        Some(_) => None,
        None => Some((authority, None)),
    }
}

fn is_valid_dns_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    if host.parse::<Ipv4Addr>().is_ok() {
        return false;
    }
    if !host
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
    {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.is_empty() {
        return false;
    }
    for label in &labels {
        if label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-') {
            return false;
        }
    }
    // Every valid public DNS TLD contains at least one ASCII letter (including punycode `xn--`),
    // which rejects numeric/octal/hex IPv4 literal spellings such as `2130706433` or `0177.0.0.1`.
    labels
        .last()
        .is_some_and(|tld| tld.bytes().any(|b| b.is_ascii_lowercase()))
}

fn split_path_query(path_query: &str) -> Option<(String, Option<String>)> {
    if !path_query.starts_with('/') {
        return None;
    }
    match path_query.split_once('?') {
        Some((path, query)) => Some((path.to_owned(), Some(query.to_owned()))),
        None => Some((path_query.to_owned(), None)),
    }
}

/// Why a resolved address must not be contacted by outbound SSRF-safe fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnusableIpClass {
    Unspecified,
    Loopback,
    Private,
    LinkLocal,
    Multicast,
    ReservedDocumentation,
    Broadcast,
}

/// Map IPv4-mapped IPv6 (`::ffff:a.b.c.d`) to IPv4; leave every other address unchanged.
pub fn canonicalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        IpAddr::V4(_) => ip,
    }
}

/// Classify an IP address that must not be contacted by outbound OAuth HTTP requests.
pub fn classify_unusable_ip(ip: IpAddr) -> Option<UnusableIpClass> {
    match canonicalize_ip(ip) {
        IpAddr::V4(address) => classify_v4(address),
        IpAddr::V6(address) => classify_v6(address),
    }
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
    // 2001:db8::/32 documentation; 100::/64 discard; 64:ff9b::/96 & 64:ff9b:1::/48 NAT64.
    if (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0)
        || (segments[0] == 0x0064 && segments[1] == 0xff9b)
    {
        return Some(UnusableIpClass::ReservedDocumentation);
    }
    // Deprecated IPv4-compatible IPv6 ::a.b.c.d (0:0:0:0:0:0:x:y where not :: or ::1).
    if segments[0..6] == [0, 0, 0, 0, 0, 0] {
        return Some(UnusableIpClass::ReservedDocumentation);
    }
    None
}

fn in_range(bits: u32, start: Ipv4Addr, end: Ipv4Addr) -> bool {
    let start = u32::from(start);
    let end = u32::from(end);
    start <= bits && bits <= end
}

/// HTTP request method supported by the SSRF-safe OAuth client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// An outbound OAuth HTTP request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    pub max_body_bytes: usize,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: DEFAULT_REQUEST_TIMEOUT,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }

    pub fn post(url: impl Into<String>, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            method: HttpMethod::Post,
            url: url.into(),
            headers: vec![("Content-Type".to_owned(), content_type.to_owned())],
            body: Some(body),
            timeout: DEFAULT_REQUEST_TIMEOUT,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_max_body_bytes(mut self, max_body_bytes: usize) -> Self {
        self.max_body_bytes = max_body_bytes;
        self
    }
}

/// An HTTP response returned by [`OAuthHttpClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    /// Lowercased response header names and trimmed values.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Return the first header matching `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&lower))
            .map(|(_, v)| v.as_str())
    }

    /// Return all headers matching `name` (case-insensitive).
    pub fn headers_named<'a>(&'a self, name: &str) -> Vec<&'a str> {
        let lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(&lower))
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

/// Class-only error type for SSRF-safe HTTPS requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrfFetchError {
    Url,
    InvalidHeader,
    Resolve,
    UnusableAddress,
    Connect,
    Tls,
    Redirect,
    Encoding,
    BodyCap,
    MalformedResponse,
    Deadline,
}

impl fmt::Display for SsrfFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Url => "request URL is invalid or not HTTPS",
            Self::InvalidHeader => "request header contains forbidden characters",
            Self::Resolve => "origin hostname could not be resolved",
            Self::UnusableAddress => "origin hostname resolved to an unusable IP address",
            Self::Connect => "origin could not be connected",
            Self::Tls => "origin TLS handshake failed",
            Self::Redirect => "origin returned a redirect",
            Self::Encoding => "origin returned a compressed content encoding",
            Self::BodyCap => "origin response exceeded size cap",
            Self::MalformedResponse => "origin returned a malformed HTTP response",
            Self::Deadline => "request exceeded its deadline",
        })
    }
}

impl std::error::Error for SsrfFetchError {}

/// Synchronous HTTP client abstraction used by discovery, registration, exchange,
/// refresh, revocation, device code, and PAR.
pub trait OAuthHttpClient: Send + Sync {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError>;
}

/// Pluggable DNS resolve + direct-SocketAddr TLS stream provider for unit testing.
pub trait HttpsAttemptIo {
    type Stream: Read + Write;

    fn resolve(&mut self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>>;
    fn connect_tls(
        &mut self,
        address: SocketAddr,
        server_name: &str,
        connect_timeout: Duration,
        io_timeout: Duration,
    ) -> Result<Self::Stream, SsrfFetchError>;
}

struct StdRustlsHttpsIo {
    tls_config: Arc<ClientConfig>,
}

impl StdRustlsHttpsIo {
    fn new() -> Result<Self, SsrfFetchError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .map_err(|_| SsrfFetchError::Tls)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            tls_config: Arc::new(config),
        })
    }
}

impl HttpsAttemptIo for StdRustlsHttpsIo {
    type Stream = StreamOwned<ClientConnection, TcpStream>;

    fn resolve(&mut self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        (host, port).to_socket_addrs().map(Iterator::collect)
    }

    fn connect_tls(
        &mut self,
        address: SocketAddr,
        server_name: &str,
        connect_timeout: Duration,
        io_timeout: Duration,
    ) -> Result<Self::Stream, SsrfFetchError> {
        let tcp = TcpStream::connect_timeout(&address, connect_timeout)
            .map_err(|err| classify_io_err(&err, SsrfFetchError::Connect))?;
        let _ = tcp.set_read_timeout(Some(io_timeout));
        let _ = tcp.set_write_timeout(Some(io_timeout));
        let server_name =
            ServerName::try_from(server_name.to_owned()).map_err(|_| SsrfFetchError::Tls)?;
        let conn = ClientConnection::new(Arc::clone(&self.tls_config), server_name)
            .map_err(|_| SsrfFetchError::Tls)?;
        let mut stream = StreamOwned::new(conn, tcp);
        // Complete the TLS handshake explicitly before sending request bytes.
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .map_err(|err| classify_io_err(&err, SsrfFetchError::Tls))?;
        }
        Ok(stream)
    }
}

fn classify_io_err(error: &io::Error, fallback: SsrfFetchError) -> SsrfFetchError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => SsrfFetchError::Deadline,
        _ => fallback,
    }
}

/// Production DNS-rebinding-proof SSRF-safe HTTPS client.
#[derive(Debug, Default, Clone)]
pub struct SsrfHttpsClient;

impl SsrfHttpsClient {
    pub fn new() -> Self {
        Self
    }
}

impl OAuthHttpClient for SsrfHttpsClient {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
        let mut io = StdRustlsHttpsIo::new()?;
        send_with_io(request, &mut io)
    }
}

/// Execute one SSRF-safe HTTPS request using a caller-provided `HttpsAttemptIo`.
pub fn send_with_io<IO: HttpsAttemptIo>(
    request: &HttpRequest,
    io: &mut IO,
) -> Result<HttpResponse, SsrfFetchError> {
    let parsed = validate_https_url(&request.url).map_err(|_| SsrfFetchError::Url)?;
    validate_request_headers(&request.headers)?;

    let deadline = Instant::now() + request.timeout;
    let addresses = io
        .resolve(&parsed.host, 443)
        .map_err(|_| SsrfFetchError::Resolve)?;
    if addresses.is_empty() {
        return Err(SsrfFetchError::Resolve);
    }
    // Fail closed if ANY resolved address is unusable (blocks dual-record DNS rebinding).
    if addresses
        .iter()
        .any(|address| classify_unusable_ip(address.ip()).is_some())
    {
        return Err(SsrfFetchError::UnusableAddress);
    }

    let mut stream = None;
    let mut last_err = SsrfFetchError::Connect;
    for address in addresses {
        let rem = remaining(deadline)?;
        let connect_timeout = rem.min(DEFAULT_CONNECT_TIMEOUT);
        match io.connect_tls(address, &parsed.host, connect_timeout, rem) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(err @ (SsrfFetchError::Deadline | SsrfFetchError::Tls)) => return Err(err),
            Err(err) => last_err = err,
        }
    }
    let mut stream = stream.ok_or(last_err)?;

    let wire = build_http1_request(&parsed, request);
    check_deadline(deadline)?;
    stream
        .write_all(&wire)
        .map_err(|err| classify_io_err(&err, SsrfFetchError::Connect))?;
    stream
        .flush()
        .map_err(|err| classify_io_err(&err, SsrfFetchError::Connect))?;

    let mut reader = BufReader::new(stream);
    let head = read_headers(&mut reader, deadline)?;
    let (status, headers) = parse_headers(&head)?;
    if (300..400).contains(&status) {
        return Err(SsrfFetchError::Redirect);
    }
    if headers
        .iter()
        .any(|(name, value)| name == "content-encoding" && !value.eq_ignore_ascii_case("identity"))
    {
        return Err(SsrfFetchError::Encoding);
    }

    let body = if status == 204 || status == 304 || (100..200).contains(&status) {
        Vec::new()
    } else if is_chunked(&headers)? {
        read_chunked_body(&mut reader, deadline, request.max_body_bytes)?
    } else {
        read_body(
            &mut reader,
            deadline,
            content_length(&headers)?,
            request.max_body_bytes,
        )?
    };

    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn validate_request_headers(headers: &[(String, String)]) -> Result<(), SsrfFetchError> {
    for (name, value) in headers {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(SsrfFetchError::InvalidHeader);
        }
        if value
            .bytes()
            .any(|b| b == b'\r' || b == b'\n' || b == b'\0')
        {
            return Err(SsrfFetchError::InvalidHeader);
        }
        if name.eq_ignore_ascii_case("host")
            || name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
            || name.eq_ignore_ascii_case("connection")
            || name.eq_ignore_ascii_case("accept-encoding")
        {
            return Err(SsrfFetchError::InvalidHeader);
        }
    }
    Ok(())
}

fn build_http1_request(parsed: &ParsedHttpsUrl, request: &HttpRequest) -> Vec<u8> {
    let target = parsed.request_target();
    let has_accept = request
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("accept"));
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nAccept-Encoding: identity\r\nConnection: close\r\n",
        request.method.as_str(),
        target,
        parsed.host
    );
    if !has_accept {
        head.push_str("Accept: application/json\r\n");
    }
    for (name, value) in &request.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    if let Some(body) = &request.body {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    if let Some(body) = &request.body {
        bytes.extend_from_slice(body);
    }
    bytes
}

fn remaining(deadline: Instant) -> Result<Duration, SsrfFetchError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or(SsrfFetchError::Deadline)
}

fn check_deadline(deadline: Instant) -> Result<(), SsrfFetchError> {
    remaining(deadline).map(|_| ())
}

fn read_headers<R: BufRead>(reader: &mut R, deadline: Instant) -> Result<Vec<u8>, SsrfFetchError> {
    let mut bytes = Vec::new();
    let mut line = Vec::new();
    loop {
        check_deadline(deadline)?;
        line.clear();
        let remaining_cap = DEFAULT_MAX_HEADER_BYTES.saturating_sub(bytes.len());
        let read = reader
            .by_ref()
            .take(remaining_cap.saturating_add(1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|err| classify_io_err(&err, SsrfFetchError::MalformedResponse))?;
        if read == 0 {
            return Err(SsrfFetchError::MalformedResponse);
        }
        if bytes.len().saturating_add(read) > DEFAULT_MAX_HEADER_BYTES {
            return Err(SsrfFetchError::MalformedResponse);
        }
        bytes.extend_from_slice(&line);
        if bytes.ends_with(b"\r\n\r\n") {
            return Ok(bytes);
        }
    }
}

fn parse_headers(bytes: &[u8]) -> Result<(u16, Vec<(String, String)>), SsrfFetchError> {
    let text = std::str::from_utf8(bytes).map_err(|_| SsrfFetchError::MalformedResponse)?;
    let text = text
        .strip_suffix("\r\n\r\n")
        .ok_or(SsrfFetchError::MalformedResponse)?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().ok_or(SsrfFetchError::MalformedResponse)?;
    let mut status_parts = status_line.split_whitespace();
    let version = status_parts
        .next()
        .ok_or(SsrfFetchError::MalformedResponse)?;
    if !version.starts_with("HTTP/1.") {
        return Err(SsrfFetchError::MalformedResponse);
    }
    let status = status_parts
        .next()
        .and_then(|code| code.parse::<u16>().ok())
        .filter(|code| (100..=599).contains(code))
        .ok_or(SsrfFetchError::MalformedResponse)?;
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or(SsrfFetchError::MalformedResponse)?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    Ok((status, headers))
}

fn content_length(headers: &[(String, String)]) -> Result<Option<usize>, SsrfFetchError> {
    let mut lengths = headers.iter().filter(|(name, _)| name == "content-length");
    let Some((_, value)) = lengths.next() else {
        return Ok(None);
    };
    if lengths.next().is_some() {
        return Err(SsrfFetchError::MalformedResponse);
    }
    value
        .parse::<usize>()
        .map(Some)
        .map_err(|_| SsrfFetchError::MalformedResponse)
}

fn is_chunked(headers: &[(String, String)]) -> Result<bool, SsrfFetchError> {
    let mut encodings = headers
        .iter()
        .filter(|(name, _)| name == "transfer-encoding");
    let Some((_, value)) = encodings.next() else {
        return Ok(false);
    };
    if encodings.next().is_some()
        || !value.eq_ignore_ascii_case("chunked")
        || headers.iter().any(|(name, _)| name == "content-length")
    {
        return Err(SsrfFetchError::MalformedResponse);
    }
    Ok(true)
}

fn read_chunk_line<R: BufRead>(
    reader: &mut R,
    deadline: Instant,
    framing_remaining: &mut usize,
) -> Result<Vec<u8>, SsrfFetchError> {
    check_deadline(deadline)?;
    let mut line = Vec::new();
    let read = reader
        .by_ref()
        .take(framing_remaining.saturating_add(1) as u64)
        .read_until(b'\n', &mut line)
        .map_err(|err| classify_io_err(&err, SsrfFetchError::MalformedResponse))?;
    if read == 0 {
        return Err(SsrfFetchError::MalformedResponse);
    }
    *framing_remaining = framing_remaining
        .checked_sub(read)
        .ok_or(SsrfFetchError::MalformedResponse)?;
    if !line.ends_with(b"\r\n") {
        return Err(SsrfFetchError::MalformedResponse);
    }
    line.truncate(line.len() - 2);
    Ok(line)
}

fn read_chunked_body<R: BufRead>(
    reader: &mut R,
    deadline: Instant,
    max_body_bytes: usize,
) -> Result<Vec<u8>, SsrfFetchError> {
    let mut body = Vec::new();
    let mut framing_remaining = DEFAULT_MAX_HEADER_BYTES;
    loop {
        let line = read_chunk_line(reader, deadline, &mut framing_remaining)?;
        let size = line.split(|byte| *byte == b';').next().unwrap_or_default();
        if size.is_empty() || !size.iter().all(u8::is_ascii_hexdigit) {
            return Err(SsrfFetchError::MalformedResponse);
        }
        let length = usize::from_str_radix(
            std::str::from_utf8(size).map_err(|_| SsrfFetchError::MalformedResponse)?,
            16,
        )
        .map_err(|_| SsrfFetchError::MalformedResponse)?;
        if length == 0 {
            loop {
                let trailer = read_chunk_line(reader, deadline, &mut framing_remaining)?;
                if trailer.is_empty() {
                    return Ok(body);
                }
                if !trailer.contains(&b':') {
                    return Err(SsrfFetchError::MalformedResponse);
                }
            }
        }
        let end = body
            .len()
            .checked_add(length)
            .ok_or(SsrfFetchError::BodyCap)?;
        if end > max_body_bytes {
            return Err(SsrfFetchError::BodyCap);
        }
        let start = body.len();
        body.resize(end, 0);
        check_deadline(deadline)?;
        reader
            .read_exact(&mut body[start..])
            .map_err(|err| classify_io_err(&err, SsrfFetchError::MalformedResponse))?;
        framing_remaining = framing_remaining
            .checked_sub(2)
            .ok_or(SsrfFetchError::MalformedResponse)?;
        let mut terminator = [0_u8; 2];
        reader
            .read_exact(&mut terminator)
            .map_err(|err| classify_io_err(&err, SsrfFetchError::MalformedResponse))?;
        if terminator != *b"\r\n" {
            return Err(SsrfFetchError::MalformedResponse);
        }
    }
}

fn read_body<R: Read>(
    reader: &mut R,
    deadline: Instant,
    content_length: Option<usize>,
    max_body_bytes: usize,
) -> Result<Vec<u8>, SsrfFetchError> {
    if let Some(length) = content_length {
        if length > max_body_bytes {
            return Err(SsrfFetchError::BodyCap);
        }
        check_deadline(deadline)?;
        let mut body = vec![0_u8; length];
        reader
            .read_exact(&mut body)
            .map_err(|err| classify_io_err(&err, SsrfFetchError::MalformedResponse))?;
        return Ok(body);
    }
    let mut body = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        check_deadline(deadline)?;
        let read = reader
            .read(&mut chunk)
            .map_err(|err| classify_io_err(&err, SsrfFetchError::MalformedResponse))?;
        if read == 0 {
            return Ok(body);
        }
        if body.len().saturating_add(read) > max_body_bytes {
            return Err(SsrfFetchError::BodyCap);
        }
        body.extend_from_slice(&chunk[..read]);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    struct FakeStream {
        written: Vec<u8>,
        response: Cursor<Vec<u8>>,
    }

    impl FakeStream {
        fn new(response: &[u8]) -> Self {
            Self {
                written: Vec::new(),
                response: Cursor::new(response.to_vec()),
            }
        }
    }

    impl Read for FakeStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.response.read(buf)
        }
    }

    impl Write for FakeStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FakeIo {
        resolved: Vec<SocketAddr>,
        response: Vec<u8>,
        connected_addrs: Vec<SocketAddr>,
        connected_server_names: Vec<String>,
    }

    impl FakeIo {
        fn new(resolved: Vec<SocketAddr>, response: &[u8]) -> Self {
            Self {
                resolved,
                response: response.to_vec(),
                connected_addrs: Vec::new(),
                connected_server_names: Vec::new(),
            }
        }
    }

    impl HttpsAttemptIo for FakeIo {
        type Stream = FakeStream;

        fn resolve(&mut self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
            Ok(self.resolved.clone())
        }

        fn connect_tls(
            &mut self,
            address: SocketAddr,
            server_name: &str,
            _connect_timeout: Duration,
            _io_timeout: Duration,
        ) -> Result<Self::Stream, SsrfFetchError> {
            self.connected_addrs.push(address);
            self.connected_server_names.push(server_name.to_owned());
            Ok(FakeStream::new(&self.response))
        }
    }

    #[test]
    fn https_url_validation_admits_public_dns_and_rejects_smuggling() {
        let parsed =
            validate_https_url("https://mcp.plaud.ai/.well-known/oauth-authorization-server")
                .expect("valid https url");
        assert_eq!(parsed.host, "mcp.plaud.ai");
        assert_eq!(parsed.path, "/.well-known/oauth-authorization-server");
        assert_eq!(parsed.query, None);

        let parsed443 = validate_https_url("https://example.com:443/token?x=1").unwrap();
        assert_eq!(parsed443.host, "example.com");
        assert_eq!(parsed443.query.as_deref(), Some("x=1"));

        for rejected in [
            "",
            "http://example.com/meta",
            "https://user@example.com/meta",
            "https://example.com/meta#frag",
            "https://127.0.0.1/meta",
            "https://[::1]/meta",
            "https://2130706433/meta",
            "https://0177.0.0.1/meta",
            "https://example.com:8443/meta",
            "https://Example.com/meta",
            "https://example.com./meta",
            "https://-bad.example.com/meta",
            "https://example.com/meta\r\nHost: evil.example",
            "https://example.com/meta with space",
            &format!("https://example.com/{}", "a".repeat(MAX_HTTPS_URL_BYTES)),
        ] {
            assert!(
                validate_https_url(rejected).is_err(),
                "expected reject for {rejected:?}"
            );
        }
    }

    #[test]
    fn unusable_ip_classification_blocks_private_cgnat_loopback_link_local_and_mapped_v6() {
        for blocked in [
            "0.0.0.0",
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "224.0.0.1",
            "192.0.2.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "64:ff9b::10.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:100.64.0.5",
        ] {
            let ip: IpAddr = blocked.parse().unwrap();
            assert!(
                classify_unusable_ip(ip).is_some(),
                "expected {blocked} to be classified unusable"
            );
        }

        for public in [
            "8.8.8.8",
            "1.1.1.1",
            "172.32.0.1",
            "2001:4860:4860::8888",
            "::ffff:8.8.8.8",
        ] {
            let ip: IpAddr = public.parse().unwrap();
            assert_eq!(classify_unusable_ip(ip), None, "expected {public} usable");
        }
    }

    #[test]
    fn ssrf_fetch_refuses_unusable_ip_and_dual_record_rebinding_without_connecting() {
        let req =
            HttpRequest::get("https://auth.example.com/.well-known/oauth-authorization-server");
        let public_addr: SocketAddr = "93.184.216.34:443".parse().unwrap();
        let metadata_addr: SocketAddr = "169.254.169.254:443".parse().unwrap();
        let mapped_loopback: SocketAddr = "[::ffff:127.0.0.1]:443".parse().unwrap();

        for addrs in [
            vec![metadata_addr],
            vec![public_addr, metadata_addr],
            vec![mapped_loopback],
        ] {
            let mut io = FakeIo::new(addrs, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            assert_eq!(
                send_with_io(&req, &mut io),
                Err(SsrfFetchError::UnusableAddress)
            );
            assert!(
                io.connected_addrs.is_empty(),
                "must never connect to unusable IP"
            );
        }
    }

    #[test]
    fn ssrf_fetch_connects_directly_to_validated_socket_addr_and_refuses_redirects_compression_and_oversized_bodies()
     {
        let public_addr: SocketAddr = "93.184.216.34:443".parse().unwrap();
        let req =
            HttpRequest::get("https://auth.example.com/.well-known/oauth-authorization-server")
                .with_max_body_bytes(32);

        // Happy path + chunked decoding + direct SocketAddr verification.
        let mut io = FakeIo::new(
            vec![public_addr],
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nDPoP-Nonce: n-123\r\n\r\n10\r\n{\"issuer\":\"ok\"}\n\r\n0\r\n\r\n",
        );
        let resp = send_with_io(&req, &mut io).expect("valid chunked response");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"{\"issuer\":\"ok\"}\n");
        assert_eq!(resp.header("dpop-nonce"), Some("n-123"));
        assert_eq!(io.connected_addrs, vec![public_addr]);
        assert_eq!(io.connected_server_names, vec!["auth.example.com"]);

        // Redirect refused.
        let mut io_redir = FakeIo::new(
            vec![public_addr],
            b"HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1/\r\nContent-Length: 0\r\n\r\n",
        );
        assert_eq!(
            send_with_io(&req, &mut io_redir),
            Err(SsrfFetchError::Redirect)
        );

        // Compressed zip-bomb refused.
        let mut io_gzip = FakeIo::new(
            vec![public_addr],
            b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 2\r\n\r\n{}",
        );
        assert_eq!(
            send_with_io(&req, &mut io_gzip),
            Err(SsrfFetchError::Encoding)
        );

        // Oversized Content-Length refused.
        let mut io_big = FakeIo::new(
            vec![public_addr],
            b"HTTP/1.1 200 OK\r\nContent-Length: 64\r\n\r\n0123456789012345678901234567890123456789012345678901234567890123",
        );
        assert_eq!(
            send_with_io(&req, &mut io_big),
            Err(SsrfFetchError::BodyCap)
        );

        // Oversized chunked body refused.
        let mut io_big_chunk = FakeIo::new(
            vec![public_addr],
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n21\r\n012345678901234567890123456789012\r\n0\r\n\r\n",
        );
        assert_eq!(
            send_with_io(&req, &mut io_big_chunk),
            Err(SsrfFetchError::BodyCap)
        );

        // 204 No Content and explicit identity content-encoding succeed cleanly.
        let mut io_204 = FakeIo::new(
            vec![public_addr],
            b"HTTP/1.1 204 No Content\r\nContent-Encoding: identity\r\n\r\n",
        );
        let resp_204 = send_with_io(&req, &mut io_204).expect("204 no content");
        assert_eq!(resp_204.status, 204);
        assert!(resp_204.body.is_empty());

        // Over-cap header line with no newline terminates cleanly at the header cap.
        let oversized_line = vec![b'A'; DEFAULT_MAX_HEADER_BYTES + 128];
        let mut io_huge_header = FakeIo::new(vec![public_addr], &oversized_line);
        assert_eq!(
            send_with_io(&req, &mut io_huge_header),
            Err(SsrfFetchError::MalformedResponse)
        );
    }
}
