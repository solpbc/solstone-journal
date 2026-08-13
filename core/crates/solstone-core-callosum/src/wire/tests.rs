// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_system::queue::TaskQueueStatusSnapshot;
use solstone_core_system::status_wire::{
    ProcessObservation, ServiceCandidate, SupervisorStatusWireInput, project_supervisor_status,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::{sleep, timeout};

use super::connection::{CallosumDiscontinuity, CallosumReceiveEvent, CallosumSocketConnection};
use super::framing::{ReadFrame, read_frame, reader};
use super::server::{CallosumSocketServer, ServerTestHooks};

static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);

#[test]
fn continuity_markers_are_explicit_and_cloneable() {
    let event = CallosumReceiveEvent::Discontinuity {
        generation: 7,
        reason: CallosumDiscontinuity::InboundSaturated,
    };
    assert!(matches!(
        event.clone(),
        CallosumReceiveEvent::Discontinuity {
            generation: 7,
            reason: CallosumDiscontinuity::InboundSaturated,
        }
    ));
}

struct TempSocket {
    root: PathBuf,
    path: PathBuf,
}

impl TempSocket {
    fn new(name: &str) -> Self {
        let ordinal = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "solstone-core-callosum-wire-{name}-{}-{ordinal}",
            std::process::id()
        ));
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

async fn next(connection: &mut CallosumSocketConnection) -> super::super::CallosumEnvelope {
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

#[tokio::test(flavor = "current_thread")]
async fn continuity_generation_connected_precedes_envelopes_and_next_message_skips_markers() {
    let socket = TempSocket::new("continuity-generation");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut first = connection(&socket.path, Map::new());
    wait_for_clients(&server, 1).await;
    assert!(matches!(
        next_event(&mut first).await,
        CallosumReceiveEvent::Discontinuity {
            generation: 1,
            reason: CallosumDiscontinuity::Connected
        }
    ));
    assert!(first.emit("top", "one", Map::new()));
    assert!(
        matches!(next_event(&mut first).await, CallosumReceiveEvent::Envelope { generation: 1, envelope } if envelope.tract == "top" && envelope.event == "one")
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
async fn continuity_reconnect_increments_generation_and_connects_before_the_new_envelope() {
    let socket = TempSocket::new("continuity-reconnect");
    let server = CallosumSocketServer::bind(&socket.path).await.unwrap();
    let mut client = connection(&socket.path, Map::new());
    wait_for_clients(&server, 1).await;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Discontinuity {
            generation: 1,
            reason: CallosumDiscontinuity::Connected
        }
    ));
    server.stop().await;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Discontinuity {
            generation: 1,
            reason: CallosumDiscontinuity::Disconnected
        }
    ));

    let replacement = CallosumSocketServer::bind(&socket.path).await.unwrap();
    wait_for_clients(&replacement, 1).await;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Discontinuity {
            generation: 2,
            reason: CallosumDiscontinuity::Connected
        }
    ));
    assert!(client.emit("continuity", "after-reconnect", Map::new()));
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Envelope { generation: 2, envelope }
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
        CallosumReceiveEvent::Discontinuity {
            generation: 1,
            reason: CallosumDiscontinuity::Disconnected
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
    assert!(matches!(
        next_event(client).await,
        CallosumReceiveEvent::Discontinuity {
            generation: 1,
            reason: CallosumDiscontinuity::Connected
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
        CallosumReceiveEvent::Discontinuity {
            generation: 1,
            reason: CallosumDiscontinuity::MalformedFrameDropped
        }
    ));
    peer.write_all(b"{\"tract\":\"valid\",\"event\":\"after\"}\n")
        .await
        .unwrap();
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Envelope { generation: 1, envelope }
            if envelope.tract == "valid" && envelope.event == "after"
    ));
    client.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn continuity_inbound_saturation_marks_current_generation() {
    let socket = TempSocket::new("continuity-saturation");
    let listener = tokio::net::UnixListener::bind(&socket.path).unwrap();
    let mut client =
        CallosumSocketConnection::with_test_inbound_capacity(&socket.path, Map::new(), 1);
    client.start();
    let mut peer = accept_connected_raw_peer(&listener, &mut client).await;
    peer.write_all(b"{\"tract\":\"burst\",\"event\":\"0\"}\n")
        .await
        .unwrap();
    sleep(Duration::from_millis(50)).await;
    peer.write_all(b"{\"tract\":\"burst\",\"event\":\"1\"}\n")
        .await
        .unwrap();
    sleep(Duration::from_millis(50)).await;
    assert!(matches!(
        next_event(&mut client).await,
        CallosumReceiveEvent::Envelope { generation: 1, .. }
    ));
    peer.write_all(b"\n").await.unwrap();
    let saturation = next_event(&mut client).await;
    assert!(
        matches!(
            saturation,
            CallosumReceiveEvent::Discontinuity {
                generation: 1,
                reason: CallosumDiscontinuity::InboundSaturated
            }
        ),
        "{saturation:?}"
    );
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

fn status_input(services: Vec<ServiceCandidate>) -> SupervisorStatusWireInput {
    SupervisorStatusWireInput {
        services,
        crashed: vec![],
        queue: TaskQueueStatusSnapshot {
            tasks: vec![],
            recent_tasks: vec![],
            queues: Default::default(),
        },
        stale_heartbeats: vec![],
        schedules: vec![],
        callosum_clients: 0,
    }
}

async fn decode_fragmented(frames: Vec<Vec<u8>>, chunk: usize) -> Vec<ReadFrame> {
    let frame_count = frames.len();
    let (mut writer, read_half) = tokio::io::duplex(64);
    let writer_task = tokio::spawn(async move {
        for frame in frames {
            for fragment in frame.chunks(chunk) {
                writer.write_all(fragment).await.unwrap();
            }
        }
    });
    let mut frame_reader = reader(read_half);
    let mut buffer = Vec::new();
    let mut decoded = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        decoded.push(read_frame(&mut frame_reader, &mut buffer).await.unwrap());
    }
    writer_task.await.unwrap();
    decoded
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
async fn ac6_reads_a_fragmented_oversized_projected_status_frame() {
    let services = (0..40)
        .map(|index| ServiceCandidate::App {
            name: format!("service-{index:02}-{}", "n".repeat(64)),
            observation: ProcessObservation::Live {
                reference: format!("ref-{index:02}-{}", "r".repeat(64)),
                pid: index + 1,
                uptime_seconds: index as u64,
            },
        })
        .collect();
    let projected = project_supervisor_status(status_input(services));
    let projector_bytes = serde_json::to_vec(&Value::Object(projected.clone())).unwrap();
    assert!(projector_bytes.len() > super::READ_BUFFER_CAPACITY);
    let encoded = super::frame::encode_envelope(&super::super::CallosumEnvelope {
        tract: "supervisor".into(),
        event: "status".into(),
        ts: None,
        extra: projected.clone(),
    })
    .unwrap();
    assert!(encoded.len() > super::READ_BUFFER_CAPACITY);
    let small = super::frame::encode_envelope(&super::super::CallosumEnvelope {
        tract: "after".into(),
        event: "clean-buffer".into(),
        ts: None,
        extra: Map::new(),
    })
    .unwrap();
    let mut frames = decode_fragmented(vec![encoded.clone(), small], 37).await;
    let ReadFrame::Envelope(envelope) = frames.remove(0) else {
        panic!("oversized frame must decode")
    };
    assert_eq!(envelope.extra, projected);
    assert_eq!(
        envelope.extra["services"][39]["ref"],
        format!("ref-39-{}", "r".repeat(64))
    );
    let ReadFrame::Envelope(after) = frames.remove(0) else {
        panic!("following frame must decode")
    };
    assert_eq!(after.event, "clean-buffer");
    let one_chunk = decode_fragmented(vec![encoded[..37].to_vec()], 37).await;
    assert!(matches!(one_chunk.as_slice(), [ReadFrame::Malformed]));
    let truncated = decode_fragmented(vec![encoded[..encoded.len() - 2].to_vec()], 37).await;
    assert!(matches!(truncated.as_slice(), [ReadFrame::Malformed]));
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
