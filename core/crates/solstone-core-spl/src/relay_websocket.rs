// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Authenticated relay WebSocket transport split into independent byte halves.
//!
//! The caller provides a fully constructed relay URL, including its required
//! `instance` and `token` query fields.  This adapter adds the matching bearer
//! header without exposing either the request or upstream transport details to
//! callers. The stream is split exactly once so the TLS loopback pipe can own
//! independent reader and writer halves for concurrent tunnel forwarding.

use bytes::Bytes;
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, header::AUTHORIZATION},
        protocol::WebSocketConfig,
    },
};

use crate::{ServiceToken, WsByteSink, WsByteSource, WsClosed};

type RelayStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The class-only failures from creating a relay WebSocket transport.
///
/// No variant stores an upstream error or request because a relay URL carries
/// the service token in its query string.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RelayWebSocketError {
    #[error("relay websocket request is invalid")]
    Request,
    #[error("relay websocket connection failed")]
    Connection,
    /// The relay returned an HTTP status without exposing its response body.
    #[error("relay websocket was rejected")]
    Status(u16),
}

/// A connected relay WebSocket before its required one-time split.
pub struct RelayWebSocket {
    reader: RelayWebSocketReader,
    writer: RelayWebSocketWriter,
}

impl RelayWebSocket {
    /// Connect an authenticated relay WebSocket with no protocol-library size cap.
    ///
    /// The full URL must already contain the token query field required by the
    /// relay protocol.  The same token is supplied again in `Authorization`.
    ///
    /// # Errors
    ///
    /// Returns a class-only error when request construction or the connection
    /// handshake fails.
    pub async fn connect(
        full_url: &str,
        token: &ServiceToken,
    ) -> Result<Self, RelayWebSocketError> {
        let request = relay_request(full_url, token)?;
        let (stream, _) =
            match connect_async_with_config(request, Some(unbounded_config()), false).await {
                Ok(result) => result,
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                    return Err(RelayWebSocketError::Status(response.status().as_u16()));
                }
                Err(_) => return Err(RelayWebSocketError::Connection),
            };
        let (writer, reader) = stream.split();

        Ok(Self {
            reader: RelayWebSocketReader { inner: reader },
            writer: RelayWebSocketWriter { inner: writer },
        })
    }

    /// Transfer the independently usable read and write halves to their owners.
    pub fn split(self) -> (RelayWebSocketReader, RelayWebSocketWriter) {
        (self.reader, self.writer)
    }
}

/// The read half of a connected relay WebSocket.
pub struct RelayWebSocketReader {
    inner: SplitStream<RelayStream>,
}

/// A listen-channel event whose WebSocket control framing is significant.
pub enum ListenEvent {
    /// A relay control message carried as text or binary bytes.
    Message(Bytes),
    /// A raw WebSocket Pong payload.
    Pong(Bytes),
}

impl RelayWebSocketReader {
    /// Reads one relay-listen event while retaining raw Pong payloads.
    pub async fn next_listen_event(&mut self) -> Result<ListenEvent, WsClosed> {
        loop {
            match self.inner.next().await {
                Some(Ok(Message::Binary(bytes))) => return Ok(ListenEvent::Message(bytes)),
                Some(Ok(Message::Text(text))) => return Ok(ListenEvent::Message(text.into())),
                Some(Ok(Message::Pong(bytes))) => return Ok(ListenEvent::Pong(bytes)),
                Some(Ok(Message::Ping(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Close(_)) | Err(_)) | None => return Err(WsClosed),
            }
        }
    }
}

impl WsByteSource for RelayWebSocketReader {
    async fn next_message(&mut self) -> Result<Option<Bytes>, WsClosed> {
        loop {
            match self.inner.next().await {
                Some(Ok(Message::Binary(bytes))) => return Ok(Some(bytes)),
                Some(Ok(Message::Text(text))) => return Ok(Some(text.into())),
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Close(_)) | Err(_)) | None => return Err(WsClosed),
            }
        }
    }
}

/// The write half of a connected relay WebSocket.
pub struct RelayWebSocketWriter {
    inner: SplitSink<RelayStream, Message>,
}

impl RelayWebSocketWriter {
    /// Sends and flushes a WebSocket Ping with its opaque acknowledgement nonce.
    pub async fn send_ping(&mut self, payload: Bytes) -> Result<(), WsClosed> {
        self.inner
            .send(Message::Ping(payload))
            .await
            .map_err(|_| WsClosed)
    }
}

impl WsByteSink for RelayWebSocketWriter {
    async fn send(&mut self, bytes: Bytes) -> Result<(), WsClosed> {
        self.inner
            .send(Message::Binary(bytes))
            .await
            .map_err(|_| WsClosed)
    }

    async fn close(&mut self) -> Result<(), WsClosed> {
        self.inner
            .send(Message::Close(None))
            .await
            .map_err(|_| WsClosed)?;
        self.inner.close().await.map_err(|_| WsClosed)
    }
}

fn relay_request(
    full_url: &str,
    token: &ServiceToken,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, RelayWebSocketError> {
    let mut request = full_url
        .into_client_request()
        .map_err(|_| RelayWebSocketError::Request)?;
    let authorization = HeaderValue::from_str(&format!("Bearer {}", token.as_str()))
        .map_err(|_| RelayWebSocketError::Request)?;
    request.headers_mut().insert(AUTHORIZATION, authorization);
    Ok(request)
}

fn unbounded_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(None)
        .max_frame_size(None)
}

#[cfg(test)]
mod tests {
    use super::{
        ListenEvent, RelayWebSocket, RelayWebSocketError, relay_request, unbounded_config,
    };
    use crate::{
        PostureGate, PostureInput, RelayDecision, TokenInput, WsByteSink, WsByteSource,
        relay_tunnel_url,
    };
    use bytes::Bytes;
    use futures_util::{SinkExt, StreamExt};
    use tokio::{net::TcpListener, time::timeout};
    use tokio_tungstenite::{
        accept_async,
        tungstenite::{Message, http::header::AUTHORIZATION},
    };

    fn token() -> Result<crate::ServiceToken, String> {
        let mut gate = PostureGate::new();
        gate.update_posture(PostureInput::Value("spl".to_owned()));
        gate.update_token(TokenInput::Value("service-token".to_owned()));
        match gate.decision() {
            RelayDecision::Allowed(permit) => Ok(permit.token().clone()),
            RelayDecision::Blocked(_) => Err("test token was unexpectedly blocked".to_owned()),
        }
    }

    #[test]
    fn request_keeps_query_token_and_adds_matching_bearer_header() -> Result<(), String> {
        let token = token()?;
        let url = relay_tunnel_url(
            "wss://relay.test",
            "/session/listen",
            "home-a",
            token.as_str(),
        );
        let request = relay_request(&url, &token).map_err(|error| error.to_string())?;

        assert_eq!(
            request.uri().query(),
            Some("instance=home-a&token=service-token")
        );
        assert_eq!(
            request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer service-token")
        );
        assert_eq!(unbounded_config().max_message_size, None);
        assert_eq!(unbounded_config().max_frame_size, None);
        assert!(!format!("{:?}", RelayWebSocketError::Request).contains(token.as_str()));
        assert!(
            !RelayWebSocketError::Connection
                .to_string()
                .contains(token.as_str())
        );
        Ok(())
    }

    #[tokio::test]
    async fn split_adapter_preserves_binary_and_text_source_bytes() -> Result<(), String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "listener bind failed".to_owned())?;
        let address = listener
            .local_addr()
            .map_err(|_| "listener address failed".to_owned())?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener
                .accept()
                .await
                .map_err(|_| "listener accept failed".to_owned())?;
            let mut websocket = accept_async(stream)
                .await
                .map_err(|_| "server upgrade failed".to_owned())?;
            websocket
                .send(Message::Binary(Bytes::from_static(b"binary")))
                .await
                .map_err(|_| "server binary send failed".to_owned())?;
            websocket
                .send(Message::Text("text".into()))
                .await
                .map_err(|_| "server text send failed".to_owned())?;
            let response = websocket
                .next()
                .await
                .ok_or_else(|| "server response ended".to_owned())?
                .map_err(|_| "server response failed".to_owned())?;
            assert_eq!(response, Message::Binary(Bytes::from_static(b"reply")));
            websocket
                .close(None)
                .await
                .map_err(|_| "server close failed".to_owned())
        });

        let token = token()?;
        let endpoint = format!("ws://{address}");
        let url = relay_tunnel_url(&endpoint, "/session/listen", "home-a", token.as_str());
        let websocket = RelayWebSocket::connect(&url, &token)
            .await
            .map_err(|error| error.to_string())?;
        let (mut reader, mut writer) = websocket.split();

        assert_eq!(
            reader
                .next_message()
                .await
                .map_err(|_| "binary source read failed".to_owned())?,
            Some(Bytes::from_static(b"binary"))
        );
        assert_eq!(
            reader
                .next_message()
                .await
                .map_err(|_| "text source read failed".to_owned())?,
            Some(Bytes::from_static(b"text"))
        );
        writer
            .send(Bytes::from_static(b"reply"))
            .await
            .map_err(|_| "sink response failed".to_owned())?;
        server
            .await
            .map_err(|_| "server task failed".to_owned())??;
        Ok(())
    }

    #[tokio::test]
    async fn listen_events_surface_pongs_and_flush_automatic_ping_replies() -> Result<(), String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "listener bind failed".to_owned())?;
        let address = listener
            .local_addr()
            .map_err(|_| "listener address failed".to_owned())?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener
                .accept()
                .await
                .map_err(|_| "listener accept failed".to_owned())?;
            let mut websocket = accept_async(stream)
                .await
                .map_err(|_| "server upgrade failed".to_owned())?;
            websocket
                .send(Message::Ping(Bytes::from_static(b"peer-ping")))
                .await
                .map_err(|_| "server ping send failed".to_owned())?;
            let reply = timeout(std::time::Duration::from_secs(1), websocket.next())
                .await
                .map_err(|_| "client did not flush automatic pong".to_owned())?
                .ok_or_else(|| "client closed before pong".to_owned())?
                .map_err(|_| "client pong read failed".to_owned())?;
            if reply != Message::Pong(Bytes::from_static(b"peer-ping")) {
                return Err("client automatic pong payload differed".to_owned());
            }
            websocket
                .send(Message::Pong(Bytes::from_static(b"heartbeat")))
                .await
                .map_err(|_| "server pong send failed".to_owned())?;
            websocket
                .send(Message::Text("{\"type\":\"incoming\"}".into()))
                .await
                .map_err(|_| "server control send failed".to_owned())
        });

        let token = token()?;
        let endpoint = format!("ws://{address}");
        let url = relay_tunnel_url(&endpoint, "/session/listen", "home-a", token.as_str());
        let websocket = RelayWebSocket::connect(&url, &token)
            .await
            .map_err(|error| error.to_string())?;
        let (mut reader, _writer) = websocket.split();

        assert!(matches!(
            reader
                .next_listen_event()
                .await
                .map_err(|_| "listen pong read failed".to_owned())?,
            ListenEvent::Pong(payload) if payload == Bytes::from_static(b"heartbeat")
        ));
        assert!(matches!(
            reader
                .next_listen_event()
                .await
                .map_err(|_| "listen control read failed".to_owned())?,
            ListenEvent::Message(payload) if payload == Bytes::from_static(b"{\"type\":\"incoming\"}")
        ));
        server
            .await
            .map_err(|_| "server task failed".to_owned())??;
        Ok(())
    }
}
