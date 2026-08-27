// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_callosum::test_support::{
    ServerTestHooks, connection_with_initial_counters, join_terminated,
};
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumEnvelope, CallosumGapReason, CallosumReceiveEvent,
    CallosumRetrySource, CallosumSocketConnection, CallosumSocketServer, CallosumStoppedReason,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{sleep, timeout};

static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);

struct TempSocket {
    root: PathBuf,
    path: PathBuf,
}

impl TempSocket {
    fn new(_name: &str) -> Self {
        let ordinal = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        // Unix-domain socket paths have a small platform limit. Keep the
        // fixture basename compact even when TMPDIR is deliberately nested.
        let root = std::env::temp_dir().join(format!("cw-{}-{ordinal}", std::process::id()));
        fs::create_dir_all(&root).expect("create temporary Callosum root");
        let path = root.join("callosum.sock");
        Self { root, path }
    }
}

impl Drop for TempSocket {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn reader<R: AsyncRead + Unpin>(stream: R) -> tokio::io::BufReader<R> {
    tokio::io::BufReader::new(stream)
}

fn fields(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn connection(path: &PathBuf, defaults: Map<String, Value>) -> CallosumSocketConnection {
    let mut connection = CallosumSocketConnection::new(path, defaults);
    connection.start();
    connection
}

async fn wait_for_clients(server: &CallosumSocketServer, count: usize) {
    timeout(Duration::from_secs(2), async {
        while server.client_count() != count {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected client count");
}

async fn wait_for_counter(check: impl Fn() -> u64) {
    timeout(Duration::from_secs(3), async {
        while check() == 0 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected counter increment");
}

async fn next(connection: &mut CallosumSocketConnection) -> CallosumEnvelope {
    timeout(Duration::from_secs(2), connection.next_message())
        .await
        .expect("message should arrive")
        .expect("connection receiver should remain open")
}

async fn next_event(connection: &mut CallosumSocketConnection) -> CallosumReceiveEvent {
    timeout(Duration::from_secs(2), connection.next_event())
        .await
        .expect("receive event should arrive")
        .expect("connection receiver should remain open")
}

struct SuppliedRetries(UnboundedReceiver<bool>);

impl CallosumRetrySource for SuppliedRetries {
    fn next_attempt(
        &mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.0.recv().await.unwrap_or(false) })
    }
}

struct ImmediateRetry;

impl CallosumRetrySource for ImmediateRetry {
    fn next_attempt(
        &mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async {
            tokio::task::yield_now().await;
            true
        })
    }
}

async fn wait_until_stopped(client: &CallosumSocketConnection) {
    timeout(Duration::from_secs(2), async {
        while client.is_running() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("wire client should terminate after counter overflow");
}

#[tokio::test(flavor = "current_thread")]
async fn attempt_counter_overflow_stops_before_connection_without_reuse() {
    let socket = TempSocket::new("attempt-counter-overflow");
    let mut client = connection_with_initial_counters(
        &socket.path,
        Box::new(ImmediateRetry),
        0,
        0,
        u64::MAX,
        0,
        false,
    );
    client.start();
    let connecting = next_event(&mut client).await;
    assert!(
        matches!(
            connecting,
            CallosumReceiveEvent::Continuity {
                generation: 0,
                epoch: 0,
                phase: CallosumConnectionPhase::Connecting { attempt: 1 },
            }
        ),
        "{connecting:?}"
    );
    let stopped = next_event(&mut client).await;
    assert!(
        matches!(
            stopped,
            CallosumReceiveEvent::Continuity {
                generation: 0,
                epoch: 0,
                phase: CallosumConnectionPhase::Stopped {
                    reason: CallosumStoppedReason::CounterOverflow,
                },
            }
        ),
        "{stopped:?}"
    );
    wait_until_stopped(&client).await;
    timeout(Duration::from_secs(2), join_terminated(&mut client))
        .await
        .expect("stopped attempt client cleanup");
    assert!(!socket.path.exists(), "attempt overflow must not connect");
}

#[tokio::test(flavor = "current_thread")]
async fn generation_counter_overflow_stops_after_connection_without_reuse() {
    let socket = TempSocket::new("generation-counter-overflow");
    let listener = tokio::net::UnixListener::bind(&socket.path).expect("bind overflow listener");
    let mut client = connection_with_initial_counters(
        &socket.path,
        Box::new(ImmediateRetry),
        u64::MAX,
        0,
        1,
        0,
        true,
    );
    client.start();
    let connecting = next_event(&mut client).await;
    assert!(
        matches!(
            connecting,
            CallosumReceiveEvent::Continuity {
                generation: 0,
                epoch: 0,
                phase: CallosumConnectionPhase::Connecting { attempt: 1 },
            }
        ),
        "{connecting:?}"
    );
    let mut peer = timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("overflow connection deadline")
        .expect("accept overflow connection")
        .0;
    let stopped = next_event(&mut client).await;
    assert!(
        matches!(
            stopped,
            CallosumReceiveEvent::Continuity {
                generation: u64::MAX,
                epoch: 0,
                phase: CallosumConnectionPhase::Stopped {
                    reason: CallosumStoppedReason::CounterOverflow,
                },
            }
        ),
        "{stopped:?}"
    );
    wait_until_stopped(&client).await;
    timeout(Duration::from_secs(2), join_terminated(&mut client))
        .await
        .expect("stopped generation client cleanup");
    let mut after_close = [0_u8; 1];
    assert_eq!(
        timeout(Duration::from_secs(2), peer.read(&mut after_close))
            .await
            .expect("overflow connection teardown deadline")
            .expect("read overflow connection teardown"),
        0,
        "generation overflow must close the connected socket"
    );
    drop(peer);
    drop(listener);
}

struct ObservedRetries {
    permissions: UnboundedReceiver<bool>,
    polled: tokio::sync::mpsc::UnboundedSender<()>,
}

impl CallosumRetrySource for ObservedRetries {
    fn next_attempt(
        &mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        let _ = self.polled.send(());
        Box::pin(async move { self.permissions.recv().await.unwrap_or(false) })
    }
}

async fn wait_for_pending_priority(client: &CallosumSocketConnection) {
    timeout(Duration::from_secs(2), async {
        while !client.has_pending_priority() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("priority event should become pending");
}

async fn observed_connected_client(
    socket: &TempSocket,
) -> (
    tokio::net::UnixListener,
    CallosumSocketConnection,
    UnixStream,
    tokio::sync::mpsc::UnboundedSender<bool>,
    UnboundedReceiver<()>,
) {
    let listener = tokio::net::UnixListener::bind(&socket.path).unwrap();
    let (permissions, permission_rx) = tokio::sync::mpsc::unbounded_channel();
    let (polled_tx, mut polled) = tokio::sync::mpsc::unbounded_channel();
    let mut client = CallosumSocketConnection::with_retry_source(
        &socket.path,
        Map::new(),
        1,
        Box::new(ObservedRetries {
            permissions: permission_rx,
            polled: polled_tx,
        }),
    );
    client.start();
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            phase: CallosumConnectionPhase::Connecting { attempt: 1 },
            ..
        }
    ));
    polled.recv().await.expect("initial retry poll");
    permissions.send(true).unwrap();
    let peer = timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("initial connection")
        .unwrap()
        .0;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 1,
            phase: CallosumConnectionPhase::Connected,
        }
    ));
    (listener, client, peer, permissions, polled)
}

#[tokio::test(flavor = "current_thread")]
async fn unconsumed_disconnect_gap_blocks_retry_until_delivery() {
    let socket = TempSocket::new("gap-before-retry");
    let (listener, mut client, peer, permissions, mut polled) =
        observed_connected_client(&socket).await;
    drop(peer);
    wait_for_pending_priority(&client).await;
    tokio::task::yield_now().await;
    assert!(
        polled.try_recv().is_err(),
        "retry polled before gap delivery"
    );
    assert!(matches!(
        client.try_next_event(),
        Some(CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Gapped {
                reason: CallosumGapReason::Disconnected,
                dropped_count: 1,
            },
        })
    ));
    polled.recv().await.expect("retry poll after gap delivery");
    permissions.send(true).unwrap();
    let _replacement = timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("reconnection")
        .unwrap()
        .0;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            phase: CallosumConnectionPhase::Connecting { attempt: 2 },
            ..
        }
    ));
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 2,
            epoch: 3,
            phase: CallosumConnectionPhase::Connected,
        }
    ));
    client.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn stop_joins_while_disconnect_gap_is_unconsumed() {
    let socket = TempSocket::new("stop-with-pending-gap");
    let (_listener, mut client, peer, _permissions, mut polled) =
        observed_connected_client(&socket).await;
    drop(peer);
    wait_for_pending_priority(&client).await;
    tokio::task::yield_now().await;
    assert!(polled.try_recv().is_err(), "retry polled before stop");
    timeout(Duration::from_millis(100), client.stop())
        .await
        .expect("stop should join without consuming the gap");
    assert!(!client.is_running());
    assert!(
        polled.try_recv().is_err(),
        "stop created a retry opportunity"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn drop_shuts_down_while_disconnect_gap_is_unconsumed() {
    let socket = TempSocket::new("drop-with-pending-gap");
    let (_listener, client, peer, _permissions, mut polled) =
        observed_connected_client(&socket).await;
    drop(peer);
    wait_for_pending_priority(&client).await;
    drop(client);
    assert!(
        timeout(Duration::from_millis(100), polled.recv())
            .await
            .expect("drop should stop the background task promptly")
            .is_none(),
        "drop created a retry opportunity"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn priority_latch_starts_connecting_and_coalesces_unavailable() {
    let socket = TempSocket::new("priority-unavailable");
    let (supplied, retries) = tokio::sync::mpsc::unbounded_channel();
    let mut client = CallosumSocketConnection::with_retry_source(
        &socket.path,
        Map::new(),
        1,
        Box::new(SuppliedRetries(retries)),
    );
    client.start();
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 0,
            epoch: 0,
            phase: CallosumConnectionPhase::Connecting { attempt: 1 },
        }
    ));
    supplied.send(true).unwrap();
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 0,
            epoch: 0,
            phase: CallosumConnectionPhase::Unavailable {
                latest_attempt: 1,
                failures_since_success: 1
            },
        }
    ));
    supplied.send(true).unwrap();
    let second_attempt = next_event(&mut client).await;
    let unavailable = if matches!(
        second_attempt,
        CallosumReceiveEvent::Continuity {
            generation: 0,
            epoch: 0,
            phase: CallosumConnectionPhase::Connecting { attempt: 2 },
        }
    ) {
        next_event(&mut client).await
    } else {
        second_attempt
    };
    assert!(
        matches!(
            unavailable,
            CallosumReceiveEvent::Continuity {
                generation: 0,
                epoch: 0,
                phase: CallosumConnectionPhase::Unavailable {
                    latest_attempt: 2,
                    failures_since_success: 2
                },
            }
        ),
        "{unavailable:?}"
    );
    client.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn real_adapter_retry_source_never_and_late_connect() {
    let socket = TempSocket::new("supplied-retry");
    let (supplied, retries) = tokio::sync::mpsc::unbounded_channel();
    let mut client = CallosumSocketConnection::with_retry_source(
        &socket.path,
        Map::new(),
        1,
        Box::new(SuppliedRetries(retries)),
    );
    client.start();
    let _ = next_event(&mut client).await;
    supplied.send(true).unwrap();
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 0,
            epoch: 0,
            phase: CallosumConnectionPhase::Unavailable {
                latest_attempt: 1,
                failures_since_success: 1
            },
        }
    ));
    let listener = tokio::net::UnixListener::bind(&socket.path).unwrap();
    supplied.send(true).unwrap();
    let _peer = listener.accept().await.unwrap();
    let connected = next_event(&mut client).await;
    let connected = if matches!(
        connected,
        CallosumReceiveEvent::Continuity {
            generation: 0,
            epoch: 0,
            phase: CallosumConnectionPhase::Connecting { attempt: 2 },
        }
    ) {
        next_event(&mut client).await
    } else {
        connected
    };
    assert!(matches!(
        connected,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 1,
            phase: CallosumConnectionPhase::Connected,
        }
    ));
    client.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn continuity_generation_connected_precedes_envelopes_and_next_message_skips_markers() {
    let socket = TempSocket::new("continuity-generation");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut first = connection(&socket.path, Map::new());
    wait_for_clients(&server, 1).await;
    assert!(matches!(
        next_event(&mut first).await,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 1,
            phase: CallosumConnectionPhase::Connected,
        }
    ));
    assert!(first.emit("top", "one", Map::new()));
    assert!(
        matches!(next_event(&mut first).await, CallosumReceiveEvent::Envelope { generation: 1, epoch: 1, envelope } if envelope.tract == "top" && envelope.event == "one")
    );
    assert!(first.emit("top", "two", Map::new()));
    let message = next(&mut first).await;
    assert_eq!(
        (message.tract.as_str(), message.event.as_str()),
        ("top", "two")
    );
    first.stop().await;
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn generation_epoch_transition_and_overflow_are_nonwrapping() {
    let socket = TempSocket::new("continuity-reconnect");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut client = connection(&socket.path, Map::new());
    wait_for_clients(&server, 1).await;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 1,
            phase: CallosumConnectionPhase::Connected,
        }
    ));
    server.stop().await;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Gapped {
                reason: CallosumGapReason::Disconnected,
                dropped_count: 1
            },
        }
    ));

    let replacement = CallosumSocketServer::bind(&socket.path).await.unwrap();
    wait_for_clients(&replacement, 1).await;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 2,
            epoch: 3,
            phase: CallosumConnectionPhase::Connected,
        }
    ));
    assert!(client.emit("continuity", "after-reconnect", Map::new()));
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Envelope { generation: 2, epoch: 3, envelope }
            if envelope.tract == "continuity" && envelope.event == "after-reconnect"
    ));
    client.stop().await;
    replacement.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn continuity_lost_connection_emits_one_disconnected_marker() {
    let socket = TempSocket::new("continuity-single-disconnect");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut client = connection(&socket.path, Map::new());
    wait_for_clients(&server, 1).await;
    let _ = next_event(&mut client).await;
    server.stop().await;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Gapped {
                reason: CallosumGapReason::Disconnected,
                dropped_count: 1
            },
        }
    ));
    assert!(
        timeout(Duration::from_millis(150), client.next_event())
            .await
            .is_err()
    );
    client.stop().await;
}

async fn connected_raw_peer(
    socket: &TempSocket,
) -> (
    tokio::net::UnixListener,
    CallosumSocketConnection,
    UnixStream,
) {
    let listener = tokio::net::UnixListener::bind(&socket.path).unwrap();
    let mut client = connection(&socket.path, Map::new());
    let peer = accept_connected_raw_peer(&listener, &mut client).await;
    (listener, client, peer)
}

async fn accept_connected_raw_peer(
    listener: &tokio::net::UnixListener,
    client: &mut CallosumSocketConnection,
) -> UnixStream {
    let peer = timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("connection should arrive")
        .expect("listener should accept")
        .0;
    let initial = next_event(client).await;
    let connected = if matches!(
        initial,
        CallosumReceiveEvent::Continuity {
            generation: 0,
            epoch: 0,
            phase: CallosumConnectionPhase::Connecting { attempt: 1 },
        }
    ) {
        next_event(client).await
    } else {
        initial
    };
    assert!(matches!(
        connected,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 1,
            phase: CallosumConnectionPhase::Connected,
        }
    ));
    peer
}

#[tokio::test(flavor = "current_thread")]
async fn continuity_malformed_frame_marks_current_generation_and_connection_survives() {
    let socket = TempSocket::new("continuity-malformed");
    let (_listener, mut client, mut peer) = connected_raw_peer(&socket).await;
    peer.write_all(b"{malformed}\n").await.unwrap();
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Gapped {
                reason: CallosumGapReason::MalformedFrameDropped,
                dropped_count: 1
            },
        }
    ));
    peer.write_all(b"{\"tract\":\"valid\",\"event\":\"after\"}\n")
        .await
        .unwrap();
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Connected,
        }
    ));
    assert!(
        matches!(next_event(&mut client).await, CallosumReceiveEvent::Envelope { generation: 1, epoch: 2, envelope }
        if envelope.tract == "valid" && envelope.event == "after")
    );
    client.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn capacity_one_priority_latch_preserves_saturation_and_recovery_order() {
    let socket = TempSocket::new("continuity-saturation");
    let listener = tokio::net::UnixListener::bind(&socket.path).unwrap();
    let mut client = CallosumSocketConnection::with_inbound_capacity(&socket.path, Map::new(), 1);
    client.start();
    let mut peer = accept_connected_raw_peer(&listener, &mut client).await;
    peer.write_all(b"{\"tract\":\"burst\",\"event\":\"0\"}\n")
        .await
        .unwrap();
    client.wait_for_frames_processed(1).await;
    peer.write_all(b"{\"tract\":\"burst\",\"event\":\"1\"}\n")
        .await
        .unwrap();
    client.wait_for_frames_processed(2).await;
    let saturation = next_event(&mut client).await;
    assert!(
        matches!(
            saturation,
            CallosumReceiveEvent::Continuity {
                generation: 1,
                epoch: 2,
                phase: CallosumConnectionPhase::Gapped {
                    reason: CallosumGapReason::InboundSaturated,
                    dropped_count: 1
                }
            }
        ),
        "{saturation:?}"
    );
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Connected,
        }
    ));
    peer.write_all(b"{\"tract\":\"burst\",\"event\":\"after\"}\n")
        .await
        .unwrap();
    assert!(
        matches!(next_event(&mut client).await, CallosumReceiveEvent::Envelope {
        generation: 1, epoch: 2, envelope
    } if envelope.event == "after")
    );
    client.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_saturation_coalesces_one_gap_epoch() {
    let socket = TempSocket::new("continuity-dropped-marker");
    let listener = tokio::net::UnixListener::bind(&socket.path).unwrap();
    let mut client = CallosumSocketConnection::with_inbound_capacity(&socket.path, Map::new(), 1);
    client.start();
    let mut peer = accept_connected_raw_peer(&listener, &mut client).await;
    peer.write_all(b"{\"tract\":\"burst\",\"event\":\"one\"}\n")
        .await
        .unwrap();
    client.wait_for_frames_processed(1).await;
    peer.write_all(b"{\"tract\":\"burst\",\"event\":\"two\"}\n")
        .await
        .unwrap();
    client.wait_for_frames_processed(2).await;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Gapped {
                reason: CallosumGapReason::InboundSaturated,
                dropped_count: 1..
            }
        }
    ));
    client.stop().await;
}

async fn raw(path: &PathBuf) -> UnixStream {
    timeout(Duration::from_secs(2), UnixStream::connect(path))
        .await
        .expect("raw client should connect")
        .expect("raw socket connect should succeed")
}

async fn raw_line(reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>) -> String {
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(2), reader.read_until(b'\n', &mut bytes))
        .await
        .expect("raw line should arrive")
        .expect("raw line should read");
    String::from_utf8(bytes).expect("server frames are UTF-8")
}

#[tokio::test(flavor = "current_thread")]
async fn ac7_echoes_to_sender_and_preserves_unknown_fields() {
    let socket = TempSocket::new("ac7");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut first = connection(&socket.path, Map::new());
    let mut second = connection(&socket.path, Map::new());
    wait_for_clients(&server, 2).await;

    assert!(first.emit("future", "unknown", fields([("extension", json!(true))])));
    for message in [&mut first, &mut second] {
        let message = next(message).await;
        assert_eq!(message.tract, "future");
        assert_eq!(message.event, "unknown");
        assert_eq!(message.extra["extension"], json!(true));
    }

    first.stop().await;
    second.stop().await;
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac8_stamps_missing_timestamp_without_replacing_existing_timestamp() {
    let socket = TempSocket::new("ac8");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut sender = raw(&socket.path).await;
    let receiver = raw(&socket.path).await;
    wait_for_clients(&server, 2).await;
    let (receiver_read, _receiver_write) = receiver.into_split();
    let mut receiver = reader(receiver_read);

    sender
        .write_all(b"{\"tract\":\"time\",\"event\":\"missing\"}\n")
        .await
        .unwrap();
    let stamped = raw_line(&mut receiver).await;
    let stamped_value: Value = serde_json::from_str(stamped.trim()).unwrap();
    let timestamp = stamped_value["ts"].as_i64().expect("integer timestamp");
    assert!(timestamp > 0);
    assert!(!stamped.contains('.'));

    sender
        .write_all(b"{\"tract\":\"time\",\"event\":\"existing\",\"ts\":7}\n")
        .await
        .unwrap();
    let existing: Value = serde_json::from_str(raw_line(&mut receiver).await.trim()).unwrap();
    assert_eq!(existing["ts"], json!(7));
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac9_missing_required_fields_are_dropped_without_disconnect() {
    let socket = TempSocket::new("ac9");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut sender = raw(&socket.path).await;
    let receiver = raw(&socket.path).await;
    wait_for_clients(&server, 2).await;
    let (receiver_read, _receiver_write) = receiver.into_split();
    let mut receiver = reader(receiver_read);
    sender.write_all(b"{\"tract\":\"only\"}\n").await.unwrap();
    let mut bytes = Vec::new();
    assert!(
        timeout(
            Duration::from_millis(100),
            receiver.read_until(b'\n', &mut bytes)
        )
        .await
        .is_err()
    );
    sender
        .write_all(b"{\"tract\":\"valid\",\"event\":\"after\"}\n")
        .await
        .unwrap();
    let valid: Value = serde_json::from_str(raw_line(&mut receiver).await.trim()).unwrap();
    assert_eq!(valid["event"], json!("after"));
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac10_emit_merges_defaults_then_positional_then_caller_fields() {
    let socket = TempSocket::new("ac10");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let defaults = fields([
        ("tract", json!("default-tract")),
        ("event", json!("default-event")),
        ("kept", json!("default")),
        ("discarded", Value::Null),
    ]);
    let mut first = connection(&socket.path, defaults);
    let mut second = connection(&socket.path, Map::new());
    wait_for_clients(&server, 2).await;
    assert!(first.emit(
        "positional-tract",
        "positional-event",
        fields([
            ("tract", json!("caller-tract")),
            ("event", json!("caller-event")),
            ("extra", json!(1)),
        ]),
    ));
    for connection in [&mut first, &mut second] {
        let message = next(connection).await;
        assert_eq!(message.tract, "caller-tract");
        assert_eq!(message.event, "caller-event");
        assert_eq!(message.extra["kept"], json!("default"));
        assert!(message.extra.get("discarded").is_none());
        assert_eq!(message.extra["extra"], json!(1));
    }
    first.stop().await;
    second.stop().await;
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac11_malformed_json_is_dropped_without_disconnect() {
    let socket = TempSocket::new("ac11");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut sender = raw(&socket.path).await;
    let receiver = raw(&socket.path).await;
    wait_for_clients(&server, 2).await;
    let (receiver_read, _receiver_write) = receiver.into_split();
    let mut receiver = reader(receiver_read);
    sender.write_all(b"{not-json}\n").await.unwrap();
    let mut bytes = Vec::new();
    assert!(
        timeout(
            Duration::from_millis(100),
            receiver.read_until(b'\n', &mut bytes)
        )
        .await
        .is_err()
    );
    sender
        .write_all(b"{\"tract\":\"valid\",\"event\":\"after\"}\n")
        .await
        .unwrap();
    let valid: Value = serde_json::from_str(raw_line(&mut receiver).await.trim()).unwrap();
    assert_eq!(valid["tract"], json!("valid"));
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac12_exposes_malformed_and_single_eviction_counters() {
    let socket = TempSocket::new("ac12");
    let hooks = Arc::new(ServerTestHooks::default());
    let server = CallosumSocketServer::bind_with_test_hooks(&socket.path, Arc::clone(&hooks))
        .await
        .unwrap();
    let mut stalled = raw(&socket.path).await;
    let mut healthy = connection(&socket.path, Map::new());
    wait_for_clients(&server, 2).await;
    let _ = next_event(&mut healthy).await;
    stalled.write_all(b"{bad}\n").await.unwrap();
    wait_for_counter(|| server.malformed_frame_drops()).await;
    hooks.block_client(1);
    assert!(healthy.emit("counter", "evict", Map::new()));
    timeout(Duration::from_secs(1), hooks.wait_for_write())
        .await
        .expect("stalled writer should be selected");
    let _ = next(&mut healthy).await;
    wait_for_counter(|| server.stalled_client_evictions()).await;
    assert_eq!(server.malformed_frame_drops(), 1);
    assert_eq!(server.stalled_client_evictions(), 1);
    healthy.stop().await;
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac13_stalled_client_does_not_block_healthy_delivery_and_is_evicted() {
    let socket = TempSocket::new("ac13");
    let hooks = Arc::new(ServerTestHooks::default());
    let server = CallosumSocketServer::bind_with_test_hooks(&socket.path, Arc::clone(&hooks))
        .await
        .unwrap();
    let _stalled = raw(&socket.path).await;
    let mut healthy = connection(&socket.path, Map::new());
    wait_for_clients(&server, 2).await;
    let _ = next_event(&mut healthy).await;
    hooks.block_client(1);
    assert!(healthy.emit("stall", "first", Map::new()));
    timeout(Duration::from_secs(1), hooks.wait_for_write())
        .await
        .expect("stalled writer should block deterministically");
    assert_eq!(next(&mut healthy).await.event, "first");
    wait_for_counter(|| server.stalled_client_evictions()).await;
    assert_eq!(server.stalled_client_evictions(), 1);
    assert!(healthy.emit("stall", "second", Map::new()));
    assert_eq!(next(&mut healthy).await.event, "second");
    healthy.stop().await;
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac14_disconnected_emits_do_not_error_and_reconnect_delivers_later_messages() {
    let socket = TempSocket::new("ac14");
    let mut connection = connection(&socket.path, Map::new());
    assert!(connection.emit("offline", "dropped", Map::new()));
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    wait_for_clients(&server, 1).await;
    assert!(connection.emit("online", "delivered", Map::new()));
    assert_eq!(next(&mut connection).await.event, "delivered");
    connection.stop().await;
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac15_connection_receives_unknown_messages_with_extensions() {
    let socket = TempSocket::new("ac15");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut first = connection(&socket.path, Map::new());
    let mut second = connection(&socket.path, Map::new());
    wait_for_clients(&server, 2).await;
    assert!(first.emit(
        "future-tract",
        "future-event",
        fields([("extension", json!("kept"))])
    ));
    let _ = next(&mut first).await;
    let message = next(&mut second).await;
    assert_eq!(message.tract, "future-tract");
    assert_eq!(message.event, "future-event");
    assert_eq!(message.extra["extension"], json!("kept"));
    first.stop().await;
    second.stop().await;
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac16_stop_unlinks_socket_and_stale_paths_rebind() {
    let socket = TempSocket::new("ac16");
    fs::write(&socket.path, b"stale").unwrap();
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    assert!(socket.path.exists());
    server.stop().await;
    assert!(!socket.path.exists());
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac17_reads_multiple_and_split_utf8_frames() {
    let socket = TempSocket::new("ac17");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut sender = raw(&socket.path).await;
    let receiver = raw(&socket.path).await;
    wait_for_clients(&server, 2).await;
    let (receiver_read, _receiver_write) = receiver.into_split();
    let mut receiver = reader(receiver_read);
    sender
        .write_all(
            b"{\"tract\":\"batch\",\"event\":\"one\"}\n{\"tract\":\"batch\",\"event\":\"two\"}\n",
        )
        .await
        .unwrap();
    for event in ["one", "two"] {
        let message: Value = serde_json::from_str(raw_line(&mut receiver).await.trim()).unwrap();
        assert_eq!(message["event"], json!(event));
    }
    let split = "{\"tract\":\"utf8\",\"event\":\"h\u{e9}\"}\n".as_bytes();
    let split_at = split.iter().position(|byte| *byte == 0xc3).unwrap() + 1;
    sender.write_all(&split[..split_at]).await.unwrap();
    sender.write_all(&split[split_at..]).await.unwrap();
    let message: Value = serde_json::from_str(raw_line(&mut receiver).await.trim()).unwrap();
    assert_eq!(message["event"], json!("h\u{e9}"));
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac18_invalid_utf8_frame_is_dropped_but_peer_stays_usable() {
    let socket = TempSocket::new("ac18");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut sender = raw(&socket.path).await;
    let receiver = raw(&socket.path).await;
    wait_for_clients(&server, 2).await;
    let (receiver_read, _receiver_write) = receiver.into_split();
    let mut receiver = reader(receiver_read);
    sender
        .write_all(b"{\"tract\":\"utf8\",\"event\":\"\xff\"}\n")
        .await
        .unwrap();
    wait_for_counter(|| server.malformed_frame_drops()).await;
    sender
        .write_all(b"{\"tract\":\"valid\",\"event\":\"after\"}\n")
        .await
        .unwrap();
    let message: Value = serde_json::from_str(raw_line(&mut receiver).await.trim()).unwrap();
    assert_eq!(message["event"], json!("after"));
    assert_eq!(server.malformed_frame_drops(), 1);
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn escaped_large_unicode_frames_keep_peer_connected() {
    let socket = TempSocket::new("large-unicode");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut first = connection(&socket.path, Map::new());
    let mut second = connection(&socket.path, Map::new());
    wait_for_clients(&server, 2).await;

    let text = "日本語🦀".repeat(1_000);
    assert!(
        text.chars().count() * 6 > 4_096,
        "payload must exceed the 4096-byte read buffer after ASCII escaping"
    );
    assert!(first.emit("unicode", "large", fields([("text", json!(text))])));
    for peer in [&mut first, &mut second] {
        let message = next(peer).await;
        assert_eq!(message.tract, "unicode", "escaped-large-unicode tract");
        assert_eq!(message.event, "large", "escaped-large-unicode event");
        assert_eq!(
            message.extra["text"],
            json!(text),
            "escaped-large-unicode text"
        );
    }

    assert!(first.emit("unicode", "after", fields([("extension", json!(true))])));
    for peer in [&mut first, &mut second] {
        let message = next(peer).await;
        assert_eq!(
            message.event, "after",
            "escaped-large-unicode follow-up event"
        );
        assert_eq!(
            message.extra["extension"],
            json!(true),
            "escaped-large-unicode follow-up extension"
        );
    }

    first.stop().await;
    second.stop().await;
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn encode_envelope_is_compact_and_ascii_escapes_non_ascii() {
    let socket = TempSocket::new("encode-envelope");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut sender = connection(&socket.path, Map::new());
    let receiver = raw(&socket.path).await;
    wait_for_clients(&server, 2).await;
    let (receiver_read, receiver_write) = receiver.into_split();
    let mut receiver = reader(receiver_read);

    assert!(sender.emit(
        "unicode",
        "nested",
        fields([
            ("text", json!("日本語🦀")),
            ("payload", json!({"items": [1, {"state": "kept"}]})),
        ])
    ));
    let echoed = next(&mut sender).await;
    assert_eq!(echoed.event, "nested", "encode_envelope echo event");
    let line = raw_line(&mut receiver).await;
    drop(receiver_write);
    assert!(
        line.contains(r#"\u65e5\u672c\u8a9e\ud83e\udd80"#),
        "encode_envelope must ASCII-escape 日本語🦀 as the Python ensure_ascii form: {line}"
    );
    let mut object: Value = serde_json::from_str(line.trim()).expect("encode_envelope JSON");
    object
        .as_object_mut()
        .expect("encode_envelope object")
        .remove("ts");
    assert_eq!(
        object,
        json!({
            "tract": "unicode",
            "event": "nested",
            "text": "日本語🦀",
            "payload": {"items": [1, {"state": "kept"}]},
        }),
        "encode_envelope compact object minus ts"
    );
    assert!(
        !line.contains(", ") && !line.contains(": "),
        "encode_envelope must emit compact separators: {line}"
    );
    assert!(
        line.contains("\\u"),
        "encode_envelope must ASCII-escape non-ASCII: {line}"
    );

    sender.stop().await;
    server.stop().await;
}
