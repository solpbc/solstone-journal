// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real-boundary SPL relay listen/enroll tests that bind loopback TCP.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use solstone_core_spl::{
    CallosumEmit, EnrollError, LoopbackConnect, LoopbackDialer, PostureGate, PostureInput,
    RelayClient, RelayClientConfig, RelayDecision, RelayWebSocket, ServiceToken, TokenInput,
    WsByteSink, WsByteSource, enroll_home, relay_tunnel_url, stop_relay_run,
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener as TokioTcpListener, TcpStream},
    sync::{Notify, oneshot},
    time::timeout,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

const TEST_TIMEOUT: Duration = Duration::from_secs(3);

fn token() -> Result<ServiceToken, String> {
    let mut gate = PostureGate::new();
    gate.update_posture(PostureInput::Value("spl".to_owned()));
    gate.update_token(TokenInput::Value("service-token".to_owned()));
    match gate.decision() {
        RelayDecision::Allowed(permit) => Ok(permit.token().clone()),
        RelayDecision::Blocked(_) => Err("test token was unexpectedly blocked".to_owned()),
    }
}

async fn relay_response(
    status: u16,
    body: &str,
) -> Result<(String, tokio::task::JoinHandle<Result<(), String>>), String> {
    let listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "relay listener binds".to_owned())?;
    let address = listener
        .local_addr()
        .map_err(|_| "relay address reads".to_owned())?;
    let body = body.to_owned();
    let worker = tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|_| "relay accepts request".to_owned())?;
        let mut request = [0_u8; 1024];
        let _ = stream
            .read(&mut request)
            .await
            .map_err(|_| "relay reads request".to_owned())?;
        let reason = if status == 200 {
            "OK"
        } else {
            "Service Unavailable"
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
            .await
            .map_err(|_| "relay response writes".to_owned())
    });
    Ok((format!("http://{address}"), worker))
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

struct ConnectedEmit(Mutex<Option<oneshot::Sender<()>>>);

impl CallosumEmit for ConnectedEmit {
    fn emit(&self, event: &'static str, _payload: serde_json::Value) {
        if event != "connected" {
            return;
        }
        let sender = match self.0.lock() {
            Ok(mut sender) => sender.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(sender) = sender {
            let _ = sender.send(());
        }
    }
}

#[tokio::test]
async fn enroll_home_accepts_a_legacy_account_token() -> Result<(), String> {
    timeout(TEST_TIMEOUT, async {
        let (relay, worker) = relay_response(200, r#"{"account_token":"legacy-token"}"#).await?;
        let result =
            tokio::task::spawn_blocking(move || enroll_home(&relay, "instance", "ca", "home"))
                .await
                .map_err(|_| "enrollment task failed".to_owned())?;
        worker
            .await
            .map_err(|_| "relay worker failed".to_owned())??;
        if result.map_err(|error| error.to_string())? != "legacy-token" {
            return Err("legacy token mapping differed".to_owned());
        }
        Ok(())
    })
    .await
    .map_err(|_| "legacy enrollment fixture timed out".to_owned())?
}

#[tokio::test]
async fn enroll_home_rejects_non_json_http_error_bodies() -> Result<(), String> {
    timeout(TEST_TIMEOUT, async {
        let (relay, worker) = relay_response(503, "temporarily unavailable").await?;
        let result =
            tokio::task::spawn_blocking(move || enroll_home(&relay, "instance", "ca", "home"))
                .await
                .map_err(|_| "enrollment task failed".to_owned())?;
        worker
            .await
            .map_err(|_| "relay worker failed".to_owned())??;
        if !matches!(
            result,
            Err(EnrollError::Rejected {
                status: 503,
                reason: None
            })
        ) {
            return Err("non-JSON rejection mapping differed".to_owned());
        }
        Ok(())
    })
    .await
    .map_err(|_| "rejected enrollment fixture timed out".to_owned())?
}

#[tokio::test]
async fn split_adapter_preserves_binary_and_text_source_bytes() -> Result<(), String> {
    timeout(TEST_TIMEOUT, async {
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
    })
    .await
    .map_err(|_| "split-adapter fixture timed out".to_owned())?
}

#[tokio::test]
async fn listen_events_surface_pongs_and_flush_automatic_ping_replies() -> Result<(), String> {
    timeout(TEST_TIMEOUT, async {
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
            let initial_ping = websocket
                .next()
                .await
                .ok_or_else(|| "client closed before initial Ping".to_owned())?
                .map_err(|_| "initial Ping read failed".to_owned())?;
            let Message::Ping(nonce) = initial_ping else {
                return Err("first listener frame was not Ping".to_owned());
            };
            websocket
                .send(Message::Pong(nonce))
                .await
                .map_err(|_| "listener Pong send failed".to_owned())?;
            websocket
                .send(Message::Ping(Bytes::from_static(b"peer-ping")))
                .await
                .map_err(|_| "server Ping send failed".to_owned())?;
            let reply = websocket
                .next()
                .await
                .ok_or_else(|| "client closed before automatic Pong".to_owned())?
                .map_err(|_| "automatic Pong read failed".to_owned())?;
            if reply != Message::Pong(Bytes::from_static(b"peer-ping")) {
                return Err("client automatic Pong payload differed".to_owned());
            }
            Ok::<(), String>(())
        });

        let (connected_send, connected_receive) = oneshot::channel();
        let mut client = RelayClient::new(
            RelayClientConfig {
                instance_id: "home-a".to_owned(),
                relay_endpoint: format!("http://{address}"),
                service_token: token()?,
                dispatch_read_deadline: Duration::from_secs(1),
                ping_interval: Duration::from_secs(3600),
                ping_ack_timeout: Duration::from_secs(1),
                ack_stability_window: Duration::from_secs(3600),
                global_admission_ceiling: 1,
            },
            Arc::new(ConnectedEmit(Mutex::new(Some(connected_send)))),
            Arc::new(ClosedDialer),
        );
        let running_client = client.clone();
        let run_task = tokio::spawn(async move { running_client.run().await });
        connected_receive
            .await
            .map_err(|_| "listener Pong was not surfaced as connected".to_owned())?;
        server
            .await
            .map_err(|_| "server task failed".to_owned())??;
        stop_relay_run(&mut client, run_task)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    })
    .await
    .map_err(|_| "listen-event fixture timed out".to_owned())?
}

#[derive(Default)]
struct RecordingEmit {
    events: Mutex<Vec<(String, serde_json::Value)>>,
    changed: Notify,
}

impl CallosumEmit for RecordingEmit {
    fn emit(&self, event: &'static str, payload: serde_json::Value) {
        match self.events.lock() {
            Ok(mut events) => events.push((event.to_owned(), payload)),
            Err(poisoned) => poisoned.into_inner().push((event.to_owned(), payload)),
        }
        self.changed.notify_waiters();
    }
}

impl RecordingEmit {
    fn snapshot(&self) -> Vec<(String, serde_json::Value)> {
        match self.events.lock() {
            Ok(events) => events.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    async fn wait_for_count(&self, event: &str, count: usize) {
        loop {
            if self
                .snapshot()
                .iter()
                .filter(|(name, _)| name == event)
                .count()
                >= count
            {
                return;
            }
            self.changed.notified().await;
        }
    }
}

struct TcpDialer(std::net::SocketAddr);

impl LoopbackDialer for TcpDialer {
    fn connect(&self) -> LoopbackConnect {
        let address = self.0;
        Box::pin(async move {
            let stream = TcpStream::connect(address).await?;
            Ok(Box::new(stream) as Box<dyn solstone_core_spl::LoopbackStream>)
        })
    }
}

#[tokio::test]
async fn incoming_offer_dials_loopback_replays_split_tls_prefix_and_emits_exact_tail()
-> Result<(), String> {
    timeout(TEST_TIMEOUT, async {
        let relay = TokioTcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "relay bind failed".to_owned())?;
        let relay_address = relay
            .local_addr()
            .map_err(|_| "relay address failed".to_owned())?;
        let loopback = TokioTcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "loopback bind failed".to_owned())?;
        let loopback_address = loopback
            .local_addr()
            .map_err(|_| "loopback address failed".to_owned())?;
        let (replayed_send, replayed_receive) = oneshot::channel();
        let relay_task = tokio::spawn(async move {
            let (listen_stream, _) = relay
                .accept()
                .await
                .map_err(|_| "listen accept failed".to_owned())?;
            let mut listen = accept_async(listen_stream)
                .await
                .map_err(|_| "listen upgrade failed".to_owned())?;
            let ping = listen
                .next()
                .await
                .ok_or_else(|| "listen ended before ping".to_owned())?
                .map_err(|_| "ping read failed".to_owned())?;
            let Message::Ping(nonce) = ping else {
                return Err("first listen frame was not Ping".to_owned());
            };
            listen
                .send(Message::Pong(nonce))
                .await
                .map_err(|_| "pong send failed".to_owned())?;
            listen
                .send(Message::Text(
                    r#"{"type":"incoming","tunnel_id":"tls-7"}"#.into(),
                ))
                .await
                .map_err(|_| "offer send failed".to_owned())?;

            let (tunnel_stream, _) = relay
                .accept()
                .await
                .map_err(|_| "tunnel accept failed".to_owned())?;
            let mut tunnel = accept_async(tunnel_stream)
                .await
                .map_err(|_| "tunnel upgrade failed".to_owned())?;
            tunnel
                .send(Message::Binary(Bytes::from_static(&[0x16, 0x03])))
                .await
                .map_err(|_| "first prefix send failed".to_owned())?;
            tunnel
                .send(Message::Binary(Bytes::from_static(&[0x01, 0x00])))
                .await
                .map_err(|_| "second prefix send failed".to_owned())?;
            let (mut local, _) = loopback
                .accept()
                .await
                .map_err(|_| "loopback dial was not observed".to_owned())?;
            let mut replayed = [0_u8; 4];
            local
                .read_exact(&mut replayed)
                .await
                .map_err(|_| "prefix replay read failed".to_owned())?;

            listen
                .send(Message::Text(
                    r#"{"type":"incoming","tunnel_id":"tls-8"}"#.into(),
                ))
                .await
                .map_err(|_| "second offer send failed".to_owned())?;
            let (second_tunnel_stream, _) = relay
                .accept()
                .await
                .map_err(|_| "second tunnel accept failed".to_owned())?;
            let mut second_tunnel = accept_async(second_tunnel_stream)
                .await
                .map_err(|_| "second tunnel upgrade failed".to_owned())?;
            second_tunnel
                .send(Message::Binary(Bytes::from_static(&[0x16, 0x03])))
                .await
                .map_err(|_| "second first-prefix send failed".to_owned())?;
            second_tunnel
                .send(Message::Binary(Bytes::from_static(&[0x01, 0x00])))
                .await
                .map_err(|_| "second second-prefix send failed".to_owned())?;
            let (mut second_local, _) = loopback.accept().await.map_err(|_| {
                "released admission did not permit a second loopback dial".to_owned()
            })?;
            let mut second_replayed = [0_u8; 4];
            second_local
                .read_exact(&mut second_replayed)
                .await
                .map_err(|_| "second prefix replay read failed".to_owned())?;
            let _ = replayed_send.send([replayed, second_replayed]);
            drop(local);
            drop(second_local);
            tunnel
                .close(None)
                .await
                .map_err(|_| "tunnel close failed".to_owned())?;
            second_tunnel
                .close(None)
                .await
                .map_err(|_| "second tunnel close failed".to_owned())?;
            Ok::<(), String>(())
        });

        let emit = Arc::new(RecordingEmit::default());
        let mut client = RelayClient::new(
            RelayClientConfig {
                instance_id: "persisted-instance".to_owned(),
                relay_endpoint: format!("http://{relay_address}"),
                service_token: ServiceToken::new("test-service-token".to_owned()),
                dispatch_read_deadline: Duration::from_secs(1),
                ping_interval: Duration::from_secs(3600),
                ping_ack_timeout: Duration::from_secs(1),
                ack_stability_window: Duration::from_secs(3600),
                global_admission_ceiling: 1,
            },
            emit.clone(),
            Arc::new(TcpDialer(loopback_address)),
        );
        let running_client = client.clone();
        let run_task = tokio::spawn(async move { running_client.run().await });
        if replayed_receive
            .await
            .map_err(|_| "prefix replay report dropped".to_owned())?
            != [[0x16, 0x03, 0x01, 0x00], [0x16, 0x03, 0x01, 0x00]]
        {
            return Err("split TLS prefixes were not replayed exactly once".to_owned());
        }
        emit.wait_for_count("tunnel_close", 2).await;
        stop_relay_run(&mut client, run_task)
            .await
            .map_err(|error| error.to_string())?;
        relay_task
            .await
            .map_err(|_| "relay task failed".to_owned())??;

        let events = emit.snapshot();
        let mut closed_ids = events
            .iter()
            .filter_map(|(event, payload)| {
                (event == "tunnel_close")
                    .then(|| payload.get("tunnel_id")?.as_str().map(str::to_owned))?
            })
            .collect::<Vec<_>>();
        closed_ids.sort();
        if closed_ids != ["tls-7", "tls-8"] {
            return Err(format!("tunnel-close identities differed: {closed_ids:?}"));
        }
        let tail = events
            .get(events.len().saturating_sub(4)..)
            .ok_or_else(|| "lifecycle tail was incomplete".to_owned())?;
        if tail
            .iter()
            .map(|(event, _)| event.as_str())
            .collect::<Vec<_>>()
            != ["tunnel_close", "health", "disconnect", "health"]
        {
            return Err(format!("lifecycle tail differed: {tail:?}"));
        }
        if tail[0]
            .1
            .get("tunnel_id")
            .and_then(serde_json::Value::as_str)
            .is_none()
            || tail[1]
                .1
                .get("relay_admission_saturated_count")
                .and_then(serde_json::Value::as_u64)
                != Some(0)
            || tail[3].1.get("state").and_then(serde_json::Value::as_str) != Some("reconnecting")
        {
            return Err("lifecycle tail payload differed or admission was not released".to_owned());
        }
        Ok(())
    })
    .await
    .map_err(|_| "incoming-offer fixture timed out".to_owned())?
}

#[tokio::test]
async fn concrete_service_shutdown_stops_then_cancels_the_live_listen_socket() -> Result<(), String>
{
    timeout(TEST_TIMEOUT, async {
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

        let mut client = RelayClient::new(
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

        stop_relay_run(&mut client, run_task)
            .await
            .map_err(|error| error.to_string())?;
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
    })
    .await
    .map_err(|_| "concrete service-shutdown fixture timed out".to_owned())?
}
