// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded HTTP/1.1 framing for the journal-local MCP endpoint.

use std::fmt;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

const MAX_REQUEST_LINE: usize = 8 * 1024;
const MAX_HEADER_BLOCK: usize = 32 * 1024;
const MAX_HEADER_FIELD: usize = 8 * 1024;
const MAX_BODY: usize = 64 * 1024;
const MAX_CHUNK_LINE: usize = 8 * 1024;
const HEADER_DEADLINE: Duration = Duration::from_secs(5);
const BODY_DEADLINE: Duration = Duration::from_secs(5);
const IDLE_DEADLINE: Duration = Duration::from_secs(30);

/// One decoded MCP HTTP request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpRequest {
    pub(crate) method: HttpMethod,
    pub(crate) target: String,
    headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
    pub(crate) connection_close: bool,
}

impl HttpRequest {
    /// Return one unambiguous case-insensitive header value.
    pub(crate) fn header(&self, name: &str) -> Result<Option<&str>, Http1Error> {
        let mut values = self
            .headers
            .iter()
            .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str());
        let value = values.next();
        if values.next().is_some() {
            return Err(Http1Error::DuplicateHeader);
        }
        Ok(value)
    }
}

/// HTTP methods admitted by the MCP endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpMethod {
    Post,
    Delete,
}

/// A framed response ready to serialize on an HTTP/1.1 connection.
pub(crate) struct HttpResponse {
    status: u16,
    reason: &'static str,
    content_type: Option<&'static str>,
    pub(crate) body: Vec<u8>,
    pub(crate) session_id: Option<String>,
    pub(crate) close: bool,
}

impl HttpResponse {
    pub(crate) fn json(status: u16, reason: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            reason,
            content_type: Some("application/json"),
            body,
            session_id: None,
            close: false,
        }
    }

    pub(crate) fn empty(status: u16, reason: &'static str) -> Self {
        Self {
            status,
            reason,
            content_type: None,
            body: Vec::new(),
            session_id: None,
            close: false,
        }
    }

    pub(crate) fn error(status: u16, reason: &'static str, message: &'static str) -> Self {
        Self {
            status,
            reason,
            content_type: Some("text/plain; charset=utf-8"),
            body: message.as_bytes().to_vec(),
            session_id: None,
            close: true,
        }
    }

    pub(crate) fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }
}

/// Bounded parsing and writing state for one HTTP/1.1 connection.
pub(crate) struct Http1Connection<S> {
    stream: S,
}

impl<S> Http1Connection<S> {
    pub(crate) fn new(stream: S) -> Self {
        Self { stream }
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    fn into_inner(self) -> S {
        self.stream
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Http1Connection<S> {
    /// Read one complete request. `wait_for_first_byte` applies the keep-alive idle limit.
    pub(crate) async fn read_request(
        &mut self,
        wait_for_first_byte: bool,
    ) -> Result<Option<HttpRequest>, Http1Error> {
        let first = if wait_for_first_byte {
            match timeout(IDLE_DEADLINE, self.read_byte()).await {
                Err(_) => return Err(Http1Error::IdleTimeout),
                Ok(Ok(Some(byte))) => Some(byte),
                Ok(Ok(None)) => return Ok(None),
                Ok(Err(_)) => return Err(Http1Error::Read),
            }
        } else {
            None
        };
        let headers = timeout(HEADER_DEADLINE, self.read_headers(first))
            .await
            .map_err(|_| Http1Error::HeaderTimeout)??;
        let Some(headers) = headers else {
            return Ok(None);
        };
        let parsed = parse_headers(&headers)?;
        let body = timeout(BODY_DEADLINE, self.read_body(parsed.framing))
            .await
            .map_err(|_| Http1Error::BodyTimeout)??;
        Ok(Some(HttpRequest {
            method: parsed.method,
            target: parsed.target,
            headers: parsed.headers,
            body,
            connection_close: parsed.connection_close,
        }))
    }

    /// Serialize one complete response before the next request is read.
    pub(crate) async fn write_response(&mut self, response: &HttpResponse) -> io::Result<()> {
        write_response(&mut self.stream, response).await
    }

    async fn read_headers(&mut self, first: Option<u8>) -> Result<Option<Vec<u8>>, Http1Error> {
        let mut bytes = Vec::with_capacity(512);
        if let Some(first) = first {
            bytes.push(first);
        }
        loop {
            if bytes.ends_with(b"\r\n\r\n") {
                return Ok(Some(bytes));
            }
            if bytes.len() >= MAX_HEADER_BLOCK {
                return Err(Http1Error::HeaderBlockTooLarge);
            }
            match self.read_byte().await.map_err(|_| Http1Error::Read)? {
                Some(byte) => bytes.push(byte),
                None if bytes.is_empty() => return Ok(None),
                None => return Err(Http1Error::UnexpectedEof),
            }
        }
    }

    async fn read_body(&mut self, framing: BodyFraming) -> Result<Vec<u8>, Http1Error> {
        match framing {
            BodyFraming::Empty => Ok(Vec::new()),
            BodyFraming::ContentLength(length) => {
                if length > MAX_BODY {
                    return Err(Http1Error::BodyTooLarge);
                }
                let mut body = vec![0_u8; length];
                self.stream
                    .read_exact(&mut body)
                    .await
                    .map_err(|_| Http1Error::UnexpectedEof)?;
                Ok(body)
            }
            BodyFraming::Chunked => self.read_chunked().await,
        }
    }

    async fn read_chunked(&mut self) -> Result<Vec<u8>, Http1Error> {
        let mut body = Vec::new();
        loop {
            let line = self.read_line(MAX_CHUNK_LINE).await?;
            let size_text = std::str::from_utf8(&line)
                .map_err(|_| Http1Error::InvalidChunk)?
                .split(';')
                .next()
                .ok_or(Http1Error::InvalidChunk)?;
            if size_text.is_empty() {
                return Err(Http1Error::InvalidChunk);
            }
            let size =
                usize::from_str_radix(size_text, 16).map_err(|_| Http1Error::InvalidChunk)?;
            if size == 0 {
                self.read_trailers().await?;
                return Ok(body);
            }
            let total = body
                .len()
                .checked_add(size)
                .ok_or(Http1Error::BodyTooLarge)?;
            if total > MAX_BODY {
                return Err(Http1Error::BodyTooLarge);
            }
            let start = body.len();
            body.resize(total, 0);
            self.stream
                .read_exact(&mut body[start..])
                .await
                .map_err(|_| Http1Error::UnexpectedEof)?;
            if self.read_exact_crlf().await.is_err() {
                return Err(Http1Error::InvalidChunk);
            }
        }
    }

    async fn read_trailers(&mut self) -> Result<(), Http1Error> {
        let mut total = 0_usize;
        loop {
            let line = self.read_line(MAX_HEADER_FIELD).await?;
            total = total
                .checked_add(line.len() + 2)
                .ok_or(Http1Error::HeaderBlockTooLarge)?;
            if total > MAX_HEADER_BLOCK {
                return Err(Http1Error::HeaderBlockTooLarge);
            }
            if line.is_empty() {
                return Ok(());
            }
        }
    }

    async fn read_line(&mut self, limit: usize) -> Result<Vec<u8>, Http1Error> {
        let mut line = Vec::new();
        loop {
            if line.len() > limit {
                return Err(Http1Error::InvalidChunk);
            }
            let Some(byte) = self.read_byte().await.map_err(|_| Http1Error::Read)? else {
                return Err(Http1Error::UnexpectedEof);
            };
            line.push(byte);
            if line.ends_with(b"\r\n") {
                line.truncate(line.len() - 2);
                return Ok(line);
            }
        }
    }

    async fn read_exact_crlf(&mut self) -> Result<(), Http1Error> {
        let mut bytes = [0_u8; 2];
        self.stream
            .read_exact(&mut bytes)
            .await
            .map_err(|_| Http1Error::UnexpectedEof)?;
        if bytes == *b"\r\n" {
            Ok(())
        } else {
            Err(Http1Error::InvalidChunk)
        }
    }

    async fn read_byte(&mut self) -> io::Result<Option<u8>> {
        let mut byte = [0_u8; 1];
        match self.stream.read(&mut byte).await? {
            0 => Ok(None),
            _ => Ok(Some(byte[0])),
        }
    }
}

async fn write_response<S: AsyncWrite + Unpin>(
    writer: &mut S,
    response: &HttpResponse,
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: {}\r\n",
        response.status,
        response.reason,
        response.body.len(),
        if response.close {
            "close"
        } else {
            "keep-alive"
        }
    );
    if let Some(content_type) = response.content_type {
        head.push_str("Content-Type: ");
        head.push_str(content_type);
        head.push_str("\r\n");
    }
    if let Some(session_id) = &response.session_id {
        head.push_str("Mcp-Session-Id: ");
        head.push_str(session_id);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes()).await?;
    writer.write_all(&response.body).await?;
    writer.flush().await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyFraming {
    Empty,
    ContentLength(usize),
    Chunked,
}

struct ParsedHeaders {
    method: HttpMethod,
    target: String,
    headers: Vec<(String, String)>,
    connection_close: bool,
    framing: BodyFraming,
}

fn parse_headers(bytes: &[u8]) -> Result<ParsedHeaders, Http1Error> {
    let content = bytes
        .strip_suffix(b"\r\n\r\n")
        .ok_or(Http1Error::BadRequest)?;
    let mut lines = content.split(|byte| *byte == b'\n');
    let request_line = lines.next().ok_or(Http1Error::BadRequest)?;
    let request_line = request_line.strip_suffix(b"\r").unwrap_or(request_line);
    if request_line.len() > MAX_REQUEST_LINE {
        return Err(Http1Error::RequestLineTooLong);
    }
    let request_line = std::str::from_utf8(request_line).map_err(|_| Http1Error::BadRequest)?;
    let mut parts = request_line.split(' ');
    let method = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("POST"), Some(target), Some("HTTP/1.1"), None) => (HttpMethod::Post, target),
        (Some("DELETE"), Some(target), Some("HTTP/1.1"), None) => (HttpMethod::Delete, target),
        _ => return Err(Http1Error::UnsupportedRequest),
    };
    let mut headers = Vec::new();
    for raw_line in lines {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if line.len() > MAX_HEADER_FIELD {
            return Err(Http1Error::HeaderFieldTooLong);
        }
        let line = std::str::from_utf8(line).map_err(|_| Http1Error::BadRequest)?;
        let (name, value) = line.split_once(':').ok_or(Http1Error::BadRequest)?;
        if name.is_empty() || !name.bytes().all(is_header_name_byte) {
            return Err(Http1Error::BadRequest);
        }
        let value = value.trim_matches([' ', '\t']);
        if name.len() + value.len() > MAX_HEADER_FIELD {
            return Err(Http1Error::HeaderFieldTooLong);
        }
        headers.push((name.to_ascii_lowercase(), value.to_owned()));
    }
    let content_length = header_value(&headers, "content-length")?;
    let transfer_encoding = header_value(&headers, "transfer-encoding")?;
    let framing = match (content_length, transfer_encoding) {
        (Some(_), Some(_)) => return Err(Http1Error::AmbiguousFraming),
        (Some(value), None) => {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(Http1Error::InvalidContentLength);
            }
            let length = value
                .parse::<usize>()
                .map_err(|_| Http1Error::InvalidContentLength)?;
            if length > MAX_BODY {
                return Err(Http1Error::BodyTooLarge);
            }
            BodyFraming::ContentLength(length)
        }
        (None, Some(value)) if value.eq_ignore_ascii_case("chunked") => BodyFraming::Chunked,
        (None, Some(_)) => return Err(Http1Error::UnsupportedTransferEncoding),
        (None, None) => BodyFraming::Empty,
    };
    let connection_close = header_value(&headers, "connection")?.is_some_and(|value| {
        value
            .split(',')
            .any(|item| item.trim().eq_ignore_ascii_case("close"))
    });
    Ok(ParsedHeaders {
        method: method.0,
        target: method.1.to_owned(),
        headers,
        connection_close,
        framing,
    })
}

fn header_value<'a>(
    headers: &'a [(String, String)],
    expected_name: &str,
) -> Result<Option<&'a str>, Http1Error> {
    let mut values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(expected_name))
        .map(|(_, value)| value.as_str());
    let value = values.next();
    if values.next().is_some() {
        return Err(Http1Error::DuplicateHeader);
    }
    Ok(value)
}

fn is_header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Why an HTTP/1.1 request cannot be safely framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Http1Error {
    Read,
    UnexpectedEof,
    IdleTimeout,
    HeaderTimeout,
    BodyTimeout,
    RequestLineTooLong,
    HeaderBlockTooLarge,
    HeaderFieldTooLong,
    BadRequest,
    UnsupportedRequest,
    DuplicateHeader,
    AmbiguousFraming,
    InvalidContentLength,
    UnsupportedTransferEncoding,
    BodyTooLarge,
    InvalidChunk,
}

impl fmt::Display for Http1Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Read => "could not read HTTP/1.1 request",
            Self::UnexpectedEof => "HTTP/1.1 request ended before framing completed",
            Self::IdleTimeout => "HTTP/1.1 keep-alive connection was idle for too long",
            Self::HeaderTimeout => "HTTP/1.1 request headers timed out",
            Self::BodyTimeout => "HTTP/1.1 request body timed out",
            Self::RequestLineTooLong => "HTTP/1.1 request line is too long",
            Self::HeaderBlockTooLarge => "HTTP/1.1 headers are too large",
            Self::HeaderFieldTooLong => "HTTP/1.1 header field is too large",
            Self::BadRequest => "HTTP/1.1 request is malformed",
            Self::UnsupportedRequest => "HTTP/1.1 request method or version is unsupported",
            Self::DuplicateHeader => "HTTP/1.1 request has an ambiguous duplicate header",
            Self::AmbiguousFraming => "HTTP/1.1 request has ambiguous body framing",
            Self::InvalidContentLength => "HTTP/1.1 Content-Length is invalid",
            Self::UnsupportedTransferEncoding => "HTTP/1.1 transfer encoding is unsupported",
            Self::BodyTooLarge => "HTTP/1.1 request body is too large",
            Self::InvalidChunk => "HTTP/1.1 chunked body is malformed",
        };
        formatter.write_str(message)
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::time::{Duration, advance};

    use super::{Http1Connection, Http1Error};

    fn request(headers: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes =
            format!("POST /mcp HTTP/1.1\r\nHost: mcp.test\r\n{headers}\r\n").into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    #[tokio::test]
    async fn fixed_body_bounds_and_no_body_framing_are_enforced() {
        let (mut writer, reader) = tokio::io::duplex(70_000);
        writer
            .write_all(&request("Content-Length: 65537\r\n", b"unread"))
            .await
            .unwrap();
        let mut connection = Http1Connection::new(reader);
        assert_eq!(
            connection.read_request(false).await,
            Err(Http1Error::BodyTooLarge)
        );
        let mut reader = connection.into_inner();
        let mut body = [0_u8; 6];
        reader.read_exact(&mut body).await.unwrap();
        assert_eq!(body, *b"unread");

        let (mut writer, reader) = tokio::io::duplex(128);
        writer.write_all(&request("", b"ignored")).await.unwrap();
        let mut connection = Http1Connection::new(reader);
        assert!(
            connection
                .read_request(false)
                .await
                .unwrap()
                .unwrap()
                .body
                .is_empty()
        );
    }

    #[tokio::test]
    async fn chunked_body_streaming_cutoff_accepts_exact_limit_only() {
        for (length, expected) in [(65_536, true), (65_537, false)] {
            let (mut writer, reader) = tokio::io::duplex(70_000);
            let mut body = format!("{:X}\r\n", length).into_bytes();
            body.extend(std::iter::repeat_n(b'x', length));
            body.extend_from_slice(b"\r\n0\r\n\r\n");
            writer
                .write_all(&request("Transfer-Encoding: chunked\r\n", &body))
                .await
                .unwrap();
            let mut connection = Http1Connection::new(reader);
            let result = connection.read_request(false).await;
            assert_eq!(result.is_ok(), expected);
            if expected {
                assert_eq!(result.unwrap().unwrap().body.len(), length);
            } else {
                assert_eq!(result, Err(Http1Error::BodyTooLarge));
            }
        }
    }

    #[tokio::test]
    async fn header_limits_and_ambiguous_framing_are_rejected() {
        let line = format!("POST /{} HTTP/1.1\r\n\r\n", "x".repeat(8 * 1024));
        let (mut writer, reader) = tokio::io::duplex(10_000);
        writer.write_all(line.as_bytes()).await.unwrap();
        let mut connection = Http1Connection::new(reader);
        assert_eq!(
            connection.read_request(false).await,
            Err(Http1Error::RequestLineTooLong)
        );

        let (mut writer, reader) = tokio::io::duplex(40_000);
        writer
            .write_all(&request(
                &format!("X-Test: {}\r\n", "x".repeat(8 * 1024)),
                b"",
            ))
            .await
            .unwrap();
        let mut connection = Http1Connection::new(reader);
        assert_eq!(
            connection.read_request(false).await,
            Err(Http1Error::HeaderFieldTooLong)
        );

        let (mut writer, reader) = tokio::io::duplex(128);
        writer
            .write_all(&request(
                "Content-Length: 0\r\nTransfer-Encoding: chunked\r\n",
                b"",
            ))
            .await
            .unwrap();
        let mut connection = Http1Connection::new(reader);
        assert_eq!(
            connection.read_request(false).await,
            Err(Http1Error::AmbiguousFraming)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn header_body_and_keep_alive_deadlines_are_absolute() {
        let (mut writer, reader) = tokio::io::duplex(128);
        let mut connection = Http1Connection::new(reader);
        let header = tokio::spawn(async move { connection.read_request(false).await });
        writer.write_all(b"P").await.unwrap();
        advance(Duration::from_secs(4)).await;
        writer.write_all(b"O").await.unwrap();
        advance(Duration::from_secs(1)).await;
        assert_eq!(header.await.unwrap(), Err(Http1Error::HeaderTimeout));

        let (mut writer, reader) = tokio::io::duplex(128);
        writer
            .write_all(&request("Content-Length: 2\r\n", b"x"))
            .await
            .unwrap();
        let mut connection = Http1Connection::new(reader);
        let body = tokio::spawn(async move { connection.read_request(false).await });
        tokio::task::yield_now().await;
        advance(Duration::from_secs(5)).await;
        assert_eq!(body.await.unwrap(), Err(Http1Error::BodyTimeout));

        let (_writer, reader) = tokio::io::duplex(128);
        let mut connection = Http1Connection::new(reader);
        let idle = tokio::spawn(async move { connection.read_request(true).await });
        tokio::task::yield_now().await;
        advance(Duration::from_secs(30)).await;
        assert_eq!(idle.await.unwrap(), Err(Http1Error::IdleTimeout));
    }
}
