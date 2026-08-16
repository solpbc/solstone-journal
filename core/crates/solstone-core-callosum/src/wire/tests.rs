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
