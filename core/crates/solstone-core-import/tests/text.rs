// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_format::content::{Family, OccurrenceTimeMs, produce_chunks};
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
        None,
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

#[cfg(feature = "full-tests")]
#[test]
fn text_publication_keeps_inference_outside_segment_and_refuses_deleted_or_replaced_parent() {
    struct ChangingWire {
        segment: PathBuf,
        mutation: &'static str,
    }
    impl WireClient for ChangingWire {
        fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            let options = solstone_core_journal_io::LockOptions {
                timeout: std::time::Duration::ZERO,
                ..Default::default()
            };
            assert!(
                solstone_core_journal_io::hold_lock(
                    self.segment.join("conversation_transcript.jsonl"),
                    options
                )
                .is_err()
            );
            assert!(solstone_core_journal_io::hold_lock(&self.segment, options).is_ok());
            match self.mutation {
                "deleted" => fs::write(self.segment.join("tombstone.json"), b"{}").unwrap(),
                "replaced" => {
                    fs::rename(&self.segment, self.segment.with_file_name("previous")).unwrap();
                    fs::create_dir(&self.segment).unwrap();
                }
                _ => (),
            }
            no_engine()
        }
    }
    for mutation in ["unchanged", "deleted", "replaced"] {
        let (_temporary, source, day) = setup("notes.txt", "synthetic transcript");
        let segment = day.join("import.text/120000_300");
        let outcome = run(
            &source,
            &day,
            "12:00:00",
            &ChangingWire {
                segment: segment.clone(),
                mutation,
            },
        );
        if mutation == "unchanged" {
            assert!(matches!(outcome, TextImportOutcome::Success(_)));
            assert!(segment.join("conversation_transcript.jsonl").exists());
        } else {
            assert!(matches!(outcome, TextImportOutcome::Failed { .. }));
            assert!(!segment.join("conversation_transcript.jsonl").exists());
        }
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
            ("20260312".to_owned(), "000259_300".to_owned(), 179),
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
    assert_stream_generation(&next, 4);
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
        assert_eq!(row["start"], "00:00:00");
        assert_eq!(row["text"], line);
        assert_eq!(row["source"], "import");
        assert!(row["source_ms"].is_i64());
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
        None,
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
            &source, day_dir, "12:00:00", import_id, stream, None, None, &wire, None,
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

fn setup_in_zone(
    name: &str,
    text: &str,
    day_str: &str,
    zone: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("journal");
    fs::create_dir_all(journal.join("config")).unwrap();
    fs::write(
        journal.join("config/journal.json"),
        json!({"identity": {"timezone": zone}}).to_string(),
    )
    .unwrap();
    let source = temporary.path().join(name);
    fs::write(&source, text).unwrap();
    let day = journal.join("chronicle").join(day_str);
    fs::create_dir_all(&day).unwrap();
    (temporary, source, day)
}

fn has_any_transcript(root: &Path) -> bool {
    if !root.exists() {
        return false;
    }
    if root.is_file() {
        return root.file_name().and_then(|n| n.to_str()) == Some("conversation_transcript.jsonl");
    }
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if has_any_transcript(&entry.path()) {
                return true;
            }
        }
    }
    false
}

#[cfg(feature = "full-tests")]
fn setup_day(name: &str, text: &str, day_str: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join(name);
    fs::write(&source, text).unwrap();
    let day = temporary.path().join("journal/chronicle").join(day_str);
    fs::create_dir_all(&day).unwrap();
    (temporary, source, day)
}

#[test]
fn spring_tile_keeps_real_instants_through_the_reader() {
    let text =
        "## 00:00:00\n**Ana:** First\n## 00:03:00\n**Ben:** Second\n## 00:05:00\n**Ana:** Third\n";
    let (_temporary, source, day) = setup_in_zone("spring.txt", text, "20260308", "America/Denver");
    let preview_size =
        solstone_core_import::text::preview(&source, &day, "01:58:00", "import.text", None)
            .unwrap();
    let created = run_ok(&source, &day, "01:58:00", &RecordingWire::always(no_engine));
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].day, "20260308");
    assert_eq!(created[0].segment, "015800_300");
    assert_eq!(created[1].day, "20260308");
    assert_eq!(created[1].segment, "030300_300");

    let preview_keys: BTreeSet<(&str, &str)> = preview_size
        .recordings
        .iter()
        .map(|r| (r.day.as_str(), r.segment.as_str()))
        .collect();
    let created_keys: BTreeSet<(&str, &str)> = created
        .iter()
        .map(|c| (c.day.as_str(), c.segment.as_str()))
        .collect();
    assert_eq!(preview_keys, created_keys);

    let rows0 = rows(&created[0].path);
    assert_eq!(
        rows0[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );
    assert_eq!(
        rows0[2].get("start").and_then(Value::as_str),
        Some("00:03:00")
    );

    let rows1 = rows(&created[1].path);
    assert_eq!(
        rows1[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );

    let expected_ms0 = chrono::NaiveDate::from_ymd_opt(2026, 3, 8)
        .unwrap()
        .and_hms_opt(8, 58, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    let text0 = fs::read_to_string(&created[0].path).unwrap();
    let rel0 = format!(
        "20260308/import.text/{}/conversation_transcript.jsonl",
        created[0].segment
    );
    let produced0 = produce_chunks(Family::AiChat, &rel0, &text0);
    assert_eq!(
        produced0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0)),
    );
    assert_eq!(
        produced0.chunks[1].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0 + 180 * 1000)),
    );

    let text1 = fs::read_to_string(&created[1].path).unwrap();
    let rel1 = format!(
        "20260308/import.text/{}/conversation_transcript.jsonl",
        created[1].segment
    );
    let produced1 = produce_chunks(Family::AiChat, &rel1, &text1);
    assert_eq!(
        produced1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0 + 300 * 1000)),
    );
}

#[test]
fn fall_back_tile_keeps_both_one_fifty_eight_instants() {
    let text = "## 00:00:00\n**Ana:** First\n## 00:05:00\n**Ben:** Second\n## 01:00:00\n**Ana:** Third\n## 01:05:00\n**Ben:** Fourth\n";
    let (_temporary, source, day) = setup_in_zone("fall.txt", text, "20261101", "America/Denver");
    let preview_size =
        solstone_core_import::text::preview(&source, &day, "01:58:00", "import.text", None)
            .unwrap();
    let created = run_ok(&source, &day, "01:58:00", &RecordingWire::always(no_engine));
    assert_eq!(created.len(), 4);
    assert_eq!(created[0].day, "20261101");
    assert_eq!(created[0].segment, "015800_300");
    assert_eq!(created[1].day, "20261101");
    assert_eq!(created[1].segment, "010300_300");
    assert_eq!(created[2].day, "20261101");
    assert_eq!(created[2].segment, "015801_300");
    assert_eq!(created[3].day, "20261101");
    assert_eq!(created[3].segment, "020300_300");

    let preview_keys: BTreeSet<(&str, &str)> = preview_size
        .recordings
        .iter()
        .map(|r| (r.day.as_str(), r.segment.as_str()))
        .collect();
    let created_keys: BTreeSet<(&str, &str)> = created
        .iter()
        .map(|c| (c.day.as_str(), c.segment.as_str()))
        .collect();
    assert_eq!(preview_keys, created_keys);

    let rows0 = rows(&created[0].path);
    assert_eq!(
        rows0[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );
    let rows1 = rows(&created[1].path);
    assert_eq!(
        rows1[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );
    let rows2 = rows(&created[2].path);
    assert_eq!(
        rows2[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );
    let rows3 = rows(&created[3].path);
    assert_eq!(
        rows3[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );

    let expected_ms0 = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
        .unwrap()
        .and_hms_opt(7, 58, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();

    let text0 = fs::read_to_string(&created[0].path).unwrap();
    let rel0 = format!(
        "20261101/import.text/{}/conversation_transcript.jsonl",
        created[0].segment
    );
    let produced0 = produce_chunks(Family::AiChat, &rel0, &text0);
    assert_eq!(
        produced0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0)),
    );

    let text1 = fs::read_to_string(&created[1].path).unwrap();
    let rel1 = format!(
        "20261101/import.text/{}/conversation_transcript.jsonl",
        created[1].segment
    );
    let produced1 = produce_chunks(Family::AiChat, &rel1, &text1);
    assert_eq!(
        produced1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0 + 300 * 1000)),
    );

    let text2 = fs::read_to_string(&created[2].path).unwrap();
    let rel2 = format!(
        "20261101/import.text/{}/conversation_transcript.jsonl",
        created[2].segment
    );
    let produced2 = produce_chunks(Family::AiChat, &rel2, &text2);
    assert_eq!(
        produced2.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0 + 3600 * 1000)),
    );

    let text3 = fs::read_to_string(&created[3].path).unwrap();
    let rel3 = format!(
        "20261101/import.text/{}/conversation_transcript.jsonl",
        created[3].segment
    );
    let produced3 = produce_chunks(Family::AiChat, &rel3, &text3);
    assert_eq!(
        produced3.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0 + 3900 * 1000)),
    );
}

#[test]
fn ordinary_day_clocks_stay_on_the_wall_time() {
    let text =
        "## 00:00:00\n**Ana:** First\n## 00:03:00\n**Ben:** Second\n## 00:05:00\n**Ana:** Third\n";
    let (_temporary, source, day) =
        setup_in_zone("ordinary.txt", text, "20260311", "America/Denver");
    let preview_size =
        solstone_core_import::text::preview(&source, &day, "01:58:00", "import.text", None)
            .unwrap();
    let created = run_ok(&source, &day, "01:58:00", &RecordingWire::always(no_engine));
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].day, "20260311");
    assert_eq!(created[0].segment, "015800_300");
    assert_eq!(created[1].day, "20260311");
    assert_eq!(created[1].segment, "020300_300");

    let preview_keys: BTreeSet<(&str, &str)> = preview_size
        .recordings
        .iter()
        .map(|r| (r.day.as_str(), r.segment.as_str()))
        .collect();
    let created_keys: BTreeSet<(&str, &str)> = created
        .iter()
        .map(|c| (c.day.as_str(), c.segment.as_str()))
        .collect();
    assert_eq!(preview_keys, created_keys);

    let rows0 = rows(&created[0].path);
    assert_eq!(
        rows0[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );
    assert_eq!(
        rows0[2].get("start").and_then(Value::as_str),
        Some("00:03:00")
    );
    let rows1 = rows(&created[1].path);
    assert_eq!(
        rows1[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );

    let expected_ms0 = chrono::NaiveDate::from_ymd_opt(2026, 3, 11)
        .unwrap()
        .and_hms_opt(7, 58, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    let text0 = fs::read_to_string(&created[0].path).unwrap();
    let rel0 = format!(
        "20260311/import.text/{}/conversation_transcript.jsonl",
        created[0].segment
    );
    let produced0 = produce_chunks(Family::AiChat, &rel0, &text0);
    assert_eq!(
        produced0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0)),
    );
    assert_eq!(
        produced0.chunks[1].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0 + 180 * 1000)),
    );

    let text1 = fs::read_to_string(&created[1].path).unwrap();
    let rel1 = format!(
        "20260311/import.text/{}/conversation_transcript.jsonl",
        created[1].segment
    );
    let produced1 = produce_chunks(Family::AiChat, &rel1, &text1);
    assert_eq!(
        produced1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0 + 300 * 1000)),
    );
}

#[test]
fn kolkata_midnight_rolls_to_the_next_local_day() {
    let text = "# 2026-03-11\n## 00:00:00\n**Ana:** late\n## 00:05:00\n**Ben:** next day\n";
    let (_temporary, source, day) = setup_in_zone("kolkata.txt", text, "20260311", "Asia/Kolkata");
    let preview_size =
        solstone_core_import::text::preview(&source, &day, "23:58:00", "import.text", None)
            .unwrap();
    let created = run_ok(&source, &day, "23:58:00", &RecordingWire::always(no_engine));
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].day, "20260311");
    assert_eq!(created[0].segment, "235800_300");
    assert_eq!(created[1].day, "20260312");
    assert_eq!(created[1].segment, "000300_300");

    let preview_keys: BTreeSet<(&str, &str)> = preview_size
        .recordings
        .iter()
        .map(|r| (r.day.as_str(), r.segment.as_str()))
        .collect();
    let created_keys: BTreeSet<(&str, &str)> = created
        .iter()
        .map(|c| (c.day.as_str(), c.segment.as_str()))
        .collect();
    assert_eq!(preview_keys, created_keys);

    let text0 = fs::read_to_string(&created[0].path).unwrap();
    let rel0 = format!(
        "20260311/import.text/{}/conversation_transcript.jsonl",
        created[0].segment
    );
    let produced0 = produce_chunks(Family::AiChat, &rel0, &text0);
    let expected0 = chrono::NaiveDate::from_ymd_opt(2026, 3, 11)
        .unwrap()
        .and_hms_opt(18, 28, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(
        produced0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected0)),
    );

    let text1 = fs::read_to_string(&created[1].path).unwrap();
    let rel1 = format!(
        "20260312/import.text/{}/conversation_transcript.jsonl",
        created[1].segment
    );
    let produced1 = produce_chunks(Family::AiChat, &rel1, &text1);
    assert_eq!(
        produced1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected0 + 300_000)),
    );
}

#[test]
fn missing_spring_start_writes_no_chronicle_segment() {
    let text = "# 2026-03-08\n## 00:00:00\n**Ana:** hello\n";
    let (temporary, source, day) = setup_in_zone("missing.txt", text, "20260308", "America/Denver");
    let preview_err =
        solstone_core_import::text::preview(&source, &day, "02:30:00", "import.text", None)
            .unwrap_err();
    match preview_err {
        TextImportError::LocalTimeMissing {
            day: failed_day,
            time,
        } => {
            assert_eq!(failed_day, "20260308");
            assert_eq!(time, "02:30:00");
        }
        other => panic!("expected LocalTimeMissing from preview, got {other:?}"),
    }

    let wire = RecordingWire::always(no_engine);
    let outcome = process_transcript_with_wire(
        &source,
        &day,
        "02:30:00",
        "20260308_023000",
        "import.text",
        None,
        None,
        &wire,
        None,
    );
    match outcome {
        TextImportOutcome::Failed {
            error:
                TextImportError::LocalTimeMissing {
                    day: failed_day,
                    time,
                },
            created,
        } => {
            assert_eq!(failed_day, "20260308");
            assert_eq!(time, "02:30:00");
            assert!(created.created.is_empty());
        }
        other => panic!("expected LocalTimeMissing, got {other:?}"),
    }
    let chronicle = temporary.path().join("journal/chronicle");
    assert!(
        !has_any_transcript(&chronicle),
        "no conversation_transcript.jsonl anywhere under chronicle/"
    );
}

#[test]
fn inline_clocks_cross_midnight_onto_the_next_local_day() {
    let text = "[23:58:00] A: before\n[00:03:00] B: after\n";
    let (_temporary, source, day) =
        setup_in_zone("midnight.txt", text, "20260311", "America/Denver");
    let preview_size =
        solstone_core_import::text::preview(&source, &day, "12:00:00", "import.text", None)
            .unwrap();
    let created = run_ok(&source, &day, "12:00:00", &RecordingWire::always(no_engine));
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].day, "20260311");
    assert_eq!(created[0].segment, "235800_300");
    assert_eq!(created[1].day, "20260312");
    assert_eq!(created[1].segment, "000300_300");

    let preview_keys: BTreeSet<(&str, &str)> = preview_size
        .recordings
        .iter()
        .map(|r| (r.day.as_str(), r.segment.as_str()))
        .collect();
    let created_keys: BTreeSet<(&str, &str)> = created
        .iter()
        .map(|c| (c.day.as_str(), c.segment.as_str()))
        .collect();
    assert_eq!(preview_keys, created_keys);

    let rows0 = rows(&created[0].path);
    assert!(rows0[0].get("untimed").is_none());
    assert_eq!(
        rows0[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );
    assert_eq!(rows0[1].get("text").and_then(Value::as_str), Some("before"));

    let rows1 = rows(&created[1].path);
    assert!(rows1[0].get("untimed").is_none());
    assert_eq!(
        rows1[1].get("start").and_then(Value::as_str),
        Some("00:00:00")
    );
    assert_eq!(rows1[1].get("text").and_then(Value::as_str), Some("after"));

    let text0 = fs::read_to_string(&created[0].path).unwrap();
    let rel0 = format!(
        "20260311/import.text/{}/conversation_transcript.jsonl",
        created[0].segment
    );
    let produced0 = produce_chunks(Family::AiChat, &rel0, &text0);
    // 2026-03-11 23:58:00 Denver MDT (UTC-6) -> 2026-03-12 05:58:00Z
    let expected0 = chrono::NaiveDate::from_ymd_opt(2026, 3, 12)
        .unwrap()
        .and_hms_opt(5, 58, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(
        produced0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected0)),
    );

    let text1 = fs::read_to_string(&created[1].path).unwrap();
    let rel1 = format!(
        "20260312/import.text/{}/conversation_transcript.jsonl",
        created[1].segment
    );
    let produced1 = produce_chunks(Family::AiChat, &rel1, &text1);
    // 2026-03-12 00:03:00 Denver MDT (UTC-6) -> 2026-03-12 06:03:00Z
    let expected1 = chrono::NaiveDate::from_ymd_opt(2026, 3, 12)
        .unwrap()
        .and_hms_opt(6, 3, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(
        produced1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected1)),
    );
}

#[test]
fn a_single_elapsed_tile_splits_at_the_owner_day_boundary() {
    for (name, text, start) in [
        (
            "clock-midnight.txt",
            "[23:59:00] Alex: before\n[00:01:00] Blair: after\n",
            "12:00:00",
        ),
        (
            "elapsed-midnight.txt",
            "## 00:00:00\n**Alex:** before\n## 00:02:00\n**Blair:** after\n",
            "23:59:00",
        ),
    ] {
        let (_temporary, source, day) = setup_in_zone(name, text, "20260311", "America/Denver");
        let preview =
            solstone_core_import::text::preview(&source, &day, start, "import.text", None).unwrap();
        let created = run_ok(&source, &day, start, &RecordingWire::always(no_engine));
        assert_eq!(created.len(), 2, "{name}");
        assert_eq!(preview.recordings.len(), 2, "{name}");
        for (item, expected_day, expected_text, hour, minute) in [
            (&created[0], "20260311", "before", 5, 59),
            (&created[1], "20260312", "after", 6, 1),
        ] {
            assert_eq!(item.day, expected_day, "{name}");
            assert!(preview.recordings.iter().any(|recording|
                recording.day == item.day && recording.segment == item.segment));
            let bytes = fs::read_to_string(&item.path).unwrap();
            let rel = format!(
                "{expected_day}/import.text/{}/conversation_transcript.jsonl",
                item.segment
            );
            let produced = produce_chunks(Family::AiChat, &rel, &bytes);
            assert_eq!(produced.chunks.len(), 1, "{name}");
            assert!(produced.chunks[0].content.contains(expected_text));
            let expected = chrono::NaiveDate::from_ymd_opt(2026, 3, 12)
                .unwrap()
                .and_hms_opt(hour, minute, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis();
            assert_eq!(
                produced.chunks[0].occurrence_time_ms,
                Some(OccurrenceTimeMs(expected)),
                "{name}"
            );
        }
    }
}

#[test]
fn out_of_day_clocks_do_not_become_a_midnight_crossing() {
    for clock in ["24:00:00", "999:00:00"] {
        let text = format!("[{clock}] Alex: before\n[00:01:00] Blair: after\n");
        let (_temporary, source, day) = setup("invalid-clock.txt", &text);
        let created = run_ok(&source, &day, "12:00:00", &RecordingWire::always(no_engine));
        assert_eq!(created.len(), 1);
        let saved = rows(&created[0].path);
        assert_eq!(saved[0]["untimed"], true);
        assert!(saved.iter().any(|row| {
            row["text"]
                .as_str()
                .is_some_and(|value| value.contains(clock))
        }));
    }
}

#[cfg(feature = "full-tests")]
#[test]
#[allow(unsafe_code)]
fn owner_zone_beats_the_host_zone() {
    struct TzGuard(Option<String>);
    impl Drop for TzGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(val) => unsafe { std::env::set_var("TZ", val) },
                None => unsafe { std::env::remove_var("TZ") },
            }
        }
    }
    let _guard = TzGuard(std::env::var("TZ").ok());
    unsafe { std::env::set_var("TZ", "UTC") };

    // With TZ=UTC and identity.timezone America/Denver:
    let text = "## 00:00:00\n**Ana:** hi\n## 01:00:00\n**Ben:** later\n";
    let (_temp, source, day) = setup_in_zone("t.txt", text, "20261101", "America/Denver");
    let created = run_ok(&source, &day, "01:58:00", &RecordingWire::always(no_engine));
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].segment, "015800_300");
    assert_eq!(created[1].segment, "015801_300");

    let expected_ms0 = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
        .unwrap()
        .and_hms_opt(7, 58, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    let text0 = fs::read_to_string(&created[0].path).unwrap();
    let rel0 = format!(
        "20261101/import.text/{}/conversation_transcript.jsonl",
        created[0].segment
    );
    let produced0 = produce_chunks(Family::AiChat, &rel0, &text0);
    assert_eq!(
        produced0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0)),
    );
    let text1 = fs::read_to_string(&created[1].path).unwrap();
    let rel1 = format!(
        "20261101/import.text/{}/conversation_transcript.jsonl",
        created[1].segment
    );
    let produced1 = produce_chunks(Family::AiChat, &rel1, &text1);
    assert_eq!(
        produced1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_ms0 + 3600 * 1000)),
    );

    // With TZ=UTC and no config/journal.json:
    let (_temp2, source2, day2) = setup_day("t2.txt", text, "20261101");
    let created2 = run_ok(
        &source2,
        &day2,
        "01:58:00",
        &RecordingWire::always(no_engine),
    );
    let text_no_cfg = fs::read_to_string(&created2[0].path).unwrap();
    let rel_no_cfg = format!(
        "20261101/import.text/{}/conversation_transcript.jsonl",
        created2[0].segment
    );
    let produced_no_cfg = produce_chunks(Family::AiChat, &rel_no_cfg, &text_no_cfg);
    let expected_utc = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
        .unwrap()
        .and_hms_opt(1, 58, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(
        produced_no_cfg.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(expected_utc)),
    );

    // Keep the 02:30 LocalTimeMissing case under Denver even when TZ=UTC:
    let (_temp3, source3, day3) = setup_in_zone(
        "t3.txt",
        "# 2026-03-08\n## 00:00:00\n**Ana:** hi\n",
        "20260308",
        "America/Denver",
    );
    let wire = RecordingWire::always(no_engine);
    let outcome = process_transcript_with_wire(
        &source3,
        &day3,
        "02:30:00",
        "20260308_023000",
        "import.text",
        None,
        None,
        &wire,
        None,
    );
    match outcome {
        TextImportOutcome::Failed {
            error: TextImportError::LocalTimeMissing { day, time },
            ..
        } => {
            assert_eq!(day, "20260308");
            assert_eq!(time, "02:30:00");
        }
        other => panic!("expected LocalTimeMissing, got {other:?}"),
    }
}

#[test]
fn limitless_utc_keeps_the_second_repeated_hour_for_elapsed_text() {
    let text = "## 00:00:00\n**Ana:** first\n## 00:05:00\n**Ana:** later\n";
    let (_temp, source, day) = setup_in_zone(
        "limitless_pendant_2026-11-01T08-58-00_to_2026-11-01T09-10-00.txt",
        text,
        "20261101",
        "America/Denver",
    );
    let stamp = solstone_core_import::detect_creation(
        &source,
        source.file_name().and_then(|name| name.to_str()),
        chrono_tz::America::Denver,
    )
    .unwrap();
    assert_eq!(stamp.as_str(), "20261101_015800");
    let preview = solstone_core_import::text::preview(
        &source,
        &day,
        "01:58:00",
        "import.text",
        stamp.source_instant(),
    )
    .unwrap();
    assert_eq!(
        preview
            .recordings
            .iter()
            .map(|row| row.segment.as_str())
            .collect::<Vec<_>>(),
        ["015800_300", "020300_300"]
    );
    let outcome = process_transcript_with_wire(
        &source,
        &day,
        "01:58:00",
        "20261101_015800",
        "import.text",
        None,
        None,
        &RecordingWire::always(no_engine),
        stamp.source_instant(),
    );
    assert!(matches!(outcome, TextImportOutcome::Success(_)));
    assert_eq!(
        outcome
            .created()
            .iter()
            .map(|row| row.segment.as_str())
            .collect::<Vec<_>>(),
        ["015800_300", "020300_300"]
    );
}

struct StoppingTopics {
    stop: solstone_core_import::WorkStop,
    requests: RefCell<usize>,
}

impl WireClient for StoppingTopics {
    fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        let call = self.requests.borrow().saturating_add(1);
        *self.requests.borrow_mut() = call;
        if call == 2 {
            self.stop.request();
        }
        context("roadmap", "workplace")
    }
}

fn files_named(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !root.exists() {
        return found;
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|item| item == name) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn stop_before_the_source_writes_no_raw_transcript_or_marker() {
    let (_temporary, source, day) = setup("meeting.md", MEETING);
    let source_bytes = fs::read(&source).unwrap();
    let stop = solstone_core_import::WorkStop::new();
    stop.request();
    let _guard = solstone_core_import::WorkStopGuard::install(&stop);
    let wire = StoppingTopics {
        stop: stop.clone(),
        requests: RefCell::new(0),
    };
    let outcome = run(&source, &day, "12:00:00", &wire);
    let TextImportOutcome::Failed { created, error } = outcome else {
        panic!("stop before the source is a failed import");
    };
    assert!(created.created.is_empty());
    assert_eq!(error.to_string(), "import stopped");
    assert_eq!(*wire.requests.borrow(), 0);
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    let journal = day.parent().unwrap().parent().unwrap();
    assert!(files_named(journal, "meeting.md").is_empty());
    assert!(files_named(&day, "conversation_transcript.jsonl").is_empty());
    assert!(
        !journal
            .join("imports/20260311_120000/imported.json")
            .exists()
    );
    assert_eq!(
        read_health_marker(journal, "20260311", HealthMarkerKind::Stream).unwrap(),
        HealthMarkerState::Absent
    );
}

#[test]
fn stop_after_the_second_model_keeps_the_first_tile_and_starts_no_later_one() {
    let (_temporary, source, day) = setup("meeting.md", MEETING);
    let source_bytes = fs::read(&source).unwrap();
    let stop = solstone_core_import::WorkStop::new();
    let _guard = solstone_core_import::WorkStopGuard::install(&stop);
    let wire = StoppingTopics {
        stop: stop.clone(),
        requests: RefCell::new(0),
    };
    let outcome = run(&source, &day, "12:00:00", &wire);
    let TextImportOutcome::Failed { created, error } = outcome else {
        panic!("stop during the second tile is a failed import");
    };
    assert_eq!(error.to_string(), "import stopped");
    assert_eq!(created.created.len(), 1);
    assert_eq!(created.created[0].segment, "120000_300");
    assert_eq!(*wire.requests.borrow(), 2);
    assert_eq!(fs::read(&source).unwrap(), source_bytes);

    let journal = day.parent().unwrap().parent().unwrap();
    let raw = journal.join("imports/20260311_120000/meeting.md");
    assert_eq!(fs::read(&raw).unwrap(), source_bytes);
    assert!(
        !journal
            .join("imports/20260311_120000/imported.json")
            .exists()
    );

    let transcripts = files_named(&day, "conversation_transcript.jsonl");
    assert_eq!(transcripts.len(), 1);
    assert_eq!(
        transcripts[0],
        day.join("import.text/120000_300/conversation_transcript.jsonl")
    );
    assert!(day.join("import.text/120500_300").is_dir());
    assert!(
        !day.join("import.text/120500_300/conversation_transcript.jsonl")
            .exists()
    );
    assert!(!day.join("import.text/123000_300").exists());
    assert!(!day.join("import.text/130000_300").exists());
    assert_stream_generation(&day, 1);
}
