// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Map, Value, json};
use solstone_core_generate::{
    ClientError, GenerateRequest, GenerateResponse, GeneratedResponse, RefusalReason,
    RefusedResponse,
};
use solstone_core_import::metadata::{
    AttemptState, IMPORT_FAILED_REASON, admit_running_attempt, read_attempt_facts, read_provenance,
};
use solstone_core_import::projection::project_import_result;
use solstone_core_import::publish::{PublishError, read_publication_record};
use solstone_core_import::text::{
    TextCreated, TextImportError, TextImportOutcome, TextWirePhase, process_transcript_with_wire,
};
use solstone_core_import::{ImportError, ProjectionStatus, RUNNING_ATTEMPT_BOUND_MS, WireClient};
use solstone_core_import_host::cli_argv;
use solstone_core_import_host::text_publication::{
    TextFinish, TextFinishError, TextTerminalInput, TextTerminalSeams, finish_text_attempt,
    finish_text_attempt_with,
};
use solstone_core_segment::{ImportSource, Kind, StreamHints};
use tempfile::TempDir;
use tower::ServiceExt;

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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn stream_record_seq(journal_root: &Path, stream: &str) -> Option<u64> {
    let path = journal_root.join("streams").join(format!("{stream}.json"));
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    value.get("seq").and_then(Value::as_u64)
}

fn stream_record_kind(journal_root: &Path, stream: &str) -> Option<String> {
    let path = journal_root.join("streams").join(format!("{stream}.json"));
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    value
        .get("kind")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn run_cli_test<E, C>(
    journal: &Path,
    args: &[&str],
    lookup_env: E,
    connectivity: C,
) -> solstone_core_import::cli_render::CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    match cli_argv::run_cli_with(
        &args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
        journal,
        lookup_env,
        connectivity,
    ) {
        cli_argv::CliOutcome::Rendered(run) => run,
        cli_argv::CliOutcome::Registry(_) => {
            panic!("test invocation must not reach a registry body")
        }
    }
}

async fn oneshot_list(app: axum::Router) -> Value {
    let response = app
        .oneshot(
            Request::builder()
                .uri("/app/import/api/list")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn oneshot_detail(app: axum::Router, timestamp: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/import/api/{timestamp}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap();
    (status, body)
}

// ---------------------------------------------------------------------------
// AC1: Durable outcome & web routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac1_red_proof_legacy_timeout_triple_on_list_and_detail() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();

    let legacy_id = "20260811_210000";
    let legacy_dir = journal.join("imports").join(legacy_id);
    fs::create_dir_all(&legacy_dir).unwrap();

    let two_hours_ago_ms = now_ms().saturating_sub(7_200_000);

    let mut legacy = Map::new();
    legacy.insert("task_id".to_owned(), json!("1755000000000"));
    legacy.insert("upload_timestamp".to_owned(), json!(two_hours_ago_ms));
    fs::write(
        legacy_dir.join("import.json"),
        serde_json::to_vec(&Value::Object(legacy)).unwrap(),
    )
    .unwrap();

    // 1. Direct projection assert
    let projection = project_import_result(&journal, legacy_id);
    assert_eq!(projection.status, ProjectionStatus::Failed);
    assert_eq!(projection.error.as_deref(), Some("Import never completed"));
    assert_eq!(projection.error_stage.as_deref(), Some("timeout"));

    // 2. Real list route assert
    let app = solstone_core_import_web::routes(journal.clone());
    let list_json = oneshot_list(app).await;
    let imports = list_json.get("imports").and_then(Value::as_array).unwrap();
    let row = imports
        .iter()
        .find(|item| item.get("timestamp").and_then(Value::as_str) == Some(legacy_id))
        .expect("row exists in list");
    assert_eq!(row.get("status").and_then(Value::as_str), Some("failed"));
    assert_eq!(
        row.get("error").and_then(Value::as_str),
        Some("Import never completed")
    );
    assert_eq!(
        row.get("error_stage").and_then(Value::as_str),
        Some("timeout")
    );

    // 3. Real detail route assert
    let app = solstone_core_import_web::routes(journal.clone());
    let (detail_status, detail_json) = oneshot_detail(app, legacy_id).await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(
        detail_json.get("status").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        detail_json.get("error").and_then(Value::as_str),
        Some("Import never completed")
    );
    assert_eq!(
        detail_json.get("error_stage").and_then(Value::as_str),
        Some("timeout")
    );
}

#[tokio::test]
async fn ac1_text_import_n_greater_than_one_success_direct_and_routes() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();

    let note = journal.join("note.md");
    fs::write(&note, "part one\npart two").unwrap();
    let timestamp = "20260818_100000";
    let day_dir = journal.join("chronicle/20260818");
    fs::create_dir_all(&day_dir).unwrap();

    let started_ms = now_ms();
    let generation = admit_running_attempt(&journal, timestamp, started_ms, None)
        .unwrap()
        .generation;

    let wire = RecordingWire::new(vec![
        generated(boundaries(&["10:00:00", "10:05:00"])),
        generated(wrapper(
            json!([{"start": "10:00:00", "text": "part one"}]),
            "topic 1",
            "setting 1",
        )),
        generated(wrapper(
            json!([{"start": "10:05:00", "text": "part two"}]),
            "topic 2",
            "setting 2",
        )),
    ]);

    let outcome = process_transcript_with_wire(
        &note,
        &day_dir,
        "10:00:00",
        timestamp,
        "import.text",
        None,
        None,
        None,
        &wire,
    );

    let finish = finish_text_attempt(
        &journal,
        timestamp,
        generation,
        TextTerminalInput::Success(outcome.created()),
    )
    .unwrap();
    assert_eq!(finish, TextFinish::Applied);

    // 1. Direct projection
    let projection = project_import_result(&journal, timestamp);
    assert_eq!(projection.status, ProjectionStatus::Success);
    assert_eq!(projection.source_type, "text");
    assert_eq!(projection.error, None);
    assert_eq!(projection.error_stage, None);
    assert_eq!(projection.entries_written, Some(2));
    assert_eq!(projection.days_affected, vec!["20260818"]);

    // 2. Real list route
    let app = solstone_core_import_web::routes(journal.clone());
    let list_json = oneshot_list(app).await;
    let imports = list_json.get("imports").and_then(Value::as_array).unwrap();
    let row = imports
        .iter()
        .find(|item| item.get("timestamp").and_then(Value::as_str) == Some(timestamp))
        .expect("row exists in list");
    assert_eq!(row.get("status").and_then(Value::as_str), Some("success"));
    assert_eq!(row.get("source_type").and_then(Value::as_str), Some("text"));
    assert_eq!(row.get("entries_written").and_then(Value::as_u64), Some(2));
    assert!(row.get("error").map_or(true, Value::is_null));

    // 3. Real detail route
    let app = solstone_core_import_web::routes(journal.clone());
    let (detail_status, detail_json) = oneshot_detail(app, timestamp).await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(
        detail_json.get("status").and_then(Value::as_str),
        Some("success")
    );
    assert_eq!(
        detail_json.get("source_type").and_then(Value::as_str),
        Some("text")
    );
    assert_eq!(
        detail_json.get("entries_written").and_then(Value::as_u64),
        Some(2)
    );
    assert!(detail_json.get("error").map_or(true, Value::is_null));

    // 4. imported.json publication record
    let import_dir = journal.join("imports").join(timestamp);
    let pub_rec = read_publication_record(&import_dir)
        .unwrap()
        .expect("publication record exists");
    assert_eq!(
        pub_rec.status,
        solstone_core_import::publish::PublicationStatus::Success
    );
    assert_eq!(pub_rec.segments.len(), 2);
}

#[tokio::test]
async fn ac1_fresh_text_import_success_direct_and_routes() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();

    let note = journal.join("note.md");
    fs::write(&note, "a short note to import").unwrap();
    let timestamp = "20260818_062652";

    let result = run_cli_test(
        &journal,
        &["--timestamp", timestamp, note.to_str().unwrap()],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);

    // 1. Direct projection
    let projection = project_import_result(&journal, timestamp);
    assert_eq!(projection.status, ProjectionStatus::Success);
    assert_eq!(projection.error, None);
    assert_eq!(projection.error_stage, None);
    assert_eq!(projection.entries_written, Some(1));
    assert_eq!(projection.days_affected, vec!["20260818"]);

    // 2. Real list route
    let app = solstone_core_import_web::routes(journal.clone());
    let list_json = oneshot_list(app).await;
    let imports = list_json.get("imports").and_then(Value::as_array).unwrap();
    let row = imports
        .iter()
        .find(|item| item.get("timestamp").and_then(Value::as_str) == Some(timestamp))
        .expect("row exists in list");
    assert_eq!(row.get("status").and_then(Value::as_str), Some("success"));
    assert_eq!(row.get("entries_written").and_then(Value::as_u64), Some(1));
    assert!(row.get("error").map_or(true, Value::is_null));

    // 3. Real detail route
    let app = solstone_core_import_web::routes(journal.clone());
    let (detail_status, detail_json) = oneshot_detail(app, timestamp).await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(
        detail_json.get("status").and_then(Value::as_str),
        Some("success")
    );
    assert_eq!(
        detail_json.get("entries_written").and_then(Value::as_u64),
        Some(1)
    );
    assert!(detail_json.get("error").map_or(true, Value::is_null));

    // 4. import.json attempt record
    let provenance = read_provenance(&journal, timestamp)
        .unwrap()
        .expect("provenance exists");
    let attempt_facts = read_attempt_facts(&provenance);
    let solstone_core_import::metadata::AttemptRead::Present(facts) = attempt_facts else {
        panic!("attempt facts absent or malformed");
    };
    assert_eq!(facts.generation, 1);
    assert_eq!(facts.state, AttemptState::Completed);

    // 5. imported.json publication record
    let import_dir = journal.join("imports").join(timestamp);
    let pub_rec = read_publication_record(&import_dir)
        .unwrap()
        .expect("publication record exists");
    assert_eq!(
        pub_rec.status,
        solstone_core_import::publish::PublicationStatus::Success
    );
    assert_eq!(pub_rec.segments.len(), 1);
}

// ---------------------------------------------------------------------------
// AC2: Zero-segment clean outcome through producer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac2_zero_segment_through_producer_routes_success_zero_events_zero() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260818_120000";
    let day_dir = journal.join("chronicle/20260818");
    fs::create_dir_all(&day_dir).unwrap();

    let started_ms = now_ms();
    let generation = admit_running_attempt(&journal, import_id, started_ms, None)
        .unwrap()
        .generation;

    let note = journal.join("empty.md");
    fs::write(&note, "some text that normalizes to unavailable").unwrap();

    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00"])),
        refused(), // normalizes to Unavailable
    ]);

    let outcome = process_transcript_with_wire(
        &note,
        &day_dir,
        "12:00:00",
        import_id,
        "import.text",
        None,
        None,
        None,
        &wire,
    );
    let TextImportOutcome::Success(work) = &outcome else {
        panic!("expected Success with zero segments, got {:?}", outcome);
    };
    assert!(work.created.is_empty());

    // Bind listener BEFORE finish
    let health_dir = journal.join("health");
    fs::create_dir_all(&health_dir).unwrap();
    let sock_path = health_dir.join("callosum.sock");
    let listener = UnixListener::bind(&sock_path).unwrap();
    listener.set_nonblocking(true).unwrap();

    let finish = finish_text_attempt(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Success(outcome.created()),
    )
    .unwrap();
    assert_eq!(finish, TextFinish::Applied);

    // Direct projection: Success, entries_written: Some(0)
    let projection = project_import_result(&journal, import_id);
    assert_eq!(projection.status, ProjectionStatus::Success);
    assert_eq!(projection.entries_written, Some(0));

    // Real list & detail routes
    let app = solstone_core_import_web::routes(journal.clone());
    let list_json = oneshot_list(app).await;
    let imports = list_json.get("imports").and_then(Value::as_array).unwrap();
    let row = imports
        .iter()
        .find(|item| item.get("timestamp").and_then(Value::as_str) == Some(import_id))
        .expect("row exists");
    assert_eq!(row.get("status").and_then(Value::as_str), Some("success"));
    assert_eq!(row.get("entries_written").and_then(Value::as_u64), Some(0));

    let app = solstone_core_import_web::routes(journal.clone());
    let (detail_status, detail_json) = oneshot_detail(app, import_id).await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(
        detail_json.get("status").and_then(Value::as_str),
        Some("success")
    );
    assert_eq!(
        detail_json.get("entries_written").and_then(Value::as_u64),
        Some(0)
    );

    // Publication record exists with 0 segments
    let pub_rec = read_publication_record(&journal.join("imports").join(import_id))
        .unwrap()
        .expect("pub rec exists");
    assert_eq!(
        pub_rec.status,
        solstone_core_import::publish::PublicationStatus::Success
    );
    assert_eq!(pub_rec.segments.len(), 0);

    // Streams record was not written for 0 segments
    assert!(
        stream_record_seq(&journal, "import.text").is_none(),
        "zero-segment import emits no stream advances"
    );

    // Verify zero events emitted
    let mut events = Vec::new();
    while let Ok((mut stream, _)) = listener.accept() {
        use std::io::Read;
        let mut buf = String::new();
        let _ = stream.read_to_string(&mut buf);
        for line in buf.lines() {
            if let Ok(val) = serde_json::from_str::<Value>(line) {
                events.push(val);
            }
        }
    }
    assert!(
        events.is_empty(),
        "zero-segment import emits zero callosum events: {:?}",
        events
    );
}

// ---------------------------------------------------------------------------
// AC3: Failure handling & error paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac3_1_error_after_n_writes_partial_publication() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260818_120000";
    let day_dir = journal.join("chronicle/20260818");
    fs::create_dir_all(&day_dir).unwrap();

    let started_ms = now_ms();
    let generation = admit_running_attempt(&journal, import_id, started_ms, None)
        .unwrap()
        .generation;

    let note = journal.join("note.md");
    fs::write(&note, "part one\npart two").unwrap();

    // Wire succeeds on boundary and segment 1, fails on segment 2
    let wire = RecordingWire::new(vec![
        generated(boundaries(&["12:00:00", "12:05:00"])),
        generated(wrapper(
            json!([{"start": "12:00:00", "text": "part one"}]),
            "topic 1",
            "setting 1",
        )),
        Err(ClientError::Io {
            primary: "process died".to_owned(),
            cleanup: None,
        }),
    ]);

    let outcome = process_transcript_with_wire(
        &note,
        &day_dir,
        "12:00:00",
        import_id,
        "import.text",
        None,
        None,
        None,
        &wire,
    );

    let TextImportOutcome::Failed { created, error } = &outcome else {
        panic!("expected Failed outcome, got {:?}", outcome);
    };
    assert_eq!(created.created.len(), 1);
    assert!(matches!(
        error,
        TextImportError::Wire {
            phase: TextWirePhase::SegmentJson,
            ..
        }
    ));

    let finish = finish_text_attempt(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Failed(outcome.created()),
    )
    .unwrap();
    assert_eq!(finish, TextFinish::Applied);

    // Publication record exists with the 1 partial segment
    let pub_rec = read_publication_record(&journal.join("imports").join(import_id))
        .unwrap()
        .expect("pub rec exists");
    assert_eq!(
        pub_rec.status,
        solstone_core_import::publish::PublicationStatus::Success
    );
    assert_eq!(pub_rec.segments.len(), 1);

    // import.json attempt has failure reason
    let provenance = read_provenance(&journal, import_id).unwrap().unwrap();
    let solstone_core_import::metadata::AttemptRead::Present(facts) =
        read_attempt_facts(&provenance)
    else {
        panic!("attempt facts absent");
    };
    assert_eq!(facts.state, AttemptState::Unconfirmed);
    assert_eq!(facts.failure_reason.as_deref(), Some(IMPORT_FAILED_REASON));

    // Projection status is Failed / execution / entries_written = 1
    let projection = project_import_result(&journal, import_id);
    assert_eq!(projection.status, ProjectionStatus::Failed);
    assert_eq!(projection.error.as_deref(), Some("import failed"));
    assert_eq!(projection.error_stage.as_deref(), Some("execution"));
    assert_eq!(projection.entries_written, Some(1));

    // Both real routes
    let app = solstone_core_import_web::routes(journal.clone());
    let list_json = oneshot_list(app).await;
    let imports = list_json.get("imports").and_then(Value::as_array).unwrap();
    let row = imports
        .iter()
        .find(|item| item.get("timestamp").and_then(Value::as_str) == Some(import_id))
        .expect("row exists");
    assert_eq!(row.get("status").and_then(Value::as_str), Some("failed"));
    assert_eq!(
        row.get("error").and_then(Value::as_str),
        Some("import failed")
    );
    assert_eq!(
        row.get("error_stage").and_then(Value::as_str),
        Some("execution")
    );
    assert_eq!(row.get("entries_written").and_then(Value::as_u64), Some(1));

    let app = solstone_core_import_web::routes(journal.clone());
    let (detail_status, detail_json) = oneshot_detail(app, import_id).await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(
        detail_json.get("status").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        detail_json.get("error").and_then(Value::as_str),
        Some("import failed")
    );
    assert_eq!(
        detail_json.get("error_stage").and_then(Value::as_str),
        Some("execution")
    );
    assert_eq!(
        detail_json.get("entries_written").and_then(Value::as_u64),
        Some(1)
    );
}

#[tokio::test]
async fn ac3_2_marker_failure_identities_published() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260818_130000";
    let day_dir = journal.join("chronicle/20260818");
    fs::create_dir_all(&day_dir).unwrap();

    let started_ms = now_ms();
    let generation = admit_running_attempt(&journal, import_id, started_ms, None)
        .unwrap()
        .generation;

    let note = journal.join("note.md");
    fs::write(&note, "marker failure test note").unwrap();

    // Plant a file at health/stream.updated marker directory to trigger marker failure in producer
    let marker_path = day_dir.join("health/stream.updated");
    fs::create_dir_all(marker_path.parent().unwrap()).unwrap();
    // Create stream.updated as a directory with read-only permissions or invalid to cause failure
    fs::create_dir(&marker_path).unwrap();

    let wire = RecordingWire::new(vec![
        generated(boundaries(&["13:00:00"])),
        generated(wrapper(
            json!([{"start": "13:00:00", "text": "marker note"}]),
            "topic",
            "setting",
        )),
    ]);

    let outcome = process_transcript_with_wire(
        &note,
        &day_dir,
        "13:00:00",
        import_id,
        "import.text",
        None,
        None,
        None,
        &wire,
    );

    let TextImportOutcome::Failed { created, error } = &outcome else {
        panic!("expected marker failure outcome, got {:?}", outcome);
    };
    assert_eq!(created.created.len(), 1);
    assert!(matches!(error, TextImportError::StreamMarker { .. }));

    // Remove the blocker directory so finish can write publication and health events cleanly
    fs::remove_dir(&marker_path).unwrap();

    let finish = finish_text_attempt(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Failed(outcome.created()),
    )
    .unwrap();
    assert_eq!(finish, TextFinish::Applied);

    let pub_rec = read_publication_record(&journal.join("imports").join(import_id))
        .unwrap()
        .expect("pub rec exists");
    assert_eq!(
        pub_rec.status,
        solstone_core_import::publish::PublicationStatus::Success
    );
    assert_eq!(pub_rec.segments.len(), 1);

    let projection = project_import_result(&journal, import_id);
    assert_eq!(projection.status, ProjectionStatus::Failed);
    assert_eq!(projection.error.as_deref(), Some("import failed"));
    assert_eq!(projection.error_stage.as_deref(), Some("execution"));
    assert_eq!(projection.entries_written, Some(1));
}

#[tokio::test]
async fn ac3_3_error_before_write_failed_empty_no_publication() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260818_140000";
    let day_dir = journal.join("chronicle/20260818");
    fs::create_dir_all(&day_dir).unwrap();

    let started_ms = now_ms();
    let generation = admit_running_attempt(&journal, import_id, started_ms, None)
        .unwrap()
        .generation;

    let note = journal.join("note.md");
    fs::write(&note, "failed note").unwrap();

    // Boundary detection fails immediately
    let wire = RecordingWire::new(vec![Err(ClientError::Io {
        primary: "server down".to_owned(),
        cleanup: None,
    })]);

    let outcome = process_transcript_with_wire(
        &note,
        &day_dir,
        "14:00:00",
        import_id,
        "import.text",
        None,
        None,
        None,
        &wire,
    );

    let TextImportOutcome::Failed { created, .. } = &outcome else {
        panic!("expected Failed outcome, got {:?}", outcome);
    };
    assert!(created.created.is_empty());

    let finish = finish_text_attempt(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Failed(outcome.created()),
    )
    .unwrap();
    assert_eq!(finish, TextFinish::Applied);

    // No imported.json publication record
    let pub_rec = read_publication_record(&journal.join("imports").join(import_id)).unwrap();
    assert!(pub_rec.is_none());

    // Both routes report failed / execution
    let app = solstone_core_import_web::routes(journal.clone());
    let list_json = oneshot_list(app).await;
    let imports = list_json.get("imports").and_then(Value::as_array).unwrap();
    let row = imports
        .iter()
        .find(|item| item.get("timestamp").and_then(Value::as_str) == Some(import_id))
        .expect("row exists");
    assert_eq!(row.get("status").and_then(Value::as_str), Some("failed"));
    assert_eq!(
        row.get("error").and_then(Value::as_str),
        Some("import failed")
    );
    assert_eq!(
        row.get("error_stage").and_then(Value::as_str),
        Some("execution")
    );

    let app = solstone_core_import_web::routes(journal.clone());
    let (detail_status, detail_json) = oneshot_detail(app, import_id).await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(
        detail_json.get("status").and_then(Value::as_str),
        Some("failed")
    );
}

#[test]
fn ac3_4_chronicle_day_is_file_refusal_before_write() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let note = journal.join("note.md");
    fs::write(&note, "note content").unwrap();
    let timestamp = "20260818_120000";

    // Create chronicle/<day> as a regular FILE so create_dir_all fails
    let day_file = journal.join("chronicle/20260818");
    fs::create_dir_all(day_file.parent().unwrap()).unwrap();
    fs::write(&day_file, "blocking file").unwrap();

    let result = run_cli_test(
        &journal,
        &["--timestamp", timestamp, note.to_str().unwrap()],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 1);

    // Attempt was finalized as Unconfirmed with IMPORT_FAILED_REASON
    let provenance = read_provenance(&journal, timestamp).unwrap().unwrap();
    let solstone_core_import::metadata::AttemptRead::Present(facts) =
        read_attempt_facts(&provenance)
    else {
        panic!("attempt facts absent");
    };
    assert_eq!(facts.state, AttemptState::Unconfirmed);
    assert_eq!(facts.failure_reason.as_deref(), Some(IMPORT_FAILED_REASON));

    // No imported.json and no stream record
    let pub_rec = read_publication_record(&journal.join("imports").join(timestamp)).unwrap();
    assert!(pub_rec.is_none());
    assert!(stream_record_seq(&journal, "import.text").is_none());
}

// ---------------------------------------------------------------------------
// AC4: Seams and failure injections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac4_1_publication_write_fail_leaves_unconfirmed_on_routes() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260811_120000";

    let started_ms = now_ms();
    let generation = admit_running_attempt(&journal, import_id, started_ms, None)
        .unwrap()
        .generation;

    let failing_publish =
        |_: solstone_core_import::publish::PublicationInput<'_>,
         _: &dyn solstone_core_import::publish::PublicationOperations| {
            Err(PublishError::RecordRead {
                path: PathBuf::from("/nonexistent"),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "injected publish fail"),
            })
        };

    let seams_pub_err = TextTerminalSeams {
        hold_lock_fn: None,
        publish_fn: Some(&failing_publish),
        record_completed_fn: None,
        record_unconfirmed_fn: None,
    };

    let result_pub_err = finish_text_attempt_with(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Success(&[]),
        seams_pub_err,
    )
    .unwrap();
    assert_eq!(result_pub_err, TextFinish::Applied);

    let provenance = read_provenance(&journal, import_id).unwrap().unwrap();
    let solstone_core_import::metadata::AttemptRead::Present(facts) =
        read_attempt_facts(&provenance)
    else {
        panic!("attempt facts absent");
    };
    assert_eq!(facts.state, AttemptState::Unconfirmed);
    assert_eq!(facts.failure_reason, None);

    // Real list & detail routes report unconfirmed (never success or completed)
    let app = solstone_core_import_web::routes(journal.clone());
    let list_json = oneshot_list(app).await;
    let imports = list_json.get("imports").and_then(Value::as_array).unwrap();
    let row = imports
        .iter()
        .find(|item| item.get("timestamp").and_then(Value::as_str) == Some(import_id))
        .expect("row exists");
    assert_eq!(
        row.get("status").and_then(Value::as_str),
        Some("unconfirmed")
    );

    let app = solstone_core_import_web::routes(journal.clone());
    let (detail_status, detail_json) = oneshot_detail(app, import_id).await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(
        detail_json.get("status").and_then(Value::as_str),
        Some("unconfirmed")
    );
}

#[tokio::test]
async fn ac4_2_lock_fail_returns_lock_error() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260812_120000";

    let started_ms = now_ms();
    let generation = admit_running_attempt(&journal, import_id, started_ms, None)
        .unwrap()
        .generation;

    let failing_lock = |_: &Path, _: &str| {
        Err(ImportError::LockFailed {
            path: PathBuf::from("/nonexistent"),
            message: "injected lock fail".to_owned(),
        })
    };

    let seams = TextTerminalSeams {
        hold_lock_fn: Some(&failing_lock),
        publish_fn: None,
        record_completed_fn: None,
        record_unconfirmed_fn: None,
    };

    let result = finish_text_attempt_with(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Success(&[]),
        seams,
    );
    assert!(matches!(result, Err(TextFinishError::Lock)));

    // imported.json was not created
    let pub_rec = read_publication_record(&journal.join("imports").join(import_id)).unwrap();
    assert!(pub_rec.is_none());

    // Attempt remains Running
    let provenance = read_provenance(&journal, import_id).unwrap().unwrap();
    let solstone_core_import::metadata::AttemptRead::Present(facts) =
        read_attempt_facts(&provenance)
    else {
        panic!("attempt facts absent");
    };
    assert_eq!(facts.state, AttemptState::Running);
}

#[tokio::test]
async fn ac4_3_attempt_write_fail_returns_attempt_write_error() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260813_120000";

    let started_ms = now_ms();
    let generation = admit_running_attempt(&journal, import_id, started_ms, None)
        .unwrap()
        .generation;

    let failing_record = |_: &Path, _: &str, _: u64, _: u64, _: Option<u64>, _: Option<String>| {
        Err(ImportError::SourceMissing {
            path: PathBuf::from("/nonexistent"),
        })
    };

    let seams = TextTerminalSeams {
        hold_lock_fn: None,
        publish_fn: None,
        record_completed_fn: Some(&failing_record),
        record_unconfirmed_fn: None,
    };

    let result = finish_text_attempt_with(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Success(&[]),
        seams,
    );
    assert!(matches!(result, Err(TextFinishError::AttemptWrite)));
}

// ---------------------------------------------------------------------------
// AC5: Generational superseding and provenance corruption
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac5_1_superseded_generation_touches_nothing_while_live_publishes() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();

    // 1. Control: live generation publishes and completes
    let live_id = "20260818_150000";
    let live_gen = admit_running_attempt(&journal, live_id, now_ms(), None)
        .unwrap()
        .generation;

    let segment_path =
        journal.join("chronicle/20260818/import.text/150000_30/conversation_transcript.jsonl");
    fs::create_dir_all(segment_path.parent().unwrap()).unwrap();
    fs::write(&segment_path, "{}\n").unwrap();

    let live_created = TextCreated {
        day: "20260818".to_owned(),
        segment: "150000_30".to_owned(),
        stream: "import.text".to_owned(),
        path: segment_path,
        hints: StreamHints {
            kind: Some(Kind::Imported(ImportSource::Named("text".to_owned()))),
            host: None,
            platform: None,
        },
    };

    finish_text_attempt(
        &journal,
        live_id,
        live_gen,
        TextTerminalInput::Success(&[live_created]),
    )
    .unwrap();

    assert!(
        read_publication_record(&journal.join("imports").join(live_id))
            .unwrap()
            .is_some(),
        "control: live generation writes publication record"
    );
    assert_eq!(
        stream_record_seq(&journal, "import.text"),
        Some(1),
        "control: live generation advances stream"
    );

    // 2. Superseded: generation 1 finishes after generation 2 admitted
    let stale_id = "20260818_160000";
    let stale_gen = admit_running_attempt(&journal, stale_id, now_ms(), None)
        .unwrap()
        .generation;
    let successor_gen = admit_running_attempt(&journal, stale_id, now_ms(), None)
        .unwrap()
        .generation;
    assert!(successor_gen > stale_gen);

    let seq_before = stream_record_seq(&journal, "import.text");
    let metadata_before = read_provenance(&journal, stale_id).unwrap().unwrap();

    let finish_stale = finish_text_attempt(
        &journal,
        stale_id,
        stale_gen,
        TextTerminalInput::Success(&[]),
    )
    .unwrap();
    assert_eq!(finish_stale, TextFinish::Stale);

    assert!(
        read_publication_record(&journal.join("imports").join(stale_id))
            .unwrap()
            .is_none(),
        "superseded generation must not write publication record"
    );
    assert_eq!(
        stream_record_seq(&journal, "import.text"),
        seq_before,
        "superseded generation must not advance stream record"
    );
    let metadata_after = read_provenance(&journal, stale_id).unwrap().unwrap();
    assert_eq!(
        metadata_before, metadata_after,
        "superseded generation must not rewrite import.json"
    );

    let solstone_core_import::metadata::AttemptRead::Present(facts) =
        read_attempt_facts(&metadata_after)
    else {
        panic!("attempt facts absent");
    };
    assert_eq!(facts.generation, successor_gen);
    assert_eq!(facts.state, AttemptState::Running);
}

#[tokio::test]
async fn ac5_2_unreadable_provenance_returns_error_and_no_publication() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260818_170000";

    let generation = admit_running_attempt(&journal, import_id, now_ms(), None)
        .unwrap()
        .generation;

    // Corrupt import.json with non-JSON bytes
    let import_dir = journal.join("imports").join(import_id);
    fs::write(import_dir.join("import.json"), b"invalid non-json bytes {").unwrap();

    let err = finish_text_attempt(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Success(&[]),
    );
    assert!(matches!(err, Err(TextFinishError::ProvenanceUnreadable)));

    let pub_rec = read_publication_record(&import_dir).unwrap();
    assert!(pub_rec.is_none());
}

#[test]
fn ac5_3_absent_and_malformed_attempt_facts_return_typed_errors() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();

    // 1. Missing import dir
    let err_missing =
        finish_text_attempt(&journal, "nonexistent", 1, TextTerminalInput::Success(&[]));
    assert!(matches!(err_missing, Err(TextFinishError::AttemptAbsent)));
    assert!(
        read_publication_record(&journal.join("imports/nonexistent"))
            .unwrap()
            .is_none()
    );

    // 2. import.json without attempt field
    let no_attempt_id = "20260818_180000";
    let no_attempt_dir = journal.join("imports").join(no_attempt_id);
    fs::create_dir_all(&no_attempt_dir).unwrap();
    fs::write(no_attempt_dir.join("import.json"), b"{}").unwrap();
    let err_no_attempt =
        finish_text_attempt(&journal, no_attempt_id, 1, TextTerminalInput::Success(&[]));
    assert!(matches!(
        err_no_attempt,
        Err(TextFinishError::AttemptAbsent)
    ));
    assert!(read_publication_record(&no_attempt_dir).unwrap().is_none());

    // 3. import.json with malformed attempt field
    let malformed_id = "20260818_190000";
    let malformed_dir = journal.join("imports").join(malformed_id);
    fs::create_dir_all(&malformed_dir).unwrap();
    fs::write(
        malformed_dir.join("import.json"),
        b"{\"attempt\": \"malformed\"}",
    )
    .unwrap();
    let err_malformed =
        finish_text_attempt(&journal, malformed_id, 1, TextTerminalInput::Success(&[]));
    assert!(matches!(
        err_malformed,
        Err(TextFinishError::AttemptMalformed)
    ));
    assert!(read_publication_record(&malformed_dir).unwrap().is_none());
}

// ---------------------------------------------------------------------------
// AC6: Callosum events & stream record sequence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac6_callosum_exact_events_and_stream_record_on_text_import() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let timestamp = "20260818_200000";
    let day_dir = journal.join("chronicle/20260818");
    fs::create_dir_all(&day_dir).unwrap();

    let started_ms = now_ms();
    let generation = admit_running_attempt(&journal, timestamp, started_ms, None)
        .unwrap()
        .generation;

    let note = journal.join("note.md");
    fs::write(&note, "part 1\npart 2").unwrap();

    let wire = RecordingWire::new(vec![
        generated(boundaries(&["20:00:00", "20:05:00"])),
        generated(wrapper(
            json!([{"start": "20:00:00", "text": "part 1"}]),
            "topic 1",
            "setting 1",
        )),
        generated(wrapper(
            json!([{"start": "20:05:00", "text": "part 2"}]),
            "topic 2",
            "setting 2",
        )),
    ]);

    let outcome = process_transcript_with_wire(
        &note,
        &day_dir,
        "20:00:00",
        timestamp,
        "import.text",
        None,
        None,
        None,
        &wire,
    );
    assert_eq!(outcome.created().len(), 2);

    // Bind UnixListener BEFORE finish
    let health_dir = journal.join("health");
    fs::create_dir_all(&health_dir).unwrap();
    let sock_path = health_dir.join("callosum.sock");
    let listener = UnixListener::bind(&sock_path).unwrap();
    listener.set_nonblocking(true).unwrap();

    let finish = finish_text_attempt(
        &journal,
        timestamp,
        generation,
        TextTerminalInput::Success(outcome.created()),
    )
    .unwrap();
    assert_eq!(finish, TextFinish::Applied);

    // Stream record verification
    assert_eq!(
        stream_record_kind(&journal, "import.text").as_deref(),
        Some("import")
    );
    assert_eq!(stream_record_seq(&journal, "import.text"), Some(2));

    // Event capture and exact verification
    let mut events = Vec::new();
    while let Ok((mut stream, _)) = listener.accept() {
        use std::io::Read;
        let mut buf = String::new();
        let _ = stream.read_to_string(&mut buf);
        for line in buf.lines() {
            if let Ok(val) = serde_json::from_str::<Value>(line) {
                events.push(val);
            }
        }
    }

    let observed_events: Vec<&Value> = events
        .iter()
        .filter(|e| {
            e.get("tract").and_then(Value::as_str) == Some("observe")
                && e.get("event").and_then(Value::as_str) == Some("observed")
        })
        .collect();
    assert_eq!(
        observed_events.len(),
        2,
        "exact 2 observed events matching 2 segments"
    );

    let drain_events: Vec<&Value> = events
        .iter()
        .filter(|e| {
            e.get("tract").and_then(Value::as_str) == Some("supervisor")
                && e.get("event").and_then(Value::as_str) == Some("drain")
        })
        .collect();
    assert_eq!(drain_events.len(), 1, "exact 1 supervisor drain event");
    assert_eq!(
        drain_events[0].get("day").and_then(Value::as_str),
        Some("20260818")
    );

    let enrichment_events: Vec<&Value> = events
        .iter()
        .filter(|e| {
            e.get("tract").and_then(Value::as_str) == Some("importer")
                && e.get("event").and_then(Value::as_str) == Some("enrichment_ready")
        })
        .collect();
    assert_eq!(enrichment_events.len(), 1, "exact 1 enrichment ready event");
    assert_eq!(
        enrichment_events[0]
            .get("import_id")
            .and_then(Value::as_str),
        Some(timestamp)
    );
    assert_eq!(
        enrichment_events[0].get("importer").and_then(Value::as_str),
        Some("text")
    );
    assert_eq!(
        enrichment_events[0]
            .get("entries_written")
            .and_then(Value::as_u64),
        Some(2)
    );
    assert_eq!(
        enrichment_events[0]
            .get("days")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
        Some(vec!["20260818"])
    );

    assert_eq!(
        events.len(),
        4,
        "exact total 4 events emitted (2 observed + 1 drain + 1 enrichment_ready)"
    );
}

// ---------------------------------------------------------------------------
// AC7: Source hints & Concurrency refusal
// ---------------------------------------------------------------------------

#[test]
fn ac7_1_completed_import_has_no_source_hint_key() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let note = journal.join("note.md");
    fs::write(&note, "note content").unwrap();
    let timestamp = "20260818_210000";

    let result = run_cli_test(
        &journal,
        &["--timestamp", timestamp, note.to_str().unwrap()],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);

    let import_dir = journal.join("imports").join(timestamp);
    let bytes = fs::read(import_dir.join("import.json")).unwrap();
    let metadata: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        metadata.get("source_hint").is_none(),
        "completed import must not persist source_hint"
    );
}

#[test]
fn ac7_2_live_running_refusal_occurs_before_producer_writes() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let note = journal.join("note.md");
    fs::write(&note, "concurrent note").unwrap();
    let timestamp = "20260818_220000";

    let live_started_ms = now_ms();
    admit_running_attempt(&journal, timestamp, live_started_ms, None).unwrap();

    let result = run_cli_test(
        &journal,
        &["--timestamp", timestamp, note.to_str().unwrap()],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 1);
    assert!(
        result
            .stderr
            .contains("text import failed: another import of this file is already running"),
        "stderr: {}",
        result.stderr
    );

    // Refusal happened before producer: chronicle directory has no segments
    let day_dir = journal.join("chronicle/20260818");
    assert!(
        !day_dir.join("import.text").exists(),
        "no segment directories created under chronicle"
    );

    // Expired attempt is admitted without refusal
    let expired_ms = live_started_ms.saturating_sub(RUNNING_ATTEMPT_BOUND_MS + 5000);
    let mut metadata = Map::new();
    metadata.insert(
        "attempt".to_owned(),
        json!({
            "attempt_id": format!("{timestamp}:1"),
            "generation": 1,
            "state": "running",
            "started_at_ms": expired_ms
        }),
    );
    let import_dir = journal.join("imports").join(timestamp);
    fs::create_dir_all(&import_dir).unwrap();
    fs::write(
        import_dir.join("import.json"),
        serde_json::to_vec(&Value::Object(metadata)).unwrap(),
    )
    .unwrap();

    let result_expired = run_cli_test(
        &journal,
        &["--timestamp", timestamp, note.to_str().unwrap()],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(
        result_expired.exit_code, 0,
        "stderr: {}",
        result_expired.stderr
    );
}

// ---------------------------------------------------------------------------
// AC8: Seams red proof assertions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac8_red_proof_injected_publish_failure_leaves_unconfirmed() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260818_230000";

    let generation = admit_running_attempt(&journal, import_id, now_ms(), None)
        .unwrap()
        .generation;

    let failing_publish =
        |_: solstone_core_import::publish::PublicationInput<'_>,
         _: &dyn solstone_core_import::publish::PublicationOperations| {
            Err(PublishError::RecordRead {
                path: PathBuf::from("/nonexistent"),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "injected read fail"),
            })
        };

    let seams = TextTerminalSeams {
        hold_lock_fn: None,
        publish_fn: Some(&failing_publish),
        record_completed_fn: None,
        record_unconfirmed_fn: None,
    };

    let result = finish_text_attempt_with(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Success(&[]),
        seams,
    )
    .unwrap();
    assert_eq!(result, TextFinish::Applied);

    let provenance = read_provenance(&journal, import_id).unwrap().unwrap();
    let solstone_core_import::metadata::AttemptRead::Present(facts) =
        read_attempt_facts(&provenance)
    else {
        panic!("attempt facts absent");
    };
    assert_eq!(
        facts.state,
        AttemptState::Unconfirmed,
        "failed publication must leave state Unconfirmed"
    );
    assert_eq!(facts.failure_reason, None);
}

#[tokio::test]
async fn ac8_red_proof_injected_attempt_write_failure_returns_attempt_write_error() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path().join("journal");
    fs::create_dir_all(&journal).unwrap();
    let import_id = "20260818_235000";

    let generation = admit_running_attempt(&journal, import_id, now_ms(), None)
        .unwrap()
        .generation;

    let failing_record = |_: &Path, _: &str, _: u64, _: u64, _: Option<u64>, _: Option<String>| {
        Err(ImportError::SourceMissing {
            path: PathBuf::from("/nonexistent"),
        })
    };

    let seams = TextTerminalSeams {
        hold_lock_fn: None,
        publish_fn: None,
        record_completed_fn: Some(&failing_record),
        record_unconfirmed_fn: None,
    };

    let result = finish_text_attempt_with(
        &journal,
        import_id,
        generation,
        TextTerminalInput::Success(&[]),
        seams,
    );
    assert!(
        matches!(result, Err(TextFinishError::AttemptWrite)),
        "injected attempt record write failure must return Err(TextFinishError::AttemptWrite)"
    );
}
