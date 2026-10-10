// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse, RefusalReason,
    RefusedResponse,
};
use solstone_core_import::{
    TextCreated, TextImportError, TextImportOutcome, TextImportWork, WireClient,
    process_transcript_with_wire,
};
use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};
use solstone_core_segment::{ImportSource, Kind, StreamHints};

/// A synthetic one-hour meeting in the v1 layout: a date and title heading, relative
/// `## HH:MM:SS` headings that run past an hour, and `**Full Name:** text` turns.
const MEETING: &str = "# 2026-03-11\n# Planning sync\n\n## 00:00:00\n**Ana Lima:** Morning, everyone. Let's start with the roadmap.\n**Ben Okafor:** Sounds good.\n\n## 00:04:59\n**Ben Okafor:** The import work is first.\n\n## 00:05:00\n**Ana Lima:** Agreed — times come from the file.\n\n## 00:31:15\n**Chidi Eze:** I have one question, about the  spacing.\nIt carries onto a second line.\n\n## 01:02:03\n**Ana Lima:** Thanks, all.\n";

/// Each turn of [`MEETING`] at its offset from the start, with its speaker and words.
const MEETING_TURNS: &[(u64, &str, &str)] = &[
    (
        0,
        "Ana Lima",
        "Morning, everyone. Let's start with the roadmap.",
    ),
    (0, "Ben Okafor", "Sounds good."),
    (299, "Ben Okafor", "The import work is first."),
    (300, "Ana Lima", "Agreed — times come from the file."),
    (
        1875,
        "Chidi Eze",
        "I have one question, about the  spacing.\nIt carries onto a second line.",
    ),
    (3723, "Ana Lima", "Thanks, all."),
];

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

    /// A wire that answers every request with the same response.
    fn always(response: fn() -> Result<GenerateResponse, ClientError>) -> Self {
        Self::new((0..64).map(|_| response()).collect())
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

fn refusal(blocking: bool) -> Result<GenerateResponse, ClientError> {
    Ok(GenerateResponse::Refused(RefusedResponse {
        id: None,
        reason: RefusalReason::NoEngineConfigured,
        reason_code: None,
        retryable: false,
        blocking,
        reset_at_ms: None,
        provider: None,
        detail: "no engine".to_owned(),
    }))
}

fn no_engine() -> Result<GenerateResponse, ClientError> {
    refusal(true)
}

fn wire_down() -> Result<GenerateResponse, ClientError> {
    Err(ClientError::Io {
        primary: "down".to_owned(),
        cleanup: None,
    })
}

/// A model that answers with times and rewritten words, which must never reach the journal.
fn meddling() -> Result<GenerateResponse, ClientError> {
    generated(json!({
        "topics": "roadmap, imports",
        "setting": "workplace",
        "entries": [{"start": "12:09:00", "speaker": "Someone", "text": "rewritten"}],
        "start": "12:09:00"
    }))
}

fn context(topics: &str, setting: &str) -> Result<GenerateResponse, ClientError> {
    generated(json!({"topics": topics, "setting": setting}))
}

fn setup(name: &str, text: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join(name);
    fs::write(&source, text).unwrap();
    let day = temporary.path().join("journal/chronicle/20260311");
    (temporary, source, day)
}

fn run(path: &Path, day: &Path, start: &str, wire: &dyn WireClient) -> TextImportOutcome {
    process_transcript_with_wire(
        path,
        day,
        start,
        "20260311_120000",
        "import.text",
        None,
        None,
        wire,
    )
}

fn run_ok(path: &Path, day: &Path, start: &str, wire: &dyn WireClient) -> Vec<TextCreated> {
    run_work(path, day, start, wire).created
}

fn run_work(path: &Path, day: &Path, start: &str, wire: &dyn WireClient) -> TextImportWork {
    match run(path, day, start, wire) {
        TextImportOutcome::Success(work) => work,
        TextImportOutcome::Failed { error, .. } => panic!("expected success, got error: {error:?}"),
    }
}

fn rows(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn seconds(clock: &str) -> u64 {
    let parts: Vec<u64> = clock.split(':').map(|part| part.parse().unwrap()).collect();
    parts[0] * 3600 + parts[1] * 60 + parts[2]
}

/// Every entry the import wrote, as (day, absolute seconds after that day's midnight,
/// speaker, text), with the segment it sits in.
fn placed(created: &[TextCreated]) -> Vec<(String, String, u64, Option<String>, String)> {
    let mut placed = Vec::new();
    for item in created {
        let (clock, length) = item.segment.split_once('_').unwrap();
        assert_eq!(
            length, "300",
            "every segment is an ordinary ~300-second one"
        );
        let segment_start = seconds(&format!(
            "{}:{}:{}",
            &clock[0..2],
            &clock[2..4],
            &clock[4..6]
        ));
        for row in rows(&item.path).into_iter().skip(1) {
            let start = seconds(row["start"].as_str().unwrap());
            assert!(start < 300, "an entry sits inside its own segment");
            assert_eq!(row["source"], "import");
            placed.push((
                item.day.clone(),
                item.segment.clone(),
                segment_start + start,
                row.get("speaker")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                row["text"].as_str().unwrap().to_owned(),
            ));
        }
    }
    placed
}

fn assert_stream_generation(day_dir: &Path, expected: u64) {
    let journal = day_dir.parent().unwrap().parent().unwrap();
    let day = day_dir.file_name().unwrap().to_str().unwrap();
    assert!(matches!(
        read_health_marker(journal, day, HealthMarkerKind::Stream).unwrap(),
        HealthMarkerState::Versioned { marker, .. } if marker.generation == expected
    ));
}

fn assert_meeting_placed_from_the_file(created: &[TextCreated]) {
    let placed = placed(created);
    let start = 12 * 3600;
    // The date and title headings stay in the import, at the start.
    assert_eq!(
        placed[0],
        (
            "20260311".to_owned(),
            "120000_300".to_owned(),
            start,
            None,
            "# 2026-03-11\n# Planning sync".to_owned()
        )
    );
    let turns = &placed[1..];
    assert_eq!(turns.len(), MEETING_TURNS.len());
    for ((day, segment, at, speaker, text), (offset, label, words)) in
        turns.iter().zip(MEETING_TURNS)
    {
        assert_eq!(day, "20260311");
        assert_eq!(*at, start + offset, "{label} at {offset}s");
        assert_eq!(
            *segment,
            format!("{}_300", {
                let tile = start + offset / 300 * 300;
                format!("{:02}{:02}{:02}", tile / 3600, tile % 3600 / 60, tile % 60)
            }),
            "tiled from the start"
        );
        assert_eq!(speaker.as_deref(), Some(*label));
        assert_eq!(text, words);
        assert!(MEETING.contains(words), "words are the file's bytes");
    }
    let segments: Vec<_> = created.iter().map(|item| item.segment.as_str()).collect();
    assert_eq!(
        segments,
        ["120000_300", "120500_300", "123000_300", "130000_300"]
    );
}

#[test]
fn a_v1_transcript_places_every_turn_from_the_file_with_no_model_at_all() {
    for (name, wire) in [
        ("no engine", RecordingWire::always(no_engine)),
        ("wire down", RecordingWire::always(wire_down)),
    ] {
        let (_temporary, source, day) = setup("meeting.md", MEETING);
        let work = run_work(&source, &day, "12:00:00", &wire);
        assert!(!work.untimed, "{name}");
        let created = work.created;
        assert_meeting_placed_from_the_file(&created);
        for item in &created {
            let header = &rows(&item.path)[0];
            assert!(header.get("topics").is_none(), "{name}");
            assert!(header.get("setting").is_none(), "{name}");
            assert!(header.get("untimed").is_none(), "{name}");
        }
        assert_eq!(
            wire.requests.borrow().len(),
            1,
            "{name}: an unavailable model is not asked again"
        );
        assert_stream_generation(&day, 4);
    }
}

#[test]
fn a_model_adds_topics_and_setting_and_nothing_else() {
    let (_temporary, source, day) = setup("meeting.md", MEETING);
    let wire = RecordingWire::always(meddling);
    let created = run_ok(&source, &day, "12:00:00", &wire);
    assert_meeting_placed_from_the_file(&created);
    for item in &created {
        let header = &rows(&item.path)[0];
        assert_eq!(header["topics"], "roadmap, imports");
        assert_eq!(header["setting"], "workplace");
    }
    let requests = wire.requests.borrow();
    assert_eq!(requests.len(), 4, "one request per segment");
    for request in requests.iter() {
        assert_eq!(request.context, "observe.detect.topics");
        assert_eq!(
            request.system_instruction.as_deref(),
            Some(include_str!(
                "../src/text_assets/detect_transcript_topics.md"
            ))
        );
        assert_eq!(request.max_output_tokens, 256);
        assert!(request.json_output);
    }
    let ContentPart::Text { text } = &requests[1].contents[0] else {
        panic!("text request")
    };
    assert_eq!(
        text, "Ana Lima: Agreed — times come from the file.\n",
        "a segment's request carries only that segment's turns"
    );
}

#[test]
fn a_refused_or_unreadable_reply_leaves_out_only_that_segments_topics() {
    let (_temporary, source, day) = setup("meeting.md", MEETING);
    let wire = RecordingWire::new(vec![
        context("roadmap", "workplace"),
        refusal(false),
        generated(json!("not an object")),
        context("", "workplace"),
    ]);
    let created = run_ok(&source, &day, "12:00:00", &wire);
    assert_meeting_placed_from_the_file(&created);
    let headers: Vec<Value> = created
        .iter()
        .map(|item| rows(&item.path)[0].clone())
        .collect();
    assert_eq!(headers[0]["topics"], "roadmap");
    assert!(headers[1].get("topics").is_none());
    assert!(headers[2].get("topics").is_none());
    assert!(headers[3].get("topics").is_none());
    assert_eq!(headers[3]["setting"], "workplace");
    assert_eq!(wire.requests.borrow().len(), 4);
}

#[test]
fn a_meeting_that_crosses_midnight_continues_into_the_next_day() {
    let (_temporary, source, day) = setup("late.txt", MEETING);
    let created = run_ok(&source, &day, "23:58:00", &RecordingWire::always(no_engine));
    let placed: Vec<_> = placed(&created)
        .into_iter()
        .map(|(day, segment, at, _, _)| (day, segment, at))
        .collect();
    assert_eq!(
        placed,
        [
            ("20260311".to_owned(), "235800_300".to_owned(), 86_280),
            ("20260311".to_owned(), "235800_300".to_owned(), 86_280),
            ("20260311".to_owned(), "235800_300".to_owned(), 86_280),
            ("20260311".to_owned(), "235800_300".to_owned(), 86_579),
            ("20260312".to_owned(), "000300_300".to_owned(), 180),
            ("20260312".to_owned(), "002800_300".to_owned(), 1_755),
            ("20260312".to_owned(), "005800_300".to_owned(), 3_603),
        ]
    );
    let next = day.parent().unwrap().join("20260312");
    assert!(
        next.join("import.text/000300_300/conversation_transcript.jsonl")
            .is_file()
    );
    assert_stream_generation(&day, 1);
    assert_stream_generation(&next, 3);
}

#[test]
fn a_bracketed_transcript_counts_its_times_from_the_start_with_no_model() {
    let file = "[00:00:00] Ana Lima: Let's begin.\n[00:04:10] Ben Okafor: The file keeps  my words.\n[00:06:02] Ana Lima: Good.\n";
    let (_temporary, source, day) = setup("bracketed.txt", file);
    let created = run_ok(&source, &day, "09:00:00", &RecordingWire::always(no_engine));
    let placed: Vec<_> = placed(&created)
        .into_iter()
        .map(|(_, segment, at, speaker, text)| (segment, at, speaker.unwrap(), text))
        .collect();
    let at = |offset: u64| 9 * 3600 + offset;
    assert_eq!(
        placed,
        [
            (
                "090000_300".to_owned(),
                at(0),
                "Ana Lima".to_owned(),
                "Let's begin.".to_owned()
            ),
            (
                "090000_300".to_owned(),
                at(250),
                "Ben Okafor".to_owned(),
                "The file keeps  my words.".to_owned()
            ),
            (
                "090500_300".to_owned(),
                at(362),
                "Ana Lima".to_owned(),
                "Good.".to_owned()
            ),
        ]
    );
}

#[test]
fn a_granola_transcript_with_clock_times_lands_at_those_times_with_no_model() {
    let file = "Weekly sync\nAna Lima (14:30:05)\nWe should start.\nIt carries on.\nBen Okafor (14:36:00)\nAgreed.\n";
    let (_temporary, source, day) = setup("granola.md", file);
    // The start the import was given is not the file's: the file's clock times win.
    let created = run_ok(&source, &day, "09:00:00", &RecordingWire::always(no_engine));
    let placed = placed(&created);
    let at = |h: u64, m: u64, s: u64| h * 3600 + m * 60 + s;
    assert_eq!(
        placed,
        [
            (
                "20260311".to_owned(),
                "143005_300".to_owned(),
                at(14, 30, 5),
                None,
                "Weekly sync".to_owned()
            ),
            (
                "20260311".to_owned(),
                "143005_300".to_owned(),
                at(14, 30, 5),
                Some("Ana Lima".to_owned()),
                "We should start.\nIt carries on.".to_owned()
            ),
            (
                "20260311".to_owned(),
                "143505_300".to_owned(),
                at(14, 36, 0),
                Some("Ben Okafor".to_owned()),
                "Agreed.".to_owned()
            ),
        ]
    );
}

#[test]
fn an_untimed_transcript_is_one_segment_at_its_start_with_no_time_invented() {
    let untimed = "Weekly sync notes\n\nAna: Morning, everyone.\nBen: Morning.\n\n[00:12] a time in a layout this does not read\n## 00:00:05\nnobody speaks under this heading\n";
    let (_temporary, source, day) = setup("notes.txt", untimed);
    let wire = RecordingWire::always(meddling);
    let work = run_work(&source, &day, "09:15:30", &wire);
    assert!(work.untimed);
    let created = work.created;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].day, "20260311");
    assert_eq!(created[0].segment, "091530_300", "no duration is estimated");
    let written = rows(&created[0].path);
    let lines: Vec<&str> = untimed.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(written.len(), lines.len() + 1);
    for (row, line) in written[1..].iter().zip(lines) {
        assert_eq!(
            row,
            &json!({"start": "00:00:00", "text": line, "source": "import"}),
            "every line, as written, at the start"
        );
    }
    assert_eq!(written[0]["topics"], "roadmap, imports");
    assert_eq!(
        written[0]["untimed"], true,
        "the header says no entry's time was read from the file"
    );
    assert_eq!(wire.requests.borrow().len(), 1);
}

#[test]
fn an_unrecognized_transcript_stays_untimed_with_no_model_at_all() {
    let (_temporary, source, day) = setup("t.txt", "one\ntwo\nthree");
    let created = run_ok(&source, &day, "12:00:00", &RecordingWire::always(wire_down));
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].segment, "120000_300");
    assert_eq!(rows(&created[0].path)[0]["untimed"], true);
    let texts: Vec<_> = rows(&created[0].path)[1..]
        .iter()
        .map(|row| (row["start"].clone(), row["text"].clone()))
        .collect();
    assert_eq!(
        texts,
        [
            (json!("00:00:00"), json!("one")),
            (json!("00:00:00"), json!("two")),
            (json!("00:00:00"), json!("three"))
        ]
    );
}

#[test]
fn an_empty_transcript_writes_nothing() {
    let (_temporary, source, day) = setup("t.txt", "\n  \n");
    let wire = RecordingWire::new(Vec::new());
    let work = run_work(&source, &day, "12:00:00", &wire);
    assert!(work.created.is_empty());
    assert!(
        !work.untimed,
        "nothing landed, so nothing is called untimed"
    );
}

#[test]
fn header_keeps_caller_and_model_setting_slots_distinct() {
    let (_temporary, source, day) = setup("t.txt", "hello");
    let wire = RecordingWire::new(vec![context("planning", "office")]);
    let outcome = process_transcript_with_wire(
        &source,
        &day,
        "12:00:00",
        "id",
        "import.text",
        Some("work"),
        Some("caller-setting"),
        &wire,
    );
    let created = outcome.created();
    assert_eq!(created.len(), 1);
    assert_eq!(
        rows(&created[0].path)[0],
        json!({
            "imported": {"id": "id", "facet": "work", "setting": "caller-setting"},
            "raw": "../../../imports/id/t.txt",
            "untimed": true,
            "topics": "planning",
            "setting": "office"
        })
    );
}

#[test]
fn raw_back_reference_is_destination_independent() {
    let (temporary, source, day) = setup("t.txt", "one\ntwo\nthree");
    let other_day = temporary.path().join("elsewhere/day");
    let journal = day.parent().unwrap().parent().unwrap().to_path_buf();
    for (day_dir, stream, import_id, root) in [
        (&day, "stream_a", "first", journal.as_path()),
        (&other_day, "stream_b", "second", temporary.path()),
    ] {
        let wire = RecordingWire::always(no_engine);
        let outcome = process_transcript_with_wire(
            &source, day_dir, "12:00:00", import_id, stream, None, None, &wire,
        );
        let created = outcome.created();
        assert_eq!(created.len(), 1);
        assert_eq!(
            rows(&created[0].path)[0]["raw"],
            format!("../../../imports/{import_id}/t.txt")
        );
        let staged = root.join("imports").join(import_id).join("t.txt");
        assert_eq!(
            fs::read_to_string(&staged).unwrap(),
            fs::read_to_string(&source).unwrap(),
            "raw pointer must resolve to a copy of the source"
        );
    }
}

#[test]
fn stamp_half_is_not_a_transcript_clock() {
    let (_temporary, source, day) = setup("t.txt", "one");
    let outcome = run(&source, &day, "062652", &RecordingWire::new(Vec::new()));
    match outcome {
        TextImportOutcome::Failed { created, error } => {
            assert!(created.created.is_empty());
            assert!(matches!(
                error,
                TextImportError::InvalidTime { value } if value == "062652"
            ));
        }
        TextImportOutcome::Success(_) => panic!("expected failure"),
    }
}

#[test]
fn unsupported_extension_is_rejected() {
    let (_temporary, source, day) = setup("t.pdf", "nope");
    let outcome = run(&source, &day, "12:00:00", &RecordingWire::new(Vec::new()));
    match outcome {
        TextImportOutcome::Failed { created, error } => {
            assert!(created.created.is_empty());
            assert!(matches!(error, TextImportError::UnsupportedFormat { .. }));
        }
        TextImportOutcome::Success(_) => panic!("expected failure"),
    }
}

#[test]
fn marker_failure_is_typed_and_retains_the_written_segment() {
    let (_temporary, source, day) = setup("t.txt", "one");
    let marker = day.join("health/stream.updated");
    fs::create_dir_all(&marker).unwrap();
    let outcome = run(&source, &day, "12:00:00", &RecordingWire::always(no_engine));
    let TextImportOutcome::Failed { created, error } = outcome else {
        panic!("expected failure");
    };
    assert_eq!(created.created.len(), 1);
    assert_eq!(created.created[0].segment, "120000_300");
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
        day.join("import.text/120000_300/conversation_transcript.jsonl")
            .is_file()
    );
}

#[test]
fn collisions_choose_and_report_a_different_segment_key() {
    let (_temporary, source, day) = setup("t.txt", "one");
    fs::create_dir_all(day.join("import.text/120000_300")).unwrap();
    let created = run_ok(&source, &day, "12:00:00", &RecordingWire::always(no_engine));
    assert_ne!(created[0].segment, "120000_300");
    assert!(created[0].path.exists());
}

#[test]
fn text_created_identity_carries_complete_metadata() {
    let (_temporary, source, day) = setup("t.txt", "hello");
    let created = run_ok(&source, &day, "12:00:00", &RecordingWire::always(no_engine));
    assert_eq!(created.len(), 1);
    let item = &created[0];
    assert_eq!(item.day, "20260311");
    assert_eq!(item.segment, "120000_300");
    assert_eq!(item.stream, "import.text");
    assert_eq!(
        item.hints,
        StreamHints {
            kind: Some(Kind::Imported(ImportSource::Named("text".to_owned()))),
            host: None,
            platform: None,
        }
    );
    assert_eq!(item.created_segment().day, "20260311");
    assert_eq!(item.created_segment().segment, "120000_300");
}
