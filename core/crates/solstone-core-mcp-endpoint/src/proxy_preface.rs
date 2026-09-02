// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict PROXY protocol v1 admission for the journal-local MCP listener.

use std::fmt;
use std::net::{IpAddr, SocketAddr};

use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_PROXY_V1_LINE_BYTES: usize = 107;

/// A validated proxy source address and bytes already read beyond its preface.
#[derive(Eq, PartialEq)]
pub(crate) struct ParsedPreface {
    pub(crate) source: SocketAddr,
    pub(crate) trailing: Vec<u8>,
}

/// A pre-TLS PROXY protocol v1 admission failure.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProxyPrefaceError {
    Missing,
    PrematureClose,
    Overlong,
    InvalidSignature,
    UnsupportedProtocol,
    InvalidFieldCount,
    InvalidSourceAddress,
    InvalidDestinationAddress,
    InvalidSourcePort,
    InvalidDestinationPort,
    Io,
}

impl fmt::Display for ProxyPrefaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Missing => "missing PROXY v1 preface",
            Self::PrematureClose => "connection closed during PROXY v1 preface",
            Self::Overlong => "PROXY v1 preface exceeds its maximum length",
            Self::InvalidSignature => "invalid PROXY v1 signature",
            Self::UnsupportedProtocol => "unsupported PROXY v1 protocol",
            Self::InvalidFieldCount => "invalid PROXY v1 field count",
            Self::InvalidSourceAddress => "invalid PROXY v1 source address",
            Self::InvalidDestinationAddress => "invalid PROXY v1 destination address",
            Self::InvalidSourcePort => "invalid PROXY v1 source port",
            Self::InvalidDestinationPort => "invalid PROXY v1 destination port",
            Self::Io => "could not read PROXY v1 preface",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ProxyPrefaceError {}

/// Read one complete and strict PROXY protocol v1 line.
pub(crate) async fn parse_preface<R>(reader: &mut R) -> Result<ParsedPreface, ProxyPrefaceError>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(MAX_PROXY_V1_LINE_BYTES);
    let mut read_buffer = [0_u8; 1024];

    loop {
        let read = reader
            .read(&mut read_buffer)
            .await
            .map_err(|_| ProxyPrefaceError::Io)?;
        if read == 0 {
            return Err(if bytes.is_empty() {
                ProxyPrefaceError::Missing
            } else {
                ProxyPrefaceError::PrematureClose
            });
        }
        bytes.extend_from_slice(&read_buffer[..read]);

        if let Some(terminator) = bytes.windows(2).position(|pair| pair == b"\r\n") {
            let line_length = terminator + 2;
            if line_length > MAX_PROXY_V1_LINE_BYTES {
                return Err(ProxyPrefaceError::Overlong);
            }
            let trailing = bytes.split_off(line_length);
            let source = parse_line(&bytes)?;
            return Ok(ParsedPreface { source, trailing });
        }
        if bytes.len() > MAX_PROXY_V1_LINE_BYTES {
            return Err(ProxyPrefaceError::Overlong);
        }
    }
}

fn parse_line(bytes: &[u8]) -> Result<SocketAddr, ProxyPrefaceError> {
    let line = std::str::from_utf8(bytes).map_err(|_| ProxyPrefaceError::InvalidSignature)?;
    let line = line
        .strip_suffix("\r\n")
        .ok_or(ProxyPrefaceError::InvalidSignature)?;
    let fields = line.split(' ').collect::<Vec<_>>();
    let Some(signature) = fields.first() else {
        return Err(ProxyPrefaceError::InvalidSignature);
    };
    if *signature != "PROXY" {
        return Err(ProxyPrefaceError::InvalidSignature);
    }
    let Some(protocol) = fields.get(1) else {
        return Err(ProxyPrefaceError::InvalidFieldCount);
    };
    let expected_v4 = match *protocol {
        "TCP4" => true,
        "TCP6" => false,
        _ => return Err(ProxyPrefaceError::UnsupportedProtocol),
    };
    let [_, _, source, destination, source_port, destination_port] = fields.as_slice() else {
        return Err(ProxyPrefaceError::InvalidFieldCount);
    };
    let source = source
        .parse::<IpAddr>()
        .map_err(|_| ProxyPrefaceError::InvalidSourceAddress)?;
    let destination = destination
        .parse::<IpAddr>()
        .map_err(|_| ProxyPrefaceError::InvalidDestinationAddress)?;
    if source.is_ipv4() != expected_v4 {
        return Err(ProxyPrefaceError::InvalidSourceAddress);
    }
    if destination.is_ipv4() != expected_v4 {
        return Err(ProxyPrefaceError::InvalidDestinationAddress);
    }
    let source_port = source_port
        .parse::<u16>()
        .map_err(|_| ProxyPrefaceError::InvalidSourcePort)?;
    let _destination_port = destination_port
        .parse::<u16>()
        .map_err(|_| ProxyPrefaceError::InvalidDestinationPort)?;
    Ok(SocketAddr::new(source, source_port))
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, ReadBuf};

    use super::{ParsedPreface, ProxyPrefaceError, parse_preface};

    struct ChunkedReader {
        chunks: VecDeque<Vec<u8>>,
        offset: usize,
    }

    impl ChunkedReader {
        fn new(chunks: &[&[u8]]) -> Self {
            Self {
                chunks: chunks.iter().map(|chunk| chunk.to_vec()).collect(),
                offset: 0,
            }
        }
    }

    impl AsyncRead for ChunkedReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            read_buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            loop {
                let Some(chunk) = self.chunks.front() else {
                    return Poll::Ready(Ok(()));
                };
                if self.offset == chunk.len() {
                    self.chunks.pop_front();
                    self.offset = 0;
                    continue;
                }
                let count = read_buffer.remaining().min(chunk.len() - self.offset);
                read_buffer.put_slice(&chunk[self.offset..self.offset + count]);
                self.offset += count;
                return Poll::Ready(Ok(()));
            }
        }
    }

    async fn parse(chunks: &[&[u8]]) -> Result<ParsedPreface, ProxyPrefaceError> {
        parse_preface(&mut ChunkedReader::new(chunks)).await
    }

    #[tokio::test]
    async fn parses_valid_tcp4_preface() {
        let parsed = parse(&[b"PROXY TCP4 198.51.100.12 127.0.0.1 4321 443\r\n"])
            .await
            .expect("TCP4 preface parses");
        assert_eq!(parsed.source.to_string(), "198.51.100.12:4321");
        assert!(parsed.trailing.is_empty());
    }

    #[tokio::test]
    async fn parses_valid_tcp6_preface() {
        let parsed = parse(&[b"PROXY TCP6 2001:db8::12 ::1 4321 443\r\n"])
            .await
            .expect("TCP6 preface parses");
        assert_eq!(parsed.source.to_string(), "[2001:db8::12]:4321");
    }

    #[tokio::test]
    async fn parses_fragmented_preface() {
        let parsed = parse(&[b"PRO", b"XY TCP4 198.51.100.12 127.0.0.1 4321 443\r", b"\n"])
            .await
            .expect("fragmented preface parses");
        assert_eq!(parsed.source.to_string(), "198.51.100.12:4321");
    }

    #[tokio::test]
    async fn preserves_bytes_coalesced_after_the_preface() {
        let parsed = parse(&[b"PROXY TCP4 198.51.100.12 127.0.0.1 4321 443\r\n\x16\x03\x03"])
            .await
            .expect("coalesced preface parses");
        assert_eq!(parsed.trailing, b"\x16\x03\x03");
    }

    #[tokio::test]
    async fn empty_input_is_rejected_as_missing() {
        assert!(matches!(parse(&[]).await, Err(ProxyPrefaceError::Missing)));
    }

    #[tokio::test]
    async fn malformed_signature_is_rejected() {
        assert!(matches!(
            parse(&[b"BROXY TCP4 198.51.100.12 127.0.0.1 4321 443\r\n"]).await,
            Err(ProxyPrefaceError::InvalidSignature)
        ));
    }

    #[tokio::test]
    async fn malformed_address_is_rejected() {
        assert!(matches!(
            parse(&[b"PROXY TCP4 not-an-address 127.0.0.1 4321 443\r\n"]).await,
            Err(ProxyPrefaceError::InvalidSourceAddress)
        ));
    }

    #[tokio::test]
    async fn malformed_port_is_rejected() {
        assert!(matches!(
            parse(&[b"PROXY TCP4 198.51.100.12 127.0.0.1 nope 443\r\n"]).await,
            Err(ProxyPrefaceError::InvalidSourcePort)
        ));
        assert!(matches!(
            parse(&[b"PROXY TCP4 198.51.100.12 127.0.0.1 65536 443\r\n"]).await,
            Err(ProxyPrefaceError::InvalidSourcePort)
        ));
    }

    #[tokio::test]
    async fn overlong_preface_is_rejected() {
        let bytes = vec![b'x'; 108];
        assert!(matches!(
            parse(&[&bytes]).await,
            Err(ProxyPrefaceError::Overlong)
        ));
    }

    #[tokio::test]
    async fn unknown_protocol_is_rejected() {
        assert!(matches!(
            parse(&[b"PROXY UNKNOWN\r\n"]).await,
            Err(ProxyPrefaceError::UnsupportedProtocol)
        ));
    }

    #[tokio::test]
    async fn partial_preface_close_is_rejected() {
        assert!(matches!(
            parse(&[b"PROXY TCP4 198.51.100.12"]).await,
            Err(ProxyPrefaceError::PrematureClose)
        ));
    }

    #[test]
    fn preface_errors_never_render_a_source_address() {
        for error in [
            ProxyPrefaceError::Missing,
            ProxyPrefaceError::PrematureClose,
            ProxyPrefaceError::Overlong,
            ProxyPrefaceError::InvalidSignature,
            ProxyPrefaceError::UnsupportedProtocol,
            ProxyPrefaceError::InvalidFieldCount,
            ProxyPrefaceError::InvalidSourceAddress,
            ProxyPrefaceError::InvalidDestinationAddress,
            ProxyPrefaceError::InvalidSourcePort,
            ProxyPrefaceError::InvalidDestinationPort,
            ProxyPrefaceError::Io,
        ] {
            let rendered = error.to_string();
            assert!(!rendered.contains("198.51.100.12"));
            assert!(!rendered.contains("2001:db8::12"));
        }
    }
}
