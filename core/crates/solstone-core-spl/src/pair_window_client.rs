// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stateless home-side clients for v06 relay pairing windows.
//!
//! These endpoints deliberately do not share the ordinary relay listener's
//! `instance` and `token` query parameters. Pair-window credentials belong in
//! upgrade headers so they do not appear in URL logs.

use std::{
    fmt, io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_util::{Sink, Stream, StreamExt};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, Uri, header::AUTHORIZATION, request::Request},
        protocol::WebSocketConfig,
    },
};

use crate::{ListenControl, ServiceToken, parse_listen_control, pipe_loopback};

/// Bound for a pairing relay DNS, connect, and WebSocket-upgrade attempt.
pub const PAIR_WINDOW_DIAL_TIMEOUT: Duration = Duration::from_secs(10);

const PAIR_DIAL_PATH: &str = "/session/pair-dial";
const PAIR_WINDOW_PATH: &str = "/session/pair-window";
const TUNNEL_WRITE_MAX: usize = 64 * 1024;

type RelayStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A redacted eight-byte pairing secret.
///
/// This intentionally implements no [`std::fmt::Display`].
pub struct PairWindowSecret([u8; 8]);

impl From<[u8; 8]> for PairWindowSecret {
    fn from(value: [u8; 8]) -> Self {
        Self(value)
    }
}

impl PairWindowSecret {
    /// Derive the redacted relay rendezvous key for this secret.
    pub fn relay_key(&self) -> RelayPairKey {
        RelayPairKey(hex_lower(&spl_core::relay_window::derive_rk(&self.0)))
    }
}

impl fmt::Debug for PairWindowSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairWindowSecret([REDACTED; 8])")
    }
}

impl Drop for PairWindowSecret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// A redacted lowercase hexadecimal relay rendezvous key.
///
/// This intentionally implements no [`std::fmt::Display`].
#[derive(Clone, Eq, PartialEq)]
pub struct RelayPairKey(String);

impl RelayPairKey {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RelayPairKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayPairKey([REDACTED; 32])")
    }
}

/// Class-only failures from the pairing relay client.
///
/// No variant holds a request, URL, credential, or upstream error because all
/// can contain bearer material.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PairWindowClientError {
    #[error("pairing relay origin is invalid")]
    RelayOrigin,
    #[error("pairing relay tunnel identifier is invalid")]
    TunnelId,
    #[error("pairing relay request is invalid")]
    Request,
    #[error("pairing relay connection timed out")]
    TimedOut,
    #[error("pairing relay connection failed")]
    Connection,
    #[error("pairing relay rejected request")]
    Rejected(u16),
    #[error("pairing relay offer was invalid")]
    Offer,
    #[error("pairing relay connection closed")]
    Closed,
    #[error("pairing relay tunnel protocol failed")]
    TunnelProtocol,
    #[error("pairing relay loopback bridge failed")]
    Bridge,
}

/// One relay offer received by a registered pairing window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairWindowOffer {
    /// The opaque identifier to use for the home-side tunnel attachment.
    pub tunnel_id: String,
}

/// A successfully registered pairing-window WebSocket.
pub struct PairWindowRegistration {
    socket: RelayStream,
}

impl PairWindowRegistration {
    /// Wait for the relay's next incoming-tunnel offer on the registration socket.
    pub async fn next_offer(&mut self) -> Result<PairWindowOffer, PairWindowClientError> {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Text(message))) => {
                    if let Some(offer) =
                        offer_from_control(parse_listen_control(message.as_bytes()))?
                    {
                        return Ok(offer);
                    }
                }
                Some(Ok(Message::Binary(message))) => {
                    if let Some(offer) = offer_from_control(parse_listen_control(message))? {
                        return Ok(offer);
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                    return Err(PairWindowClientError::Closed);
                }
            }
        }
    }

    /// Explicitly retire the remote pairing window by closing its registration socket.
    pub async fn close(mut self) -> Result<(), PairWindowClientError> {
        self.socket
            .close(None)
            .await
            .map_err(|_| PairWindowClientError::Connection)
    }
}

/// An attached relay tunnel exposed as a WebSocket-backed byte stream.
pub struct PairWindowTunnel {
    socket: RelayStream,
    read_tail: Vec<u8>,
    read_pos: usize,
}

impl PairWindowTunnel {
    fn new(socket: RelayStream) -> Self {
        Self {
            socket,
            read_tail: Vec::new(),
            read_pos: 0,
        }
    }

    fn copy_tail(&mut self, buffer: &mut ReadBuf<'_>) -> bool {
        if self.read_pos >= self.read_tail.len() {
            self.read_tail.clear();
            self.read_pos = 0;
            return false;
        }

        let count = buffer
            .remaining()
            .min(self.read_tail.len().saturating_sub(self.read_pos));
        if count == 0 {
            return true;
        }
        buffer.put_slice(&self.read_tail[self.read_pos..self.read_pos + count]);
        self.read_pos += count;
        if self.read_pos == self.read_tail.len() {
            self.read_tail.clear();
            self.read_pos = 0;
        }
        true
    }
}

impl AsyncRead for PairWindowTunnel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if self.copy_tail(buffer) {
            return Poll::Ready(Ok(()));
        }

        loop {
            match Stream::poll_next(Pin::new(&mut self.socket), context) {
                Poll::Ready(Some(Ok(Message::Binary(bytes)))) => {
                    if bytes.is_empty() {
                        continue;
                    }
                    self.read_tail = bytes.to_vec();
                    self.read_pos = 0;
                    let _ = self.copy_tail(buffer);
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
                Poll::Ready(Some(Ok(Message::Close(_))) | None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Ok(Message::Text(_) | Message::Frame(_)))) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected pairing relay websocket message",
                    )));
                }
                Poll::Ready(Some(Err(_))) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "pairing relay websocket read failed",
                    )));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncWrite for PairWindowTunnel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let mut socket = Pin::new(&mut self.socket);
        match Sink::poll_ready(socket.as_mut(), context) {
            Poll::Ready(Ok(())) => {
                let count = buffer.len().min(TUNNEL_WRITE_MAX);
                let message = Message::Binary(buffer[..count].to_vec().into());
                Sink::start_send(socket, message).map_or_else(
                    |_| {
                        Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "pairing relay websocket write failed",
                        )))
                    },
                    |_| Poll::Ready(Ok(count)),
                )
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pairing relay websocket not ready",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Sink::poll_flush(Pin::new(&mut self.socket), context).map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pairing relay websocket flush failed",
            )
        })
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Sink::poll_close(Pin::new(&mut self.socket), context).map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pairing relay websocket close failed",
            )
        })
    }
}

/// Build the header-only WebSocket URL for a home pairing-window registration.
///
/// # Errors
///
/// Returns [`PairWindowClientError::RelayOrigin`] when the relay origin is not
/// a bare HTTP(S) origin.
pub fn pair_window_registration_url(relay_origin: &str) -> Result<String, PairWindowClientError> {
    pair_window_url(relay_origin, PAIR_WINDOW_PATH)
}

/// Build the header-only WebSocket URL for a home pairing tunnel attachment.
///
/// # Errors
///
/// Returns [`PairWindowClientError::TunnelId`] for an empty tunnel identifier,
/// or [`PairWindowClientError::RelayOrigin`] for an invalid relay origin.
pub fn pair_window_tunnel_url(
    relay_origin: &str,
    tunnel_id: &str,
) -> Result<String, PairWindowClientError> {
    if tunnel_id.is_empty() {
        return Err(PairWindowClientError::TunnelId);
    }
    pair_window_url(
        relay_origin,
        &format!("/tunnel/{}", encode_path_component(tunnel_id)),
    )
}

/// Open a home pairing-window registration at the relay.
///
/// The request carries only the service bearer credential and relay key as
/// application credentials; neither appears in the request URL.
pub async fn register_pair_window(
    relay_origin: &str,
    service_token: &ServiceToken,
    relay_key: &RelayPairKey,
) -> Result<PairWindowRegistration, PairWindowClientError> {
    let request = registration_request(relay_origin, service_token, relay_key)?;
    let socket = connect(request).await?;
    Ok(PairWindowRegistration { socket })
}

/// Attach the home side of a pairing relay tunnel.
///
/// The attachment uses the service bearer credential only. It deliberately
/// omits both `Sec-Pair-Key` and every query parameter.
pub async fn attach_pair_window_tunnel(
    relay_origin: &str,
    tunnel_id: &str,
    service_token: &ServiceToken,
) -> Result<PairWindowTunnel, PairWindowClientError> {
    let request = tunnel_attach_request(relay_origin, tunnel_id, service_token)?;
    connect(request).await.map(PairWindowTunnel::new)
}

/// Pipe an attached pairing tunnel into Convey's existing anonymous Door.
///
/// # Errors
///
/// Returns a class-only bridge failure for either the loopback dial or the
/// subsequent byte copy.
pub async fn bridge_pair_window_tunnel(
    tunnel: PairWindowTunnel,
) -> Result<(u64, u64), PairWindowClientError> {
    let loopback = TcpStream::connect(("127.0.0.1", 7657))
        .await
        .map_err(|_| PairWindowClientError::Bridge)?;
    pipe_loopback(tunnel, loopback, &[])
        .await
        .map_err(|_| PairWindowClientError::Bridge)
}

fn registration_request(
    relay_origin: &str,
    service_token: &ServiceToken,
    relay_key: &RelayPairKey,
) -> Result<Request<()>, PairWindowClientError> {
    let mut request =
        authenticated_request(&pair_window_registration_url(relay_origin)?, service_token)?;
    let relay_key =
        HeaderValue::from_str(relay_key.as_str()).map_err(|_| PairWindowClientError::Request)?;
    request.headers_mut().insert("sec-pair-key", relay_key);
    Ok(request)
}

fn tunnel_attach_request(
    relay_origin: &str,
    tunnel_id: &str,
    service_token: &ServiceToken,
) -> Result<Request<()>, PairWindowClientError> {
    authenticated_request(
        &pair_window_tunnel_url(relay_origin, tunnel_id)?,
        service_token,
    )
}

fn authenticated_request(
    url: &str,
    service_token: &ServiceToken,
) -> Result<Request<()>, PairWindowClientError> {
    let mut request = url
        .into_client_request()
        .map_err(|_| PairWindowClientError::Request)?;
    let authorization = HeaderValue::from_str(&format!("Bearer {}", service_token.as_str()))
        .map_err(|_| PairWindowClientError::Request)?;
    request.headers_mut().insert(AUTHORIZATION, authorization);
    Ok(request)
}

async fn connect(request: Request<()>) -> Result<RelayStream, PairWindowClientError> {
    let dial = connect_async_with_config(request, Some(unbounded_config()), false);
    match timeout(PAIR_WINDOW_DIAL_TIMEOUT, dial).await {
        Err(_) => Err(PairWindowClientError::TimedOut),
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => {
            Err(PairWindowClientError::Rejected(response.status().as_u16()))
        }
        Ok(Err(_)) => Err(PairWindowClientError::Connection),
    }
}

fn pair_window_url(relay_origin: &str, path: &str) -> Result<String, PairWindowClientError> {
    let pair_dial_url = spl_core::relay::pair_dial_url(relay_origin)
        .map_err(|_| PairWindowClientError::RelayOrigin)?;
    let uri: Uri = pair_dial_url
        .parse()
        .map_err(|_| PairWindowClientError::RelayOrigin)?;
    if uri.path() != PAIR_DIAL_PATH || uri.query().is_some() {
        return Err(PairWindowClientError::RelayOrigin);
    }
    let scheme = uri.scheme_str().ok_or(PairWindowClientError::RelayOrigin)?;
    let authority = uri.authority().ok_or(PairWindowClientError::RelayOrigin)?;
    Ok(format!("{scheme}://{authority}{path}"))
}

fn offer_from_control(
    control: ListenControl,
) -> Result<Option<PairWindowOffer>, PairWindowClientError> {
    match control {
        ListenControl::Incoming { tunnel_id } => Ok(Some(PairWindowOffer { tunnel_id })),
        ListenControl::Ignore => Ok(None),
        ListenControl::Invalid => Err(PairWindowClientError::Offer),
    }
}

fn encode_path_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn unbounded_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(None)
        .max_frame_size(None)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

    use super::{
        PairWindowClientError, PairWindowSecret, RelayPairKey, pair_window_registration_url,
        pair_window_tunnel_url, registration_request, tunnel_attach_request,
    };
    use crate::ServiceToken;

    fn secret_and_key() -> (PairWindowSecret, RelayPairKey) {
        let secret = PairWindowSecret::from([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        let key = secret.relay_key();
        (secret, key)
    }

    fn token() -> ServiceToken {
        ServiceToken::new("service-token".to_owned())
    }

    #[test]
    fn pair_window_urls_are_header_only_and_escape_tunnel_ids() -> Result<(), String> {
        let registration = pair_window_registration_url("https://relay.test/")
            .map_err(|error| error.to_string())?;
        let tunnel = pair_window_tunnel_url("http://relay.test:8123", "offer/with?query")
            .map_err(|error| error.to_string())?;

        assert_eq!(registration, "wss://relay.test/session/pair-window");
        assert_eq!(tunnel, "ws://relay.test:8123/tunnel/offer%2Fwith%3Fquery");
        for url in [&registration, &tunnel] {
            let parsed: tokio_tungstenite::tungstenite::http::Uri =
                url.parse().map_err(|error| format!("parse URL: {error}"))?;
            assert!(parsed.query().is_none());
        }
        assert_eq!(
            pair_window_tunnel_url("https://relay.test", ""),
            Err(PairWindowClientError::TunnelId)
        );
        assert_eq!(
            pair_window_registration_url("wss://relay.test"),
            Err(PairWindowClientError::RelayOrigin)
        );
        Ok(())
    }

    #[test]
    fn pairing_requests_have_only_their_required_application_headers() -> Result<(), String> {
        let (_secret, relay_key) = secret_and_key();
        let token = token();
        let registration = registration_request("https://relay.test", &token, &relay_key)
            .map_err(|error| error.to_string())?;
        let attach = tunnel_attach_request("https://relay.test", "offer-1", &token)
            .map_err(|error| error.to_string())?;

        assert_eq!(registration.uri().query(), None);
        assert_eq!(attach.uri().query(), None);
        assert_eq!(
            registration
                .headers()
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer service-token")
        );
        assert_eq!(
            registration
                .headers()
                .get("sec-pair-key")
                .and_then(|value| value.to_str().ok()),
            Some("e34481a4cde647ba9c9fb29a59e18271")
        );
        assert_eq!(
            attach
                .headers()
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer service-token")
        );
        assert!(attach.headers().get("sec-pair-key").is_none());

        let registration_header_names = header_names(&registration);
        let attach_header_names = header_names(&attach);
        assert_eq!(
            registration_header_names,
            BTreeSet::from([
                "authorization",
                "connection",
                "host",
                "sec-pair-key",
                "sec-websocket-key",
                "sec-websocket-version",
                "upgrade",
            ])
        );
        assert_eq!(
            attach_header_names,
            BTreeSet::from([
                "authorization",
                "connection",
                "host",
                "sec-websocket-key",
                "sec-websocket-version",
                "upgrade",
            ])
        );
        Ok(())
    }

    #[test]
    fn secret_types_and_errors_are_redacted() {
        let fake_secret = "0123456789abcdef";
        let fake_relay_key = "e34481a4cde647ba9c9fb29a59e18271";
        let fake_token = "service-token-for-redaction-test";
        let secret = PairWindowSecret::from([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        let relay_key = secret.relay_key();
        let token = ServiceToken::new(fake_token.to_owned());
        let error = PairWindowClientError::Rejected(401);

        for rendered in [
            format!("{secret:?}"),
            format!("{relay_key:?}"),
            format!("{error}"),
            format!("{error:?}"),
        ] {
            assert!(!rendered.contains(fake_secret));
            assert!(!rendered.contains(fake_relay_key));
            assert!(!rendered.contains(token.as_str()));
        }
    }

    fn header_names(request: &tokio_tungstenite::tungstenite::http::Request<()>) -> BTreeSet<&str> {
        request.headers().keys().map(|name| name.as_str()).collect()
    }
}
