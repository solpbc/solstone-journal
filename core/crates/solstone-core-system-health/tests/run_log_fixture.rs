// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_system_health::{HealthEvent, RunLogRecord};

#[test]
fn all_known_event_kinds_decode_as_typed_variants() {
    for event in [
        "activity.detected",
        "activity.persisted",
        "activity.prompts_skipped",
        "activity.unchanged",
        "group.start",
        "group.complete",
        "memory_throttle.complete",
        "phase.start",
        "phase.complete",
        "run.start",
        "run.complete",
        "sense.skip",
        "sense.complete",
        "sense.change_detect",
        "talent.dispatch",
        "talent.complete",
        "talent.fail",
        "talent.skip",
    ] {
        let record: RunLogRecord =
            serde_json::from_value(json!({"event": event, "ts": 1, "mode": "segment"})).unwrap();
        assert!(
            !matches!(record.event, HealthEvent::Unknown(_, _)),
            "{event}"
        );
    }
}

#[test]
fn unknown_and_mismatched_optional_fields_round_trip() {
    let input = json!({"event":"future.kind","ts":2,"nested":{"ok":true},"count":7});
    let record: RunLogRecord = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(serde_json::to_value(record).unwrap(), input);

    let input = json!({"event":"talent.skip","ts":2,"mode":7,"cache_hit":"no","future":true});
    let record: RunLogRecord = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(serde_json::to_value(record).unwrap(), input);
}

#[test]
fn mode_less_dispatch_skip_is_a_valid_known_event() {
    let record: RunLogRecord = serde_json::from_value(json!({
        "event": "talent.skip",
        "ts": 1424,
        "name": "documents",
        "reason": "skip_talents_flag",
        "detail": "dispatch disabled",
        "day": "20990202",
    }))
    .unwrap();
    let HealthEvent::TalentSkip(payload) = record.event else {
        panic!("expected talent.skip");
    };
    assert_eq!(payload.mode, None);
    assert_eq!(payload.reason.as_deref(), Some("skip_talents_flag"));
}

#[test]
fn missing_or_invalid_envelope_is_rejected_by_decoder() {
    for value in [
        json!({"ts": 1}),
        json!({"event": 1, "ts": 1}),
        json!({"event":"run.start"}),
        json!({"event":"run.start","ts":1.5}),
        Value::Null,
    ] {
        assert!(serde_json::from_value::<RunLogRecord>(value).is_err());
    }
}
