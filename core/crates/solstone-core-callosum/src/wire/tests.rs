// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_system::queue::TaskQueueStatusSnapshot;
use solstone_core_system::status_wire::{
    ProcessObservation, ServiceCandidate, SupervisorStatusWireInput, project_supervisor_status,
};
use tokio::io::AsyncWriteExt;

use super::connection::{CallosumConnectionPhase, CallosumGapReason, CallosumReceiveEvent};
use super::framing::{ReadFrame, read_frame, reader};

#[test]
fn continuity_markers_are_explicit_and_cloneable() {
    let event = CallosumReceiveEvent::Continuity {
        generation: 7,
        epoch: 9,
        phase: CallosumConnectionPhase::Gapped {
            reason: CallosumGapReason::InboundSaturated,
            dropped_count: 3,
        },
    };
    assert!(matches!(
        event.clone(),
        CallosumReceiveEvent::Continuity {
            generation: 7,
            epoch: 9,
            phase: CallosumConnectionPhase::Gapped {
                reason: CallosumGapReason::InboundSaturated,
                dropped_count: 3,
            },
        }
    ));
}

fn status_input(services: Vec<ServiceCandidate>) -> SupervisorStatusWireInput {
    SupervisorStatusWireInput {
        services,
        crashed: vec![],
        queue: TaskQueueStatusSnapshot {
            tasks: vec![],
            recent_tasks: vec![],
            queues: Default::default(),
            held: vec![],
            queue_hold: None,
        },
        stale_heartbeats: vec![],
        schedules: vec![],
        callosum_clients: 0,
        sense_pending_queue_depth: None,
        sense_pending_age_ms: None,
        sense_pending_received: false,
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
    let mut buffer = crate::local_inference::FrameAccum::new();
    let mut decoded = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        decoded.push(read_frame(&mut frame_reader, &mut buffer).await.unwrap());
    }
    writer_task.await.unwrap();
    decoded
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
async fn ac8_stamps_missing_timestamp_without_replacing_existing_timestamp_in_memory() {
    let mut missing = super::super::CallosumEnvelope {
        tract: "time".into(),
        event: "missing".into(),
        ts: None,
        extra: Map::new(),
    };
    super::server::stamp_timestamp(&mut missing);
    assert!(missing.ts.expect("integer timestamp") > 0);
    let encoded = super::frame::encode_envelope(&missing).unwrap();
    assert!(!encoded.contains(&b'.'));
    let mut existing = super::super::CallosumEnvelope {
        tract: "time".into(),
        event: "existing".into(),
        ts: Some(7),
        extra: Map::new(),
    };
    super::server::stamp_timestamp(&mut existing);
    assert_eq!(existing.ts, Some(7));
}

#[tokio::test(flavor = "current_thread")]
async fn ac9_missing_required_fields_decode_as_malformed() {
    let valid = super::frame::encode_envelope(&super::super::CallosumEnvelope {
        tract: "valid".into(),
        event: "after".into(),
        ts: None,
        extra: Map::new(),
    })
    .unwrap();
    let frames = decode_fragmented(vec![b"{\"tract\":\"only\"}\n".to_vec(), valid], 64).await;
    assert!(matches!(
        frames.as_slice(),
        [ReadFrame::Malformed, ReadFrame::Envelope(envelope)] if envelope.event == "after"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn ac11_malformed_json_decodes_as_malformed() {
    let valid = super::frame::encode_envelope(&super::super::CallosumEnvelope {
        tract: "valid".into(),
        event: "after".into(),
        ts: None,
        extra: Map::new(),
    })
    .unwrap();
    let frames = decode_fragmented(vec![b"{not-json}\n".to_vec(), valid], 64).await;
    assert!(matches!(
        frames.as_slice(),
        [ReadFrame::Malformed, ReadFrame::Envelope(envelope)] if envelope.event == "after"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn ac17_reads_multiple_and_split_utf8_frames_in_memory() {
    let one = super::frame::encode_envelope(&super::super::CallosumEnvelope {
        tract: "batch".into(),
        event: "one".into(),
        ts: None,
        extra: Map::new(),
    })
    .unwrap();
    let two = super::frame::encode_envelope(&super::super::CallosumEnvelope {
        tract: "batch".into(),
        event: "two".into(),
        ts: None,
        extra: Map::new(),
    })
    .unwrap();
    let frames = decode_fragmented(vec![one, two], 37).await;
    assert!(matches!(
        frames.as_slice(),
        [
            ReadFrame::Envelope(first),
            ReadFrame::Envelope(second)
        ] if first.event == "one" && second.event == "two"
    ));
    let split = "{\"tract\":\"utf8\",\"event\":\"h\u{e9}\"}\n"
        .as_bytes()
        .to_vec();
    let split_at = split.iter().position(|byte| *byte == 0xc3).unwrap() + 1;
    let frames = decode_fragmented(vec![split], split_at).await;
    assert!(matches!(
        frames.as_slice(),
        [ReadFrame::Envelope(envelope)] if envelope.event == "h\u{e9}"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn ac18_invalid_utf8_frame_decodes_as_invalid() {
    let valid = super::frame::encode_envelope(&super::super::CallosumEnvelope {
        tract: "valid".into(),
        event: "after".into(),
        ts: None,
        extra: Map::new(),
    })
    .unwrap();
    let frames = decode_fragmented(
        vec![b"{\"tract\":\"utf8\",\"event\":\"\xff\"}\n".to_vec(), valid],
        64,
    )
    .await;
    assert!(matches!(
        frames.as_slice(),
        [ReadFrame::InvalidUtf8, ReadFrame::Envelope(envelope)] if envelope.event == "after"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn private_request_routing_isolates_reply_and_counts_source_calls() {
    use super::server::{CallosumSocketServer, route_client_frame};
    use crate::local_inference::{
        ExchangeSession, LocalInferencePrivateRequest, LocalInferenceSnapshot,
        LocalInferenceSnapshotOffer,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (server, _broadcast_rx) = CallosumSocketServer::new_routing_test();

    let secret = [42_u8; 32];
    server.set_test_snapshot_secret(Some(secret));

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&call_count);
    server.set_local_inference_snapshot_source(move || {
        let n = count_clone.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            LocalInferenceSnapshotOffer::Ready(
                LocalInferenceSnapshot::try_new(1, 8001, b"token-client-1").unwrap(),
            )
        } else {
            LocalInferenceSnapshotOffer::Ready(
                LocalInferenceSnapshot::try_new(2, 8002, b"token-client-2").unwrap(),
            )
        }
    });

    // Two requesters with different connection IDs (1, 2) and client 3
    let (tx1, mut rx1) = tokio::sync::mpsc::channel(64);
    let (tx2, mut rx2) = tokio::sync::mpsc::channel(64);
    let (tx3, mut rx3) = tokio::sync::mpsc::channel(64);
    let (sd1, _) = tokio::sync::watch::channel(false);
    let (sd2, _) = tokio::sync::watch::channel(false);
    let (sd3, _) = tokio::sync::watch::channel(false);

    {
        let mut clients = server.inner.clients.lock().unwrap();
        clients.insert(
            1,
            super::server::ClientEntry {
                outbound: tx1,
                shutdown: sd1,
                _task: tokio::spawn(async {}),
            },
        );
        clients.insert(
            2,
            super::server::ClientEntry {
                outbound: tx2,
                shutdown: sd2,
                _task: tokio::spawn(async {}),
            },
        );
        clients.insert(
            3,
            super::server::ClientEntry {
                outbound: tx3,
                shutdown: sd3,
                _task: tokio::spawn(async {}),
            },
        );
    }

    let nonce1 = [11_u8; 32];
    let req1 = LocalInferencePrivateRequest {
        correlation: 101,
        nonce: nonce1,
    };
    route_client_frame(&server.inner, 1, ReadFrame::PrivateRequest(req1));

    let nonce2 = [22_u8; 32];
    let req2 = LocalInferencePrivateRequest {
        correlation: 202,
        nonce: nonce2,
    };
    route_client_frame(&server.inner, 2, ReadFrame::PrivateRequest(req2));

    assert_eq!(call_count.load(Ordering::SeqCst), 2);
    assert_ne!(call_count.load(Ordering::SeqCst), 0);

    // Each outbound contains only its own token
    let reply1 = rx1.try_recv().expect("client 1 receives its reply");
    let reply2 = rx2.try_recv().expect("client 2 receives its reply");
    assert!(rx3.try_recv().is_err(), "client 3 must receive no bytes");

    let snap1 =
        ExchangeSession::verify_and_decode_response(&secret, &nonce1, 101, &reply1).unwrap();
    assert_eq!(snap1.generation(), 1);
    assert_eq!(snap1.port(), 8001);
    assert_eq!(snap1.token(), b"token-client-1");

    let snap2 =
        ExchangeSession::verify_and_decode_response(&secret, &nonce2, 202, &reply2).unwrap();
    assert_eq!(snap2.generation(), 2);
    assert_eq!(snap2.port(), 8002);
    assert_eq!(snap2.token(), b"token-client-2");
}

#[tokio::test(flavor = "current_thread")]
async fn successive_offers_return_latest_snapshot_and_call_source_each_time() {
    use super::server::{CallosumSocketServer, route_client_frame};
    use crate::local_inference::{
        ExchangeSession, LocalInferencePrivateRequest, LocalInferenceSnapshot,
        LocalInferenceSnapshotOffer,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (server, _broadcast_rx) = CallosumSocketServer::new_routing_test();

    let secret = [42_u8; 32];
    server.set_test_snapshot_secret(Some(secret));

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&call_count);
    server.set_local_inference_snapshot_source(move || {
        let n = count_clone.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            LocalInferenceSnapshotOffer::Ready(
                LocalInferenceSnapshot::try_new(1, 8001, b"token-A").unwrap(),
            )
        } else {
            LocalInferenceSnapshotOffer::Ready(
                LocalInferenceSnapshot::try_new(2, 8002, b"token-B").unwrap(),
            )
        }
    });

    let (tx1, mut rx1) = tokio::sync::mpsc::channel(64);
    let (sd1, _) = tokio::sync::watch::channel(false);
    server.inner.clients.lock().unwrap().insert(
        1,
        super::server::ClientEntry {
            outbound: tx1,
            shutdown: sd1,
            _task: tokio::spawn(async {}),
        },
    );

    let nonce1 = [1_u8; 32];
    let req1 = LocalInferencePrivateRequest {
        correlation: 1,
        nonce: nonce1,
    };
    route_client_frame(&server.inner, 1, ReadFrame::PrivateRequest(req1));
    let reply1 = rx1.try_recv().unwrap();
    let snap1 = ExchangeSession::verify_and_decode_response(&secret, &nonce1, 1, &reply1).unwrap();
    assert_eq!(snap1.generation(), 1);
    assert_eq!(snap1.token(), b"token-A");

    let nonce2 = [2_u8; 32];
    let req2 = LocalInferencePrivateRequest {
        correlation: 2,
        nonce: nonce2,
    };
    route_client_frame(&server.inner, 1, ReadFrame::PrivateRequest(req2));
    let reply2 = rx1.try_recv().unwrap();
    let snap2 = ExchangeSession::verify_and_decode_response(&secret, &nonce2, 2, &reply2).unwrap();
    assert_eq!(snap2.generation(), 2);
    assert_eq!(snap2.token(), b"token-B");

    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn removed_connection_drops_reply_and_correlation_reuse_is_not_misrouted() {
    use super::server::{CallosumSocketServer, route_client_frame};
    use crate::local_inference::{
        ExchangeSession, LocalInferencePrivateRequest, LocalInferenceSnapshot,
        LocalInferenceSnapshotOffer,
    };

    let (server, _broadcast_rx) = CallosumSocketServer::new_routing_test();

    let secret = [42_u8; 32];
    server.set_test_snapshot_secret(Some(secret));
    server.set_local_inference_snapshot_source(|| {
        LocalInferenceSnapshotOffer::Ready(
            LocalInferenceSnapshot::try_new(1, 8080, b"tok").unwrap(),
        )
    });

    // Client 1 connects
    let (tx1, mut rx1) = tokio::sync::mpsc::channel(64);
    let (sd1, _) = tokio::sync::watch::channel(false);
    server.inner.clients.lock().unwrap().insert(
        1,
        super::server::ClientEntry {
            outbound: tx1,
            shutdown: sd1,
            _task: tokio::spawn(async {}),
        },
    );

    // Client 1 is removed
    server.inner.clients.lock().unwrap().remove(&1);

    // Request routed to old client ID 1 -> reply cannot be delivered
    let req1 = LocalInferencePrivateRequest {
        correlation: 100,
        nonce: [1_u8; 32],
    };
    route_client_frame(&server.inner, 1, ReadFrame::PrivateRequest(req1));
    assert!(rx1.try_recv().is_err());

    // Later client ID 2 connects and reuses correlation ID 100
    let (tx2, mut rx2) = tokio::sync::mpsc::channel(64);
    let (sd2, _) = tokio::sync::watch::channel(false);
    server.inner.clients.lock().unwrap().insert(
        2,
        super::server::ClientEntry {
            outbound: tx2,
            shutdown: sd2,
            _task: tokio::spawn(async {}),
        },
    );

    let nonce2 = [2_u8; 32];
    let req2 = LocalInferencePrivateRequest {
        correlation: 100,
        nonce: nonce2,
    };
    route_client_frame(&server.inner, 2, ReadFrame::PrivateRequest(req2));

    // The new connection's own later reply is the only bytes it receives
    let reply = rx2.try_recv().expect("client 2 receives its reply");
    let snap = ExchangeSession::verify_and_decode_response(&secret, &nonce2, 100, &reply).unwrap();
    assert_eq!(snap.generation(), 1);
    assert!(rx2.try_recv().is_err(), "client 2 receives no extra bytes");
}

#[tokio::test(flavor = "current_thread")]
async fn unconfigured_source_and_secret_absent_behaviors() {
    use super::server::{CallosumSocketServer, route_client_frame};
    use crate::local_inference::{
        ExchangeSession, LocalInferencePrivateRequest, LocalInferenceReadError,
    };

    let (server, _broadcast_rx) = CallosumSocketServer::new_routing_test();

    let secret = [42_u8; 32];
    server.set_test_snapshot_secret(Some(secret));
    // No snapshot source registered!

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let (sd, _) = tokio::sync::watch::channel(false);
    server.inner.clients.lock().unwrap().insert(
        1,
        super::server::ClientEntry {
            outbound: tx,
            shutdown: sd,
            _task: tokio::spawn(async {}),
        },
    );

    let req = LocalInferencePrivateRequest {
        correlation: 1,
        nonce: [1_u8; 32],
    };
    route_client_frame(&server.inner, 1, ReadFrame::PrivateRequest(req));

    let reply = rx
        .try_recv()
        .expect("receives authenticated unavailable reply");
    let err =
        ExchangeSession::verify_and_decode_response(&secret, &[1_u8; 32], 1, &reply).unwrap_err();
    assert_eq!(err, LocalInferenceReadError::Unavailable);

    // If secret is None -> no reply at all
    server.set_test_snapshot_secret(None);
    route_client_frame(&server.inner, 1, ReadFrame::PrivateRequest(req));
    assert!(rx.try_recv().is_err(), "no reply when secret is absent");
}

#[tokio::test(flavor = "current_thread")]
async fn private_rejected_frames_feed_through_read_and_route_frame() {
    use super::framing::{ReadFrame, read_frame, reader};
    use super::server::{CallosumSocketServer, route_client_frame};
    use crate::local_inference::{FrameAccum, LocalInferenceSnapshot, LocalInferenceSnapshotOffer};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (server, mut broadcast_rx) = CallosumSocketServer::new_routing_test();

    let secret = [42_u8; 32];
    server.set_test_snapshot_secret(Some(secret));

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&call_count);
    server.set_local_inference_snapshot_source(move || {
        count_clone.fetch_add(1, Ordering::SeqCst);
        LocalInferenceSnapshotOffer::Ready(
            LocalInferenceSnapshot::try_new(1, 8080, b"secret-token").unwrap(),
        )
    });

    let (tx1, mut rx1) = tokio::sync::mpsc::channel(64);
    let (sd1, _) = tokio::sync::watch::channel(false);
    server.inner.clients.lock().unwrap().insert(
        1,
        super::server::ClientEntry {
            outbound: tx1,
            shutdown: sd1,
            _task: tokio::spawn(async {}),
        },
    );

    // 1. Ambiguous SCLP JSON
    let mut r1 = reader(&b"SCLP{\"tract\":\"observe\",\"event\":\"tick\"}\n"[..]);
    let mut accum1 = FrameAccum::new();
    let frame1 = read_frame(&mut r1, &mut accum1).await.unwrap();
    assert!(matches!(frame1, ReadFrame::PrivateRejected));
    route_client_frame(&server.inner, 1, frame1);

    // 2. Unknown version (99)
    let mut buf_bad_ver = [0_u8; 50];
    buf_bad_ver[..4].copy_from_slice(b"SCLP");
    buf_bad_ver[4] = 99;
    buf_bad_ver[5] = 1;
    buf_bad_ver[6..10].copy_from_slice(&40_u32.to_le_bytes());
    let mut r2 = reader(&buf_bad_ver[..]);
    let mut accum2 = FrameAccum::new();
    let frame2 = read_frame(&mut r2, &mut accum2).await.unwrap();
    assert!(matches!(frame2, ReadFrame::PrivateRejected));
    route_client_frame(&server.inner, 1, frame2);

    // 3. Unknown op (99)
    let mut buf_bad_op = [0_u8; 50];
    buf_bad_op[..4].copy_from_slice(b"SCLP");
    buf_bad_op[4] = 1;
    buf_bad_op[5] = 99;
    buf_bad_op[6..10].copy_from_slice(&40_u32.to_le_bytes());
    let mut r3 = reader(&buf_bad_op[..]);
    let mut accum3 = FrameAccum::new();
    let frame3 = read_frame(&mut r3, &mut accum3).await.unwrap();
    assert!(matches!(frame3, ReadFrame::PrivateRejected));
    route_client_frame(&server.inner, 1, frame3);

    // Source call count stays 0
    assert_eq!(call_count.load(Ordering::SeqCst), 0);
    // Broadcasts receiver gets nothing
    assert!(broadcast_rx.try_recv().is_err());
    // Token bytes are absent in outbound
    assert!(rx1.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn full_outbound_evicts_stalled_client_without_affecting_peers() {
    use super::server::{CallosumSocketServer, route_client_frame};
    use crate::CallosumEnvelope;
    use crate::local_inference::{
        LocalInferencePrivateRequest, LocalInferenceSnapshot, LocalInferenceSnapshotOffer,
    };

    let (server, _broadcast_rx) = CallosumSocketServer::new_routing_test();

    let secret = [42_u8; 32];
    server.set_test_snapshot_secret(Some(secret));
    server.set_local_inference_snapshot_source(|| {
        LocalInferenceSnapshotOffer::Ready(
            LocalInferenceSnapshot::try_new(1, 8080, b"token").unwrap(),
        )
    });

    let (tx1, _rx1) = tokio::sync::mpsc::channel(super::SERVER_CLIENT_OUTBOUND_CAPACITY);
    let (tx2, mut rx2) = tokio::sync::mpsc::channel(super::SERVER_CLIENT_OUTBOUND_CAPACITY);
    let (sd1, _) = tokio::sync::watch::channel(false);
    let (sd2, _) = tokio::sync::watch::channel(false);

    // Fill client 1's channel to SERVER_CLIENT_OUTBOUND_CAPACITY
    for i in 0..super::SERVER_CLIENT_OUTBOUND_CAPACITY {
        tx1.try_send(vec![i as u8]).unwrap();
    }

    {
        let mut clients = server.inner.clients.lock().unwrap();
        clients.insert(
            1,
            super::server::ClientEntry {
                outbound: tx1,
                shutdown: sd1,
                _task: tokio::spawn(async {}),
            },
        );
        clients.insert(
            2,
            super::server::ClientEntry {
                outbound: tx2,
                shutdown: sd2,
                _task: tokio::spawn(async {}),
            },
        );
    }

    let req = LocalInferencePrivateRequest {
        correlation: 1,
        nonce: [1_u8; 32],
    };
    route_client_frame(&server.inner, 1, ReadFrame::PrivateRequest(req));

    // Client 1 should be evicted due to full outbound channel
    {
        let clients = server.inner.clients.lock().unwrap();
        assert!(!clients.contains_key(&1));
        assert!(clients.contains_key(&2));
    }

    // Route an ordinary envelope and show the other client still receives it
    let env = CallosumEnvelope {
        tract: "observe".to_string(),
        event: "tick".to_string(),
        ts: None,
        extra: Default::default(),
    };
    route_client_frame(&server.inner, 2, ReadFrame::Envelope(env.clone()));
    let line = super::frame::encode_envelope(&env).unwrap();
    super::server::enqueue_client_bytes(&server.inner, 2, line.clone());
    assert_eq!(rx2.try_recv().unwrap(), line);
}

#[tokio::test(flavor = "current_thread")]
async fn duplex_exchange_async_and_spawn_blocking_consumer() {
    use super::framing::{ReadFrame, read_frame, reader};
    use super::local_inference_client::drive_exchange_async;
    use crate::local_inference::{ExchangeSession, encode_response_frame};
    use std::time::{Duration, Instant};

    let secret = [11_u8; 32];
    let (mut client_stream, server_stream) = tokio::io::duplex(256);

    let server_task = tokio::spawn(async move {
        let mut frame_reader = reader(server_stream);
        let mut accum = crate::local_inference::FrameAccum::new();
        let frame = read_frame(&mut frame_reader, &mut accum).await.unwrap();
        match frame {
            ReadFrame::PrivateRequest(req) => {
                let reply = encode_response_frame(
                    &secret,
                    crate::local_inference::PRIVATE_KIND_CREDENTIAL,
                    req.correlation,
                    &req.nonce,
                    42,
                    9090,
                    b"tok-42",
                );
                let (read_half, mut write_half) = tokio::io::split(frame_reader.into_inner());
                write_half.write_all(&reply).await.unwrap();
                let _ = read_half;
            }
            _ => panic!("expected PrivateRequest"),
        }
    });

    let nonce = [77_u8; 32];
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut session = ExchangeSession::new(secret, 999, nonce, deadline);
    let snapshot = drive_exchange_async(&mut client_stream, &mut session)
        .await
        .unwrap();
    server_task.await.unwrap();

    assert_eq!(snapshot.generation(), 42);
    assert_eq!(snapshot.port(), 9090);
    assert_eq!(snapshot.token(), b"tok-42");

    // Pass owned snapshot to spawn_blocking worker
    let worker = tokio::task::spawn_blocking(move || {
        assert_eq!(snapshot.generation(), 42);
        assert_eq!(snapshot.port(), 9090);
        assert_eq!(snapshot.token(), b"tok-42");
        snapshot.generation()
    });
    assert_eq!(worker.await.unwrap(), 42);
}

#[tokio::test(flavor = "current_thread")]
async fn sync_entry_from_entered_runtime_and_spawn_blocking_returns_runtime_entered() {
    use super::local_inference_client::request_local_inference_snapshot_sync;
    use crate::local_inference::LocalInferenceReadError;
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(5);
    let res = request_local_inference_snapshot_sync("health/callosum.sock", deadline);
    assert_eq!(res.unwrap_err(), LocalInferenceReadError::RuntimeEntered);

    let blocking_res = tokio::task::spawn_blocking(move || {
        request_local_inference_snapshot_sync("health/callosum.sock", deadline)
    })
    .await
    .unwrap();
    assert_eq!(
        blocking_res.unwrap_err(),
        LocalInferenceReadError::RuntimeEntered
    );
}

#[test]
fn sync_entry_from_plain_thread_on_host_returns_unsupported() {
    use super::local_inference_client::request_local_inference_snapshot_sync;
    use crate::local_inference::LocalInferenceReadError;
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(5);
    let res = request_local_inference_snapshot_sync("health/callosum.sock", deadline);
    assert_eq!(res.unwrap_err(), LocalInferenceReadError::Unsupported);
}
