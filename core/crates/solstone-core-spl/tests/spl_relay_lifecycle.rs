// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real-boundary SPL relay listen/enroll tests that bind loopback TCP.

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use solstone_core_spl::{
    CallosumEmit, EnrollError, ListenEvent, LoopbackConnect, LoopbackDialer, PostureGate,
    PostureInput, RelayClient, RelayClientConfig, RelayDecision, RelayWebSocket, ServiceToken,
    TokenInput, WsByteSink, WsByteSource, enroll_home, relay_tunnel_url,
};
use tokio::{net::TcpListener as TokioTcpListener, sync::oneshot, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};

fn token() -> Result<ServiceToken, String> {
    let mut gate = PostureGate::new();
    gate.update_posture(PostureInput::Value("spl".to_owned()));
    gate.update_token(TokenInput::Value("service-token".to_owned()));
    match gate.decision() {
        RelayDecision::Allowed(permit) => Ok(permit.token().clone()),
        RelayDecision::Blocked(_) => Err("test token was unexpectedly blocked".to_owned()),
    }
}

fn relay_response(status: u16, body: &str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("relay listener binds");
    let address = listener.local_addr().expect("relay address reads");
    let body = body.to_owned();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("relay accepts request");
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).expect("relay reads request");
        let reason = if status == 200 {
            "OK"
        } else {
            "Service Unavailable"
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("relay response writes");
    });
    (format!("http://{address}"), worker)
}

struct NullEmit;

impl CallosumEmit for NullEmit {
    fn emit(&self, _event: &'static str, _payload: serde_json::Value) {}
}

struct ClosedDialer;

impl LoopbackDialer for ClosedDialer {
    fn connect(&self) -> LoopbackConnect {
        Box::pin(async {
            Err(std::io::Error::other(
                "loopback dialer is unused by this test",
            ))
        })
    }
}

#[test]
fn enroll_home_accepts_a_legacy_account_token() {
    let (relay, worker) = relay_response(200, r#"{"account_token":"legacy-token"}"#);
    let result = enroll_home(&relay, "instance", "ca", "home");
    worker.join().expect("relay worker joins");
    assert_eq!(result.unwrap(), "legacy-token");
}

#[test]
fn enroll_home_rejects_non_json_http_error_bodies() {
    let (relay, worker) = relay_response(503, "temporarily unavailable");
    let result = enroll_home(&relay, "instance", "ca", "home");
    worker.join().expect("relay worker joins");
    assert!(matches!(
        result,
        Err(EnrollError::Rejected {
            status: 503,
            reason: None,
        })
    ));
}

#[tokio::test]
async fn split_adapter_preserves_binary_and_text_source_bytes() -> Result<(), String> {
    let listener = TokioTcpListener::bind("127.0.0.1:0")
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
    let listener = TokioTcpListener::bind("127.0.0.1:0")
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

#[tokio::test]
async fn concrete_service_client_stop_then_aborts_the_live_listen_socket() -> Result<(), String> {
    let relay = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "could not bind fake relay".to_owned())?;
    let address = relay
        .local_addr()
        .map_err(|_| "could not read fake relay address".to_owned())?;
    let (opened_send, opened_receive) = oneshot::channel();
    let (closed_send, closed_receive) = oneshot::channel();
    let relay_task = tokio::spawn(async move {
        let (stream, _) = relay
            .accept()
            .await
            .map_err(|_| "fake relay did not accept listen socket".to_owned())?;
        let mut socket = accept_async(stream)
            .await
            .map_err(|_| "fake relay WebSocket upgrade failed".to_owned())?;
        let _ = opened_send.send(());
        let closed = timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .map_err(|_| "listen socket stayed open after service cancellation".to_owned())?;
        let observed_close = matches!(closed, None | Some(Err(_)));
        let _ = closed_send.send(observed_close);
        Ok::<(), String>(())
    });

    let client = RelayClient::new(
        RelayClientConfig {
            instance_id: "persisted-instance".to_owned(),
            relay_endpoint: format!("http://{address}"),
            service_token: ServiceToken::new("test-service-token".to_owned()),
            dispatch_read_deadline: std::time::Duration::from_secs(10),
            ping_interval: std::time::Duration::from_secs(3600),
            ping_ack_timeout: std::time::Duration::from_secs(2),
            ack_stability_window: std::time::Duration::from_secs(3600),
            global_admission_ceiling: 1,
        },
        Arc::new(NullEmit),
        Arc::new(ClosedDialer),
    );
    let running_client = client.clone();
    let run_task = tokio::spawn(async move { running_client.run().await });
    timeout(std::time::Duration::from_secs(2), opened_receive)
        .await
        .map_err(|_| "relay listen socket was never opened".to_owned())?
        .map_err(|_| "fake relay did not report opened socket".to_owned())?;

    client.stop().await;
    run_task.abort();
    let _ = run_task.await;
    let closed = timeout(std::time::Duration::from_secs(2), closed_receive)
        .await
        .map_err(|_| "fake relay did not observe socket closure".to_owned())?
        .map_err(|_| "fake relay closure report dropped".to_owned())?;
    if !closed {
        return Err("relay listen socket ended without closing".to_owned());
    }
    relay_task
        .await
        .map_err(|_| "fake relay task failed".to_owned())??;
    Ok(())
}
