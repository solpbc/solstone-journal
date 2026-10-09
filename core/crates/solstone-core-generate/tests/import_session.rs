// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, Log, Metadata, Record};
use serde_json::Value;
use solstone_core_generate::{
    ClientError, GenerateRequest, GenerateResponse, OneShotClient, SessionClient,
};
use solstone_core_import::{
    CreatedSegment, PublicationOperations, SystemWireClient, TextImportOutcome, WireClient,
    process_transcript_with_wire,
};
use solstone_core_import_sources::document::{
    DocumentImportRequest, DocumentModelClient, PdfPage, PdfPayload, PdfWorker, PdfWorkerRequest,
    SystemDocumentModelClient, import as import_document,
};
use solstone_core_indexer_store::scan::RescanFileStatus;
use solstone_core_segment::StreamAdvance;

static TEST_LOCK: Mutex<()> = Mutex::new(());

/// A synthetic transcript in the v1 layout whose two turns fall in two segments, so a
/// text import asks the model twice, once per segment, for topics and setting only.
const TWO_SEGMENTS: &str =
    "## 00:00:00\n**Ana Lima:** alpha\n\n## 00:05:00\n**Ben Okafor:** beta\n";

/// The request the text importer sends for one segment's topics.
fn topics_request(text: &str) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: "observe.detect.topics".to_owned(),
        contents: vec![solstone_core_generate::ContentPart::Text {
            text: text.to_owned(),
        }],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 256,
        timeout_s: None,
        json_output: true,
        json_schema: None,
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}
static WARN_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct TestLogger;

impl Log for TestLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record) {
        if record.level() == Level::Warn {
            let mut lines = WARN_LINES.lock().unwrap();
            lines.push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

static INIT_LOGGER: std::sync::Once = std::sync::Once::new();

fn init_test_logger() {
    INIT_LOGGER.call_once(|| {
        log::set_max_level(log::LevelFilter::Warn);
        let _ = log::set_boxed_logger(Box::new(TestLogger));
    });
}

fn take_warn_lines() -> Vec<String> {
    let mut lines = WARN_LINES.lock().unwrap();
    std::mem::take(&mut *lines)
}

fn session_stub_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_solstone-generate-session-stub"))
}

fn one_shot_stub_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_solstone-generate-one-shot-stub"))
}

const PNG_1X1: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1, 13, 10,
    45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

struct FakeWorker {
    responses: Mutex<VecDeque<PdfPayload>>,
    requests: Mutex<Vec<PdfWorkerRequest>>,
}

impl FakeWorker {
    fn new(responses: Vec<PdfPayload>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl PdfWorker for FakeWorker {
    fn execute(
        &self,
        request: &PdfWorkerRequest,
    ) -> Result<PdfPayload, solstone_core_import_sources::document::WorkerFailure> {
        self.requests.lock().unwrap().push(request.clone());
        let payload = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected worker call");
        if let Some(render) = &request.render {
            fs::create_dir_all(&render.render_dir).unwrap();
            for page in &payload.pages {
                if let Some(name) = &page.rendered {
                    fs::write(render.render_dir.join(name), PNG_1X1).unwrap();
                }
            }
        }
        Ok(payload)
    }
}

fn single_image_page(index: usize) -> PdfPage {
    PdfPage {
        index,
        rendered: Some(format!("page-{index:04}.png")),
        ..PdfPage::default()
    }
}

#[derive(Default)]
struct FakePublication;

impl PublicationOperations for FakePublication {
    fn advance_stream(
        &self,
        _: &Path,
        _: &CreatedSegment,
    ) -> Result<StreamAdvance, solstone_core_segment::UnboundStreamAdvanceError> {
        Ok(StreamAdvance {
            prev_day: None,
            prev_segment: None,
            seq: 1,
        })
    }
    fn rescan_file(&self, _: &Path, _: &Path) -> Result<RescanFileStatus, String> {
        Ok(RescanFileStatus::Declined)
    }
    fn touch_stream_health_marker(&self, _: &Path, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn emit_observed(&self, _: &Path, _: Option<&str>, _: &str, _: &str, _: &str) {}
    fn emit_enrichment_ready(
        &self,
        _: &Path,
        _: Option<&str>,
        _: &str,
        _: &str,
        _: &[String],
        _: u64,
    ) {
    }
    fn emit_drain(&self, _: &Path, _: Option<&str>, _: &str) {}
}

struct OneShotWireClient(OneShotClient);

impl WireClient for OneShotWireClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.0.execute(request)
    }
}

struct OneShotDocClient(OneShotClient);

impl DocumentModelClient for OneShotDocClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.0.execute(request)
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "solstone-import-session-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&path);
    path
}

fn read_request_log(path: &Path) -> Vec<Value> {
    if !path.exists() {
        return Vec::new();
    }
    let content = fs::read_to_string(path).unwrap();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn read_pid_file(path: &Path) -> u32 {
    let content = fs::read_to_string(path).expect("read pid file");
    content.trim().parse::<u32>().expect("parse pid")
}

#[test]
fn oracle_13_one_child_and_same_branches() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _ = take_warn_lines();

    // 1. Text transcript import in import_context mode
    let temp = temp_dir("oracle_13_text");
    let source_path = temp.join("source.txt");
    fs::write(&source_path, TWO_SEGMENTS).unwrap();
    let day_dir = temp.join("chronicle/20260311");
    let pid_path = temp.join("session.pid");
    let reqs_path = temp.join("requests.jsonl");

    let session_client = OneShotClient::at_path(session_stub_path())
        .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "import_context")
        .with_env(
            "SOLSTONE_GENERATE_SESSION_STUB_PID_PATH",
            pid_path.to_str().unwrap(),
        )
        .with_env(
            "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
            reqs_path.to_str().unwrap(),
        );
    let wire = SystemWireClient::new(session_client);

    let outcome = process_transcript_with_wire(
        &source_path,
        &day_dir,
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &wire,
    );
    let TextImportOutcome::Success(work) = outcome else {
        panic!("expected success outcome");
    };

    let pid = read_pid_file(&pid_path);
    assert!(pid > 0);

    let reqs = read_request_log(&reqs_path);
    assert_eq!(
        reqs.len(),
        2,
        "request log must have 2 lines: one topics request per segment"
    );
    assert_eq!(reqs[0]["context"], "observe.detect.topics");
    assert_eq!(reqs[1]["context"], "observe.detect.topics");

    // Compare written transcript files byte-for-byte with OneShotClient
    let temp_oneshot = temp_dir("oracle_13_oneshot");
    let source_oneshot = temp_oneshot.join("source.txt");
    fs::write(&source_oneshot, TWO_SEGMENTS).unwrap();
    let day_oneshot = temp_oneshot.join("chronicle/20260311");
    let oneshot_client = OneShotClient::at_path(one_shot_stub_path())
        .with_env("SOLSTONE_GENERATE_ONE_SHOT_STUB_MODE", "import_context");
    let oneshot_wire = OneShotWireClient(oneshot_client);

    let outcome_oneshot = process_transcript_with_wire(
        &source_oneshot,
        &day_oneshot,
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &oneshot_wire,
    );
    let TextImportOutcome::Success(work_oneshot) = outcome_oneshot else {
        panic!("expected oneshot success");
    };

    assert_eq!(work.created.len(), work_oneshot.created.len());
    for (a, b) in work.created.iter().zip(work_oneshot.created.iter()) {
        let bytes_a = fs::read(&a.path).unwrap();
        let bytes_b = fs::read(&b.path).unwrap();
        assert_eq!(
            bytes_a, bytes_b,
            "transcript files must match byte-for-byte"
        );
    }
    wire.finish();

    // 2. Both stubs refuse the model -> the same files, without topics
    let temp_refuse_session = temp_dir("oracle_13_refuse_session");
    let source_refuse_session = temp_refuse_session.join("source.txt");
    fs::write(&source_refuse_session, TWO_SEGMENTS).unwrap();
    let day_refuse_session = temp_refuse_session.join("chronicle/20260311");

    let wire_refuse_session = SystemWireClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "import_refuse_model"),
    );
    let outcome_refuse_session = process_transcript_with_wire(
        &source_refuse_session,
        &day_refuse_session,
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &wire_refuse_session,
    );
    let TextImportOutcome::Success(work_refuse_session) = outcome_refuse_session else {
        panic!("expected success on refuse segment");
    };

    let temp_refuse_oneshot = temp_dir("oracle_13_refuse_oneshot");
    let source_refuse_oneshot = temp_refuse_oneshot.join("source.txt");
    fs::write(&source_refuse_oneshot, TWO_SEGMENTS).unwrap();
    let day_refuse_oneshot = temp_refuse_oneshot.join("chronicle/20260311");

    let wire_refuse_oneshot =
        OneShotWireClient(OneShotClient::at_path(one_shot_stub_path()).with_env(
            "SOLSTONE_GENERATE_ONE_SHOT_STUB_MODE",
            "import_refuse_model",
        ));
    let outcome_refuse_oneshot = process_transcript_with_wire(
        &source_refuse_oneshot,
        &day_refuse_oneshot,
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &wire_refuse_oneshot,
    );
    let TextImportOutcome::Success(work_refuse_oneshot) = outcome_refuse_oneshot else {
        panic!("expected success on oneshot refuse segment");
    };

    assert_eq!(
        work_refuse_session.created.len(),
        work_refuse_oneshot.created.len()
    );
    for (a, b) in work_refuse_session
        .created
        .iter()
        .zip(work_refuse_oneshot.created.iter())
    {
        let bytes_a = fs::read(&a.path).unwrap();
        let bytes_b = fs::read(&b.path).unwrap();
        assert_eq!(
            bytes_a, bytes_b,
            "transcript files without topics must match byte-for-byte"
        );
    }
    wire_refuse_session.finish();

    // 3. Sibling client failure assertion
    assert!(
        SessionClient::sibling_path().is_err(),
        "SessionClient::sibling_path() must be Err in test environment"
    );
    let sibling_wire = SystemWireClient::sibling();
    let temp_sibling = temp_dir("oracle_13_sibling");
    let source_sibling = temp_sibling.join("source.txt");
    fs::write(&source_sibling, TWO_SEGMENTS).unwrap();
    let outcome_sibling = process_transcript_with_wire(
        &source_sibling,
        &temp_sibling.join("chronicle/20260311"),
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &sibling_wire,
    );
    assert!(matches!(outcome_sibling, TextImportOutcome::Success(_)));

    // 4. Missing executable error formatting
    let missing_wire = SystemWireClient::new(OneShotClient::at_path(temp.join("no-such-binary")));
    let temp_missing = temp_dir("oracle_13_missing");
    let source_missing = temp_missing.join("source.txt");
    fs::write(&source_missing, TWO_SEGMENTS).unwrap();
    let outcome_missing = process_transcript_with_wire(
        &source_missing,
        &temp_missing.join("chronicle/20260311"),
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &missing_wire,
    );
    // A model that cannot start changes nothing the file carries: the import lands.
    let TextImportOutcome::Success(work_missing) = outcome_missing else {
        panic!("expected success without a model");
    };
    assert_eq!(work_missing.created.len(), 2);
    let err = missing_wire
        .execute(&topics_request("alpha"))
        .expect_err("a missing executable is a client error");
    let err_display = format!("{err}");
    assert!(!err_display.contains("session-req-"));
    assert!(!err_display.contains("solstone-generate-session-terminal-v2"));
    assert!(!err_display.contains("alpha"));
    assert!(!err_display.contains("credential"));

    // 5. One image-only document page, mode import_context
    let temp_doc = temp_dir("oracle_13_doc");
    let doc_source = temp_doc.join("doc.pdf");
    fs::write(&doc_source, b"fake pdf bytes").unwrap();
    let doc_pid_path = temp_doc.join("doc.pid");
    let doc_reqs_path = temp_doc.join("doc_requests.jsonl");

    let worker = FakeWorker::new(vec![
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 0,
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        },
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![single_image_page(1)],
            ..PdfPayload::default()
        },
    ]);

    let doc_model = SystemDocumentModelClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "import_context")
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_PID_PATH",
                doc_pid_path.to_str().unwrap(),
            )
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
                doc_reqs_path.to_str().unwrap(),
            ),
    );

    let doc_req = DocumentImportRequest {
        source: &doc_source,
        journal_root: &temp_doc,
        import_dir: &temp_doc.join("import"),
        import_id: "doc-0",
        revision: None,
        password: None,
        force: false,
        now: SystemTime::now(),
    };
    let pub_ops = FakePublication;
    let doc_result = import_document(doc_req, &worker, &doc_model, &pub_ops);
    assert_eq!(doc_result.entries_written, 1);

    let doc_pid = read_pid_file(&doc_pid_path);
    assert!(doc_pid > 0);

    let manifest_path = temp_doc.join("import/content_manifest.jsonl");
    let manifest_content = fs::read_to_string(manifest_path).unwrap();
    let manifest_val: Value = serde_json::from_str(&manifest_content).unwrap();
    assert_eq!(manifest_val["meta"]["model_calls"], 1);

    let (day, segment) = &doc_result.segments.as_ref().unwrap()[0];
    let transcript_file = temp_doc
        .join("chronicle")
        .join(day)
        .join("import.document")
        .join(segment)
        .join("document_transcript.md");
    let transcript_text = fs::read_to_string(&transcript_file).unwrap();
    assert!(
        transcript_text.contains("page text from the model"),
        "transcript must contain model text"
    );

    // OneShotClient document comparison
    let temp_doc_oneshot = temp_dir("oracle_13_doc_oneshot");
    let doc_source_oneshot = temp_doc_oneshot.join("doc.pdf");
    fs::write(&doc_source_oneshot, b"fake pdf bytes").unwrap();
    let worker_oneshot = FakeWorker::new(vec![
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 0,
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        },
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![single_image_page(1)],
            ..PdfPayload::default()
        },
    ]);
    let doc_model_oneshot = OneShotDocClient(
        OneShotClient::at_path(one_shot_stub_path())
            .with_env("SOLSTONE_GENERATE_ONE_SHOT_STUB_MODE", "import_context"),
    );
    let doc_req_oneshot = DocumentImportRequest {
        source: &doc_source_oneshot,
        journal_root: &temp_doc_oneshot,
        import_dir: &temp_doc_oneshot.join("import"),
        import_id: "doc-0",
        revision: None,
        password: None,
        force: false,
        now: SystemTime::now(),
    };
    let doc_result_oneshot = import_document(
        doc_req_oneshot,
        &worker_oneshot,
        &doc_model_oneshot,
        &pub_ops,
    );
    let (day_os, seg_os) = &doc_result_oneshot.segments.as_ref().unwrap()[0];
    let transcript_file_oneshot = temp_doc_oneshot
        .join("chronicle")
        .join(day_os)
        .join("import.document")
        .join(seg_os)
        .join("document_transcript.md");
    let transcript_text_oneshot = fs::read_to_string(transcript_file_oneshot).unwrap();
    assert_eq!(transcript_text, transcript_text_oneshot);

    // 6. Document transport failure marker test
    let temp_doc_missing = temp_dir("oracle_13_doc_missing");
    let doc_source_missing = temp_doc_missing.join("doc.pdf");
    fs::write(&doc_source_missing, b"fake pdf bytes").unwrap();
    let worker_missing = FakeWorker::new(vec![
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 0,
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        },
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![single_image_page(1)],
            ..PdfPayload::default()
        },
    ]);
    let doc_model_missing = SystemDocumentModelClient::new(OneShotClient::at_path(
        temp_doc_missing.join("no-such-binary"),
    ));
    let doc_req_missing = DocumentImportRequest {
        source: &doc_source_missing,
        journal_root: &temp_doc_missing,
        import_dir: &temp_doc_missing.join("import"),
        import_id: "doc-0",
        revision: None,
        password: None,
        force: false,
        now: SystemTime::now(),
    };
    let doc_res_missing = import_document(
        doc_req_missing,
        &worker_missing,
        &doc_model_missing,
        &pub_ops,
    );
    let (day_m, seg_m) = &doc_res_missing.segments.as_ref().unwrap()[0];
    let transcript_missing = fs::read_to_string(
        temp_doc_missing
            .join("chronicle")
            .join(day_m)
            .join("import.document")
            .join(seg_m)
            .join("document_transcript.md"),
    )
    .unwrap();
    assert!(!transcript_missing.contains("session-req-"));
    assert!(!transcript_missing.contains("solstone-generate-session-terminal-v2"));
    assert!(!transcript_missing.contains("alpha"));
    assert!(!transcript_missing.contains("credential"));

    // Document refused page
    let temp_doc_refused = temp_dir("oracle_13_doc_refused");
    let doc_source_refused = temp_doc_refused.join("doc.pdf");
    fs::write(&doc_source_refused, b"fake pdf bytes").unwrap();
    let worker_refused = FakeWorker::new(vec![
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 0,
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        },
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![single_image_page(1)],
            ..PdfPayload::default()
        },
    ]);
    let doc_model_refused = SystemDocumentModelClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "import_refuse_model"),
    );
    let doc_req_refused = DocumentImportRequest {
        source: &doc_source_refused,
        journal_root: &temp_doc_refused,
        import_dir: &temp_doc_refused.join("import"),
        import_id: "doc-0",
        revision: None,
        password: None,
        force: false,
        now: SystemTime::now(),
    };
    let doc_res_refused = import_document(
        doc_req_refused,
        &worker_refused,
        &doc_model_refused,
        &pub_ops,
    );
    let (day_r, seg_r) = &doc_res_refused.segments.as_ref().unwrap()[0];
    let transcript_refused = fs::read_to_string(
        temp_doc_refused
            .join("chronicle")
            .join(day_r)
            .join("import.document")
            .join(seg_r)
            .join("document_transcript.md"),
    )
    .unwrap();
    assert!(transcript_refused.contains("model refused: provider-response-invalid"));
}

#[test]
fn oracle_14_resubmit_once() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _ = take_warn_lines();

    // 1. Text import in import_closed_once mode
    let temp = temp_dir("oracle_14_text");
    let source_path = temp.join("source.txt");
    fs::write(&source_path, TWO_SEGMENTS).unwrap();
    let day_dir = temp.join("chronicle/20260311");
    let pid_path = temp.join("session.pid");
    let reqs_path = temp.join("requests.jsonl");

    let wire = SystemWireClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "import_closed_once")
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_PID_PATH",
                pid_path.to_str().unwrap(),
            )
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
                reqs_path.to_str().unwrap(),
            ),
    );

    let outcome = process_transcript_with_wire(
        &source_path,
        &day_dir,
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &wire,
    );
    assert!(matches!(outcome, TextImportOutcome::Success(_)));

    let reqs = read_request_log(&reqs_path);
    assert_eq!(
        reqs.len(),
        3,
        "log length must be 3: attempt 0 + attempt 1 for the first segment + the second segment"
    );
    assert_eq!(reqs[0]["attempt_index"], 0);
    assert_eq!(reqs[0]["context"], "observe.detect.topics");
    assert_eq!(reqs[1]["attempt_index"], 1);
    assert_eq!(reqs[1]["context"], "observe.detect.topics");
    assert_ne!(reqs[0]["id"], reqs[1]["id"]);

    let pid = read_pid_file(&pid_path);
    assert!(pid > 0);

    let warns = take_warn_lines();
    assert_eq!(warns.len(), 1);
    assert_eq!(
        warns[0],
        "confidential channel closed before a response: reason_code=confidential_channel_closed context=observe.detect.topics attempt_index=1"
    );
    wire.finish();

    // 2. Mode refuse_confidential_closed_always on every topics call
    let temp_always = temp_dir("oracle_14_always");
    let source_always = temp_always.join("source.txt");
    fs::write(&source_always, TWO_SEGMENTS).unwrap();
    let reqs_always = temp_always.join("requests.jsonl");

    let wire_always = SystemWireClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_MODE",
                "refuse_confidential_closed_always",
            )
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
                reqs_always.to_str().unwrap(),
            ),
    );

    let outcome_always = process_transcript_with_wire(
        &source_always,
        &temp_always.join("chronicle/20260311"),
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &wire_always,
    );
    assert!(matches!(outcome_always, TextImportOutcome::Success(_)));
    let reqs_always_log = read_request_log(&reqs_always);
    assert_eq!(
        reqs_always_log.len(),
        4,
        "log length must be 4: attempt 0 + attempt 1 for each of the two segments"
    );
    let warns_always = take_warn_lines();
    assert_eq!(warns_always.len(), 2);
    wire_always.finish();

    // 3. Mode import_refuse_model: each segment refused once, not resubmitted, no warn
    let temp_refuse = temp_dir("oracle_14_refuse");
    let source_refuse = temp_refuse.join("source.txt");
    fs::write(&source_refuse, TWO_SEGMENTS).unwrap();
    let reqs_refuse = temp_refuse.join("requests.jsonl");

    let wire_refuse = SystemWireClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "import_refuse_model")
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
                reqs_refuse.to_str().unwrap(),
            ),
    );
    let _ = process_transcript_with_wire(
        &source_refuse,
        &temp_refuse.join("chronicle/20260311"),
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &wire_refuse,
    );
    let reqs_refuse_log = read_request_log(&reqs_refuse);
    assert_eq!(
        reqs_refuse_log.len(),
        2,
        "log length must be 2: one refusal per segment, no resubmit"
    );
    assert_eq!(reqs_refuse_log[0]["attempt_index"], 0);
    assert_eq!(reqs_refuse_log[1]["attempt_index"], 0);
    assert_eq!(take_warn_lines().len(), 0);
    wire_refuse.finish();

    // 4. One image-only document page, mode import_closed_once
    let temp_doc = temp_dir("oracle_14_doc");
    let doc_source = temp_doc.join("doc.pdf");
    fs::write(&doc_source, b"fake pdf bytes").unwrap();
    let doc_reqs_path = temp_doc.join("doc_requests.jsonl");

    let worker = FakeWorker::new(vec![
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 0,
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        },
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![single_image_page(1)],
            ..PdfPayload::default()
        },
    ]);

    let doc_model = SystemDocumentModelClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "import_closed_once")
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
                doc_reqs_path.to_str().unwrap(),
            ),
    );

    let doc_req = DocumentImportRequest {
        source: &doc_source,
        journal_root: &temp_doc,
        import_dir: &temp_doc.join("import"),
        import_id: "doc-0",
        revision: None,
        password: None,
        force: false,
        now: SystemTime::now(),
    };
    let pub_ops = FakePublication;
    let doc_result = import_document(doc_req, &worker, &doc_model, &pub_ops);
    assert_eq!(doc_result.entries_written, 1);

    let manifest_path = temp_doc.join("import/content_manifest.jsonl");
    let manifest_content = fs::read_to_string(manifest_path).unwrap();
    let manifest_val: Value = serde_json::from_str(&manifest_content).unwrap();
    assert_eq!(manifest_val["meta"]["model_calls"], 2);

    let doc_reqs = read_request_log(&doc_reqs_path);
    assert_eq!(doc_reqs.len(), 2);
    assert_eq!(doc_reqs[0]["attempt_index"], 0);
    assert_eq!(doc_reqs[1]["attempt_index"], 1);
    assert_ne!(doc_reqs[0]["id"], doc_reqs[1]["id"]);

    let doc_warns = take_warn_lines();
    assert_eq!(doc_warns.len(), 1);
    let doc_context = doc_reqs[0]["context"].as_str().unwrap();
    assert_eq!(
        doc_warns[0],
        format!(
            "confidential channel closed before a response: reason_code=confidential_channel_closed context={doc_context} attempt_index=1"
        )
    );

    let (day, segment) = &doc_result.segments.as_ref().unwrap()[0];
    let transcript_file = temp_doc
        .join("chronicle")
        .join(day)
        .join("import.document")
        .join(segment)
        .join("document_transcript.md");
    let transcript_text = fs::read_to_string(&transcript_file).unwrap();
    assert!(transcript_text.contains("page text from the model"));

    // 5. Document refuse_confidential_closed_always
    let temp_doc_always = temp_dir("oracle_14_doc_always");
    let doc_source_always = temp_doc_always.join("doc.pdf");
    fs::write(&doc_source_always, b"fake pdf bytes").unwrap();
    let doc_reqs_always = temp_doc_always.join("doc_requests.jsonl");

    let worker_always = FakeWorker::new(vec![
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 0,
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        },
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![single_image_page(1)],
            ..PdfPayload::default()
        },
    ]);

    let doc_model_always = SystemDocumentModelClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_MODE",
                "refuse_confidential_closed_always",
            )
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
                doc_reqs_always.to_str().unwrap(),
            ),
    );

    let doc_req_always = DocumentImportRequest {
        source: &doc_source_always,
        journal_root: &temp_doc_always,
        import_dir: &temp_doc_always.join("import"),
        import_id: "doc-0",
        revision: None,
        password: None,
        force: false,
        now: SystemTime::now(),
    };
    let doc_res_always =
        import_document(doc_req_always, &worker_always, &doc_model_always, &pub_ops);
    let doc_reqs_always_log = read_request_log(&doc_reqs_always);
    assert_eq!(doc_reqs_always_log.len(), 2);
    let manifest_always: Value = serde_json::from_str(
        &fs::read_to_string(temp_doc_always.join("import/content_manifest.jsonl")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest_always["meta"]["model_calls"], 2);

    let (day_a, seg_a) = &doc_res_always.segments.as_ref().unwrap()[0];
    let transcript_always = fs::read_to_string(
        temp_doc_always
            .join("chronicle")
            .join(day_a)
            .join("import.document")
            .join(seg_a)
            .join("document_transcript.md"),
    )
    .unwrap();
    assert!(transcript_always.contains("model refused: confidential-channel-closed"));

    // 6. Document import_refuse_segment
    let _ = take_warn_lines();
    let temp_doc_ref = temp_dir("oracle_14_doc_ref");
    let doc_source_ref = temp_doc_ref.join("doc.pdf");
    fs::write(&doc_source_ref, b"fake pdf bytes").unwrap();
    let doc_reqs_ref = temp_doc_ref.join("doc_requests.jsonl");

    let worker_ref = FakeWorker::new(vec![
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 0,
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        },
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![single_image_page(1)],
            ..PdfPayload::default()
        },
    ]);

    let doc_model_ref = SystemDocumentModelClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "import_refuse_model")
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
                doc_reqs_ref.to_str().unwrap(),
            ),
    );

    let doc_req_ref = DocumentImportRequest {
        source: &doc_source_ref,
        journal_root: &temp_doc_ref,
        import_dir: &temp_doc_ref.join("import"),
        import_id: "doc-0",
        revision: None,
        password: None,
        force: false,
        now: SystemTime::now(),
    };
    let doc_res_ref = import_document(doc_req_ref, &worker_ref, &doc_model_ref, &pub_ops);
    let doc_reqs_ref_log = read_request_log(&doc_reqs_ref);
    assert_eq!(doc_reqs_ref_log.len(), 1);
    let manifest_ref: Value = serde_json::from_str(
        &fs::read_to_string(temp_doc_ref.join("import/content_manifest.jsonl")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest_ref["meta"]["model_calls"], 1);

    let (day_rf, seg_rf) = &doc_res_ref.segments.as_ref().unwrap()[0];
    let transcript_ref = fs::read_to_string(
        temp_doc_ref
            .join("chronicle")
            .join(day_rf)
            .join("import.document")
            .join(seg_rf)
            .join("document_transcript.md"),
    )
    .unwrap();
    assert!(transcript_ref.contains("model refused: provider-response-invalid"));
    assert_eq!(take_warn_lines().len(), 0);
}

#[test]
fn oracle_15_child_death_respawns_later() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _ = take_warn_lines();

    // 1. Text import with exit_once
    let temp = temp_dir("oracle_15_text");
    let source_path = temp.join("source.txt");
    fs::write(&source_path, TWO_SEGMENTS).unwrap();
    let day_dir = temp.join("chronicle/20260311");
    let exit_once_path = temp.join("exit_once_marker");
    let pid_path = temp.join("session.pid");
    let reqs_path = temp.join("requests.jsonl");

    let wire = SystemWireClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "exit_once")
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_EXIT_ONCE_PATH",
                exit_once_path.to_str().unwrap(),
            )
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_PID_PATH",
                pid_path.to_str().unwrap(),
            )
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
                reqs_path.to_str().unwrap(),
            ),
    );

    // The child exits on its first request: a client error, and no resubmit.
    let client_err = wire
        .execute(&topics_request("alpha"))
        .expect_err("expected a client error from the exited child");
    assert!(
        matches!(client_err, ClientError::UnexpectedChild(_)),
        "source error must be UnexpectedChild"
    );
    let err_display = format!("{client_err}");
    assert!(!err_display.contains("session-req-"));
    assert!(!err_display.contains("solstone-generate-session-terminal-v2"));
    assert!(!err_display.contains("alpha"));
    assert!(!err_display.contains("credential"));

    let reqs1 = read_request_log(&reqs_path);
    assert_eq!(reqs1.len(), 1, "no resubmit on child exit");
    let pid1 = read_pid_file(&pid_path);

    // Second call on the same client succeeds on a new child PID
    let outcome2 = process_transcript_with_wire(
        &source_path,
        &day_dir,
        "12:00:00",
        "20260311_120000",
        "import.text",
        None,
        None,
        &wire,
    );
    assert!(matches!(outcome2, TextImportOutcome::Success(_)));
    let pid2 = read_pid_file(&pid_path);
    assert_ne!(
        pid1, pid2,
        "new child PID must be different from the exited one"
    );
    wire.finish();

    // 2. Document import with exit_once
    let temp_doc = temp_dir("oracle_15_doc");
    let doc_source = temp_doc.join("doc.pdf");
    fs::write(&doc_source, b"fake pdf bytes").unwrap();
    let doc_exit_once_path = temp_doc.join("doc_exit_once_marker");
    let doc_pid_path = temp_doc.join("doc.pid");
    let doc_reqs_path = temp_doc.join("doc_requests.jsonl");

    let doc_model = SystemDocumentModelClient::new(
        OneShotClient::at_path(session_stub_path())
            .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "exit_once")
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_EXIT_ONCE_PATH",
                doc_exit_once_path.to_str().unwrap(),
            )
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_PID_PATH",
                doc_pid_path.to_str().unwrap(),
            )
            .with_env(
                "SOLSTONE_GENERATE_SESSION_STUB_REQUESTS_PATH",
                doc_reqs_path.to_str().unwrap(),
            ),
    );

    let worker1 = FakeWorker::new(vec![
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 0,
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        },
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![single_image_page(1)],
            ..PdfPayload::default()
        },
    ]);

    let doc_req1 = DocumentImportRequest {
        source: &doc_source,
        journal_root: &temp_doc,
        import_dir: &temp_doc.join("import1"),
        import_id: "doc-0",
        revision: None,
        password: None,
        force: false,
        now: SystemTime::now(),
    };
    let pub_ops = FakePublication;
    let _ = import_document(doc_req1, &worker1, &doc_model, &pub_ops);
    let doc_reqs1 = read_request_log(&doc_reqs_path);
    assert_eq!(doc_reqs1.len(), 1, "one request before child exit");
    let doc_pid1 = read_pid_file(&doc_pid_path);

    // Second document import on the same client succeeds on a new child PID
    let worker2 = FakeWorker::new(vec![
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 0,
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        },
        PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![single_image_page(1)],
            ..PdfPayload::default()
        },
    ]);

    let doc_source2 = temp_doc.join("source2.pdf");
    fs::write(&doc_source2, b"%PDF-1.4 second document").unwrap();
    let doc_req2 = DocumentImportRequest {
        source: &doc_source2,
        journal_root: &temp_doc,
        import_dir: &temp_doc.join("import2"),
        import_id: "doc-1",
        revision: None,
        password: None,
        force: false,
        now: SystemTime::now(),
    };
    let doc_res2 = import_document(doc_req2, &worker2, &doc_model, &pub_ops);
    assert_eq!(doc_res2.entries_written, 1);
    let doc_pid2 = read_pid_file(&doc_pid_path);
    assert_ne!(doc_pid1, doc_pid2, "new child PID must be spawned");

    doc_model.finish();
}

#[test]
fn a_session_child_that_exited_between_calls_is_replaced_before_the_next_call() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = temp_dir("respawn_between_calls");
    let exit_marker = temp.join("exit_marker");
    let pid_path = temp.join("session.pid");
    let adapter = OneShotClient::at_path(session_stub_path())
        .with_env(
            "SOLSTONE_GENERATE_SESSION_STUB_MODE",
            "exit_after_first_reply",
        )
        .with_env(
            "SOLSTONE_GENERATE_SESSION_STUB_EXIT_ONCE_PATH",
            exit_marker.to_str().unwrap(),
        )
        .with_env(
            "SOLSTONE_GENERATE_SESSION_STUB_PID_PATH",
            pid_path.to_str().unwrap(),
        )
        .into_session_adapter();
    let request = |id: &str| GenerateRequest {
        id: Some(id.to_owned()),
        context: "test.respawn".to_owned(),
        contents: vec![solstone_core_generate::ContentPart::Text {
            text: "x".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.0,
        max_output_tokens: 8,
        timeout_s: None,
        json_output: false,
        json_schema: None,
        enforce_responsiveness: false,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    };

    let first = adapter
        .execute(&request("first"))
        .expect("first call answered");
    assert!(matches!(first, GenerateResponse::Generated(_)));
    let first_pid = read_pid_file(&pid_path);
    // Wait until the first child is gone, so the next call meets an ended session.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !exit_marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "first child never exited"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The stub exits right after writing the marker; give the session's 10 ms
    // exit poll time to observe it.
    std::thread::sleep(std::time::Duration::from_millis(300));

    let second = adapter
        .execute(&request("second"))
        .expect("second call answered by a new child");
    assert!(matches!(second, GenerateResponse::Generated(_)));
    assert_ne!(
        read_pid_file(&pid_path),
        first_pid,
        "a new child served the second call"
    );
    adapter.finish();
    let _ = fs::remove_dir_all(&temp);
}
