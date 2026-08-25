// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_generate::{
    ClientError, GenerateRequest, GenerateResponse, GeneratedResponse, RefusalReason,
    RefusedResponse,
};
use solstone_core_import::{
    TextImportError, TextWirePhase, WireClient, process_transcript_with_wire,
};
use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};

struct RecordingWire {
    responses: RefCell<VecDeque<Result<GenerateResponse, ClientError>>>,
    requests: RefCell<Vec<GenerateRequest>>,
}

impl RecordingWire {
    fn new(responses: Vec<Result<GenerateResponse, ClientError>>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl WireClient for RecordingWire {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.requests.borrow_mut().push(request.clone());
        self.responses
            .borrow_mut()
            .pop_front()
            .expect("test wire has a response for every request")
    }
}

fn generated(text: Value) -> Result<GenerateResponse, ClientError> {
    Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
        id: None,
        text: text.to_string(),
        model: "test-model".to_owned(),
        usage: json!({}),
        finish_reason: "stop".to_owned(),
        thinking: None,
        schema_validation: None,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied: Vec::new(),
    })))
}

fn refused() -> Result<GenerateResponse, ClientError> {
    Ok(GenerateResponse::Refused(RefusedResponse {
        id: None,
        reason: RefusalReason::NoEngineConfigured,
        reason_code: None,
        retryable: false,
        blocking: true,
        reset_at_ms: None,
        provider: None,
        detail: "no engine".to_owned(),
    }))
}

fn boundaries(times: &[&str]) -> Value {
    json!({
        "segments": times.iter().enumerate().map(|(index, start_at)| {
            json!({"start_at": start_at, "line": index + 1})
        }).collect::<Vec<_>>()
    })
}

fn wrapper(entries: Value, topics: &str, setting: &str) -> Value {
    json!({"entries": entries, "topics": topics, "setting": setting})
}

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("t.txt");
    fs::write(&source, "one\ntwo\nthree").unwrap();
    let day = temporary.path().join("chronicle/20260311");
    (temporary, source, day)
}

fn run(
    path: &Path,
    day: &Path,
    wire: &RecordingWire,
    audio_duration: Option<u64>,
) -> Result<Vec<PathBuf>, TextImportError> {
    process_transcript_with_wire(
        path,
        day,
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        audio_duration,
        wire,
    )
}

fn rows(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_stream_generation(day_dir: &Path, expected: u64) {
    let journal = day_dir.parent().unwrap().parent().unwrap();
    let day = day_dir.file_name().unwrap().to_str().unwrap();
    assert!(matches!(
        read_health_marker(journal, day, HealthMarkerKind::Stream).unwrap(),
        HealthMarkerState::Versioned { marker, .. } if marker.generation == expected
    ));
}

fn oracle_case(name: &str) -> Value {
    let oracle: Value = serde_json::from_str(include_str!("fixtures/text-oracles.json")).unwrap();
    oracle["cases"][name].clone()
}

fn assert_created_matches_oracle(created: &[PathBuf], expected: &Value) {
    let expected_created = expected["created"].as_array().unwrap();
    assert_eq!(created.len(), expected_created.len());
    for (path, expected) in created.iter().zip(expected_created) {
        assert_eq!(
            path.parent().unwrap().file_name().unwrap(),
            expected["segment_dir"].as_str().unwrap()
        );
        assert_eq!(
            path.file_name().unwrap(),
            expected["file"].as_str().unwrap()
        );
        assert_eq!(rows(path), expected["rows"].as_array().unwrap().clone());
    }
}

fn standard_responses(entry_times: &[&str]) -> Vec<Result<GenerateResponse, ClientError>> {
    vec![
        generated(boundaries(&["12:00:00", "12:00:30", "12:05:00"])),
        generated(wrapper(
            json!([{"start": entry_times[0], "text": "first"}, {"start": entry_times[1], "text": "second"}]),
            "t1",
            "kitchen",
        )),
        generated(wrapper(
            json!([{"start": entry_times[2], "text": "third"}]),
            "",
            "",
        )),
        generated(wrapper(
            json!([{"start": entry_times[3], "text": "fourth"}]),
            "t3",
            "",
        )),
    ]
}

#[test]
fn ac1_gap_derived_durations_no_audio_duration_matches_oracle() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(standard_responses(&[
        "12:00:00", "12:00:12", "12:00:45", "12:05:01",
    ]));
    let created = run(&source, &day, &wire, None).unwrap();
    assert_created_matches_oracle(
        &created,
        &oracle_case("gap_derived_durations_no_audio_duration"),
    );
}

#[test]
fn ac1_last_segment_uses_audio_duration_matches_oracle() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(standard_responses(&[
        "12:00:00", "12:00:00", "12:00:30", "12:05:00",
    ]));
    let created = run(&source, &day, &wire, Some(600)).unwrap();
    assert_created_matches_oracle(&created, &oracle_case("last_segment_uses_audio_duration"));
}

#[test]
fn ac1_out_of_order_raises_after_prior_write() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00", "12:05:00", "12:00:00"])),
        generated(wrapper(
            json!([{"start": "12:00:00", "text": "first"}]),
            "",
            "",
        )),
        generated(wrapper(
            json!([{"start": "12:05:00", "text": "second"}]),
            "",
            "",
        )),
    ]);
    let error = run(&source, &day, &wire, None).unwrap_err();
    let expected = oracle_case("out_of_order_raises");
    assert_eq!(error.to_string(), expected["raised"]["message"]);
    assert!(
        day.join("import.text/120000_300/conversation_transcript.jsonl")
            .exists()
    );
    assert_stream_generation(&day, 1);
}

#[test]
fn ac1_audio_duration_too_short_raises_after_prior_writes() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(standard_responses(&[
        "12:00:00", "12:00:00", "12:00:30", "12:05:00",
    ]));
    let error = run(&source, &day, &wire, Some(10)).unwrap_err();
    let expected = oracle_case("audio_duration_too_short_raises");
    assert_eq!(error.to_string(), expected["raised"]["message"]);
    assert!(
        day.join("import.text/120000_30/conversation_transcript.jsonl")
            .exists()
    );
    assert!(
        day.join("import.text/120030_270/conversation_transcript.jsonl")
            .exists()
    );
    assert_stream_generation(&day, 2);
}

#[test]
fn stream_marker_advances_before_a_later_segment_wire_failure() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00", "12:00:30"])),
        generated(wrapper(
            json!([{"start": "12:00:00", "text": "first"}]),
            "",
            "",
        )),
        Err(ClientError::Io("second conversion failed".to_owned())),
    ]);

    let error = run(&source, &day, &wire, None).unwrap_err();

    assert!(matches!(
        error,
        TextImportError::Wire {
            phase: TextWirePhase::SegmentJson,
            ..
        }
    ));
    assert!(
        day.join("import.text/120000_30/conversation_transcript.jsonl")
            .is_file()
    );
    assert_stream_generation(&day, 1);
    assert!(
        !day.parent()
            .unwrap()
            .join("20260312/health/stream.updated")
            .exists()
    );
}

#[test]
fn marker_failure_is_typed_and_retains_the_written_segment() {
    let (_temporary, source, day) = setup();
    let marker = day.join("health/stream.updated");
    fs::create_dir_all(&marker).unwrap();
    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00"])),
        generated(wrapper(
            json!([{"start": "12:00:00", "text": "first"}]),
            "",
            "",
        )),
    ]);

    let error = run(&source, &day, &wire, None).unwrap_err();

    assert!(matches!(
        &error,
        TextImportError::StreamMarker {
            path,
            day: failed_day,
            ..
        } if path == &marker && failed_day == "20260311"
    ));
    assert!(error.to_string().contains("remains written"));
    assert!(
        day.join("import.text/120000_5/conversation_transcript.jsonl")
            .is_file()
    );
}

#[test]
fn ac2_falsy_wrapper_is_skipped_matches_oracle() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00", "12:00:30", "12:05:00"])),
        generated(wrapper(json!([{"start": "12:00:00", "text": "x"}]), "", "")),
        refused(),
        generated(wrapper(json!([{"start": "12:05:00", "text": "z"}]), "", "")),
    ]);
    let created = run(&source, &day, &wire, None).unwrap();
    assert_created_matches_oracle(&created, &oracle_case("falsy_wrapper_is_skipped"));
}

#[test]
fn ac3_relativizes_entries_and_clamps_zero_duration_to_one_second() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00", "12:00:00"])),
        generated(wrapper(
            json!([
                {"start": "11:59:59", "text": "before"},
                {"start": "not-a-time", "text": "unchanged"},
                {"text": "missing-start"},
                {"start": "12:00:00"}
            ]),
            "",
            "",
        )),
        generated(wrapper(
            json!([{"start": "12:00:00", "text": "last"}]),
            "",
            "",
        )),
    ]);
    let created = run(&source, &day, &wire, None).unwrap();
    assert_eq!(
        created[0].parent().unwrap().file_name().unwrap(),
        "120000_1"
    );
    assert_eq!(
        rows(&created[0])[1],
        json!({"start": "00:00:00", "text": "before", "source": "import"})
    );
    assert_eq!(
        rows(&created[0])[2],
        json!({"start": "not-a-time", "text": "unchanged", "source": "import"})
    );
    assert_eq!(
        rows(&created[0])[3],
        json!({"text": "missing-start", "source": "import"})
    );
    assert_eq!(rows(&created[0])[4], json!({"start": "00:00:00"}));
}

#[test]
fn ac4_header_keeps_caller_and_model_setting_slots_distinct() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00"])),
        generated(wrapper(
            json!([{"start": "12:00:00", "text": "hello"}]),
            "planning",
            "office",
        )),
    ]);
    let created = process_transcript_with_wire(
        &source,
        &day,
        "12:00:00",
        "id",
        "import.text",
        Some("work"),
        Some("caller-setting"),
        None,
        &wire,
    )
    .unwrap();
    assert_eq!(
        rows(&created[0])[0],
        json!({
            "imported": {"id": "id", "facet": "work", "setting": "caller-setting"},
            "raw": "../../../imports/id/t.txt",
            "topics": "planning",
            "setting": "office"
        })
    );
}

#[test]
fn ac5_raw_back_reference_is_destination_independent() {
    let (temporary, source, day) = setup();
    let other_day = temporary.path().join("elsewhere/day");
    for (day_dir, stream, import_id) in [
        (&day, "stream_a", "first"),
        (&other_day, "stream_b", "second"),
    ] {
        let wire = RecordingWire::new(vec![
            generated(boundaries(&["12:00:00"])),
            generated(wrapper(json!([{"start": "12:00:00", "text": "x"}]), "", "")),
        ]);
        let created = process_transcript_with_wire(
            &source, day_dir, "12:00:00", import_id, stream, None, None, None, &wire,
        )
        .unwrap();
        assert_eq!(
            rows(&created[0])[0]["raw"],
            format!("../../../imports/{import_id}/t.txt")
        );
        let staged = temporary
            .path()
            .join("imports")
            .join(import_id)
            .join("t.txt");
        assert_eq!(
            fs::read_to_string(&staged).unwrap(),
            fs::read_to_string(&source).unwrap(),
            "raw pointer must resolve to a copy of the source"
        );
    }
}

#[test]
fn ac7_recording_wire_receives_the_two_generate_request_shapes() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00"])),
        generated(wrapper(json!([{"start": "12:00:00", "text": "x"}]), "", "")),
    ]);
    run(&source, &day, &wire, None).unwrap();
    let requests = wire.requests.borrow();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].context, "observe.detect.segment");
    assert_eq!(
        requests[0].system_instruction.as_deref(),
        Some(include_str!(
            "../src/text_assets/detect_transcript_segment.md"
        ))
    );
    assert_eq!(requests[0].temperature, 0.3);
    assert_eq!(requests[0].max_output_tokens, 4096);
    assert_eq!(requests[0].thinking_budget, Some(8192));
    assert!(requests[0].json_output);
    assert!(
        requests[0]
            .json_schema
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|schema| !schema.is_empty())
    );
    assert_eq!(requests[1].context, "observe.detect.json");
    assert_eq!(
        requests[1].system_instruction.as_deref(),
        Some(include_str!("../src/text_assets/detect_transcript_json.md"))
    );
    assert_eq!(requests[1].temperature, 0.3);
    assert_eq!(requests[1].max_output_tokens, 8192);
    assert_eq!(requests[1].thinking_budget, Some(8192));
    assert!(requests[1].json_output);
    assert!(
        requests[1]
            .json_schema
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|schema| !schema.is_empty())
    );
}

#[test]
fn stamp_half_is_not_a_transcript_clock() {
    let (_temporary, source, day) = setup();
    let wire = RecordingWire::new(vec![refused(), refused()]);
    let error = process_transcript_with_wire(
        &source,
        &day,
        "062652",
        "20260818_062652",
        "import.text",
        None,
        None,
        None,
        &wire,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        TextImportError::InvalidTime { value } if value == "062652"
    ));
}

#[test]
fn ac8_unsupported_extension_is_rejected() {
    let (temporary, _source, day) = setup();
    let source = temporary.path().join("t.pdf");
    fs::write(&source, "nope").unwrap();
    let wire = RecordingWire::new(Vec::new());
    assert!(matches!(
        run(&source, &day, &wire, None),
        Err(TextImportError::UnsupportedFormat { .. })
    ));
}

#[test]
fn boundary_refusal_is_named_and_wire_failures_propagate_by_phase() {
    let (_temporary, source, day) = setup();
    // A model refusal used to abort with SegmentationUnavailable. That left an owner
    // with no path to get text into the journal when generate had no engine. The
    // native fallback writes the whole file as one segment instead of claiming
    // success over an empty import (the empty-success case this test originally
    // closed). Wire IO failures still propagate by phase.
    let boundary_refusal = RecordingWire::new(vec![refused(), refused()]);
    let created = run(&source, &day, &boundary_refusal, None).expect("fallback writes");
    assert_eq!(created.len(), 1);
    let written = rows(&created[0]);
    assert_eq!(written[1]["text"], "one\ntwo\nthree");
    assert_eq!(written[1]["start"], "00:00:00");

    let boundary_failure = RecordingWire::new(vec![Err(ClientError::Io("down".to_owned()))]);
    assert!(matches!(
        run(&source, &day, &boundary_failure, None),
        Err(TextImportError::Wire {
            phase: TextWirePhase::SegmentBoundary,
            ..
        })
    ));

    let conversion_failure = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00"])),
        Err(ClientError::Io("down".to_owned())),
    ]);
    assert!(matches!(
        run(&source, &day, &conversion_failure, None),
        Err(TextImportError::Wire {
            phase: TextWirePhase::SegmentJson,
            ..
        })
    ));
}

#[test]
fn collisions_choose_and_report_a_different_segment_key() {
    let (_temporary, source, day) = setup();
    fs::create_dir_all(day.join("import.text/120000_5")).unwrap();
    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00"])),
        generated(wrapper(json!([{"start": "12:00:00", "text": "x"}]), "", "")),
    ]);
    let created = run(&source, &day, &wire, None).unwrap();
    assert_ne!(
        created[0].parent().unwrap().file_name().unwrap(),
        "120000_5"
    );
    assert!(created[0].exists());
}
