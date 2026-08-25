// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private, in-process coverage for describe pipeline semantics.
//!
//! The `cli` integration harness retains the native decode, masking, detector,
//! session-child, and command-line contracts. These tests deliberately supply
//! decoded PNG frames and a synchronous session so pipeline behavior does not
//! require repeatedly transcoding the video corpus or spawning a helper child.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use serde_json::{Value, json};
use solstone_core_generate::{
    GenerateRequest, GenerateResponse, GeneratedResponse, ReasonCode, ReasonCodeValue,
    RefusalReason, RefusedResponse, SessionCloseError, SessionCompletion, SessionFailure,
    SessionFailureReason, SessionLaunchError, SessionLaunchReason, SessionReceiveError,
    SessionSubmitError,
};

use super::{DescribeOptions, RunError, run_decoded};
use crate::decode::{DescribeResult, QualifiedFrame};
use crate::session::{DescribeSession, DescribeSessionFactory};
use crate::{WinnowConfig, selection::CategoryOverride};

type ResponsePlan = dyn Fn(&GenerateRequest) -> SessionCompletion + Send + Sync;

#[derive(Clone)]
struct ScriptedSession {
    plan: Arc<ResponsePlan>,
    requests: Arc<Mutex<Vec<GenerateRequest>>>,
    responses: Arc<Mutex<VecDeque<SessionCompletion>>>,
}

impl ScriptedSession {
    fn new(plan: impl Fn(&GenerateRequest) -> SessionCompletion + Send + Sync + 'static) -> Self {
        Self {
            plan: Arc::new(plan),
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn requests(&self) -> Vec<GenerateRequest> {
        self.requests.lock().expect("test request lock").clone()
    }
}

impl DescribeSession for ScriptedSession {
    fn submit(&self, request: GenerateRequest) -> Result<(), SessionSubmitError> {
        let response = (self.plan)(&request);
        self.requests
            .lock()
            .expect("test request lock")
            .push(request);
        self.responses
            .lock()
            .expect("test response lock")
            .push_back(response);
        Ok(())
    }

    fn recv_timeout(&self, _timeout: Duration) -> Result<SessionCompletion, SessionReceiveError> {
        self.responses
            .lock()
            .expect("test response lock")
            .pop_front()
            .ok_or(SessionReceiveError::Disconnected)
    }

    fn close(&self) -> Result<(), SessionCloseError> {
        Ok(())
    }
}

struct ScriptedFactory {
    session: ScriptedSession,
    fail_launch: bool,
}

impl ScriptedFactory {
    fn new(plan: impl Fn(&GenerateRequest) -> SessionCompletion + Send + Sync + 'static) -> Self {
        Self {
            session: ScriptedSession::new(plan),
            fail_launch: false,
        }
    }

    fn failing() -> Self {
        Self {
            session: ScriptedSession::new(default_response),
            fail_launch: true,
        }
    }

    fn requests(&self) -> Vec<GenerateRequest> {
        self.session.requests()
    }
}

impl DescribeSessionFactory for ScriptedFactory {
    fn spawn(
        &self,
        _max_in_flight: usize,
        _explicit_journal: Option<&Path>,
    ) -> Result<Box<dyn DescribeSession>, SessionLaunchError> {
        if self.fail_launch {
            return Err(SessionLaunchError {
                reason: SessionLaunchReason::Spawn("scripted launch failure".to_owned()),
                retryable: false,
                blocking: true,
            });
        }
        Ok(Box::new(self.session.clone()))
    }
}

fn generated(request: &GenerateRequest, text: impl Into<String>) -> SessionCompletion {
    SessionCompletion::Response(GenerateResponse::Generated(Box::new(GeneratedResponse {
        id: request.id.clone(),
        text: text.into(),
        model: "describe-test".to_owned(),
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

fn generated_with_finish(
    request: &GenerateRequest,
    text: impl Into<String>,
    finish_reason: &str,
    schema_validation: Option<Value>,
) -> SessionCompletion {
    SessionCompletion::Response(GenerateResponse::Generated(Box::new(GeneratedResponse {
        id: request.id.clone(),
        text: text.into(),
        model: "describe-test".to_owned(),
        usage: json!({}),
        finish_reason: finish_reason.to_owned(),
        thinking: None,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied: Vec::new(),
    })))
}

fn refused(
    request: &GenerateRequest,
    retryable: bool,
    blocking: bool,
    reason_code: Option<&str>,
) -> SessionCompletion {
    SessionCompletion::Response(GenerateResponse::Refused(RefusedResponse {
        id: request.id.clone(),
        reason: RefusalReason::ProviderResponseInvalid,
        reason_code: reason_code.map(known_reason),
        retryable,
        blocking,
        reset_at_ms: None,
        provider: Some("scripted".to_owned()),
        detail: "scripted refusal".to_owned(),
    }))
}

fn no_engine(request: &GenerateRequest) -> SessionCompletion {
    SessionCompletion::Response(GenerateResponse::Refused(RefusedResponse {
        id: request.id.clone(),
        reason: RefusalReason::NoEngineConfigured,
        reason_code: None,
        retryable: false,
        blocking: true,
        reset_at_ms: None,
        provider: None,
        detail: "no engine".to_owned(),
    }))
}

fn known_reason(value: &str) -> ReasonCodeValue {
    ReasonCodeValue::Known(ReasonCode::new(value).expect("known test reason code"))
}

fn category(primary: &str, secondary: &str, overlap: bool) -> String {
    json!({
        "visual_description": "synthetic frame",
        "primary": primary,
        "secondary": secondary,
        "overlap": overlap,
    })
    .to_string()
}

fn default_response(request: &GenerateRequest) -> SessionCompletion {
    match request.context.as_str() {
        "observe.describe.frame" => generated(request, category("code", "none", true)),
        "observe.extract.selection" => {
            generated(request, json!({"frame_ids":[1, 2, 3]}).to_string())
        }
        _ if request.json_output => generated(request, json!({"ok": true}).to_string()),
        _ => generated(request, "# extracted markdown"),
    }
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestRun {
    root: PathBuf,
    journal: PathBuf,
    video: PathBuf,
}

impl TestRun {
    fn new(label: &str) -> Self {
        let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "solstone-describe-pipeline-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create test root");
        let journal = root.join("journal");
        fs::create_dir(&journal).expect("create test journal");
        let video = root.join("screen.webm");
        fs::write(&video, b"synthetic video bytes").expect("write test input");
        Self {
            root,
            journal,
            video,
        }
    }

    fn artifact(&self) -> PathBuf {
        self.video.with_extension("jsonl")
    }

    fn options(&self, redo: bool, redact_rules: Vec<String>) -> DescribeOptions<'_> {
        DescribeOptions {
            video: &self.video,
            journal: &self.journal,
            explicit_journal: Some(&self.journal),
            jobs: 2,
            redo,
            config: WinnowConfig::default(),
            redact_rules,
            max_extractions: 3,
            category_overrides: BTreeMap::<String, CategoryOverride>::new(),
        }
    }

    fn rows(&self) -> Vec<Value> {
        fs::read_to_string(self.artifact())
            .expect("read artifact")
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid artifact JSONL"))
            .collect()
    }
}

impl Drop for TestRun {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn png() -> Vec<u8> {
    let image = ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(2, 2, Rgb([7, 8, 9]));
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode synthetic PNG");
    bytes.into_inner()
}

fn decoded(frame_ids: &[u64]) -> DescribeResult {
    DescribeResult {
        width: Some(2),
        height: Some(2),
        qualified_frames: frame_ids
            .iter()
            .enumerate()
            .map(|(index, frame_id)| QualifiedFrame {
                frame_id: *frame_id,
                timestamp: index as f64 / 10.0,
                aruco: None,
                png: png(),
            })
            .collect(),
        first_hash: Some(10),
        last_hash: Some(20),
        qualified_count: frame_ids.len(),
        decode_failed: false,
        winnow: None,
    }
}

fn phase_requests(requests: &[GenerateRequest], context: &str) -> Vec<GenerateRequest> {
    requests
        .iter()
        .filter(|request| request.context == context)
        .cloned()
        .collect()
}

fn rewrite_header(path: &Path, update: impl FnOnce(&mut Value)) {
    let contents = fs::read_to_string(path).expect("read artifact");
    let (header, rows) = contents.split_once('\n').expect("header newline");
    let mut header: Value = serde_json::from_str(header).expect("header JSON");
    update(&mut header);
    fs::write(path, format!("{header}\n{rows}")).expect("rewrite artifact");
}

fn artifact_body(path: &Path) -> String {
    fs::read_to_string(path)
        .expect("read artifact")
        .split_once('\n')
        .expect("header newline")
        .1
        .to_owned()
}

#[test]
fn synthetic_decode_covers_pipeline_contracts_without_native_media() {
    let test = TestRun::new("contracts");
    let factory = ScriptedFactory::new(default_response);

    run_decoded(
        test.options(
            false,
            vec!["secret one".to_owned(), "secret two".to_owned()],
        ),
        &factory,
        decoded(&[1, 2, 3]),
    )
    .expect("synthetic pipeline succeeds");

    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "analyzed");
    assert_eq!(rows[0]["_solstone_processing"]["reason_code"], "ok");
    assert_eq!(rows[0]["qualified_count"], 3);
    assert_eq!(rows[1..].len(), 3);
    for row in &rows[1..] {
        assert_eq!(row["analysis"]["primary"], "code");
        assert_eq!(row["enhanced"], true);
        assert_eq!(row["content"]["code"], "# extracted markdown");
        assert!(row.get("pending").is_none());
        let requests = row["requests"].as_array().expect("request records");
        assert_eq!(requests[0]["type"], "describe");
        assert_eq!(requests[1]["type"], "category");
        assert_eq!(requests[1]["category"], "code");
    }

    let requests = factory.requests();
    let phase_one = phase_requests(&requests, "observe.describe.frame");
    assert_eq!(phase_one.len(), 3);
    assert!(phase_one.iter().all(|request| {
        request.json_output
            && request.temperature == 0.7
            && request.max_output_tokens == 512
            && request.thinking_budget == Some(1024)
            && request
                .system_instruction
                .as_deref()
                .is_some_and(|instruction| instruction.ends_with("- secret one\n- secret two\n"))
    }));
    let selection = phase_requests(&requests, "observe.extract.selection");
    assert_eq!(selection.len(), 1);
    assert_eq!(selection[0].temperature, 0.3);
    assert_eq!(selection[0].max_output_tokens, 1024);
    assert_eq!(selection[0].thinking_budget, Some(4096));
    let extracts = phase_requests(&requests, "observe.describe.code");
    assert_eq!(extracts.len(), 3);
    assert!(extracts.iter().all(|request| !request.json_output));
}

#[test]
fn synthetic_screen_detection_failure_is_recorded_for_each_eligible_frame() {
    let test = TestRun::new("screen-detection-failure");
    let factory = ScriptedFactory::new(|request| {
        if request.context == "observe.describe.frame" {
            generated(request, category("media", "none", true))
        } else {
            default_response(request)
        }
    });
    run_decoded(test.options(false, Vec::new()), &factory, decoded(&[1, 2]))
        .expect("detector failure remains a row-level processing result");

    let rows = test.rows();
    for row in &rows[1..] {
        assert_eq!(row["detection_error"]["reason_code"], "rfdetr-unavailable");
        assert!(
            row["detection_error"]["detail"]
                .as_str()
                .is_some_and(|detail| !detail.is_empty())
        );
        assert!(row.get("detections").is_none());
    }
    assert_eq!(
        rows[1]["detection_error"]["detail"], rows[2]["detection_error"]["detail"],
        "later eligible frames retain the first detector failure"
    );

    let unqualified = TestRun::new("unqualified-detection");
    run_decoded(
        unqualified.options(false, Vec::new()),
        &ScriptedFactory::new(default_response),
        decoded(&[1]),
    )
    .expect("unqualified frame completes without invoking the detector");
    assert!(unqualified.rows()[1].get("detection_error").is_none());
}

#[test]
fn synthetic_decode_retries_and_latches_row_failures() {
    let retry = TestRun::new("retry");
    let factory = ScriptedFactory::new(|request| match request.context.as_str() {
        "observe.describe.frame" if request.attempt_index == 0 => {
            refused(request, true, false, Some("brain_refresh_timeout"))
        }
        _ => default_response(request),
    });
    run_decoded(retry.options(false, Vec::new()), &factory, decoded(&[1]))
        .expect("retry then success");
    let phase_one = phase_requests(&factory.requests(), "observe.describe.frame");
    assert_eq!(phase_one.len(), 2);
    assert_eq!(phase_one[0].attempt_index, 0);
    assert_eq!(phase_one[1].attempt_index, 1);

    let failed = TestRun::new("all-failed");
    let factory = ScriptedFactory::new(|request| {
        if request.context == "observe.describe.frame" {
            refused(request, true, false, Some("brain_refresh_timeout"))
        } else {
            default_response(request)
        }
    });
    assert!(matches!(
        run_decoded(failed.options(false, Vec::new()), &factory, decoded(&[1])),
        Err(RunError::Internal(_))
    ));
    assert_eq!(
        phase_requests(&factory.requests(), "observe.describe.frame").len(),
        5
    );
    let rows = failed.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    assert_eq!(rows[0]["_solstone_processing"]["attempts"], 1);
    assert_eq!(rows.len(), 1);

    let extraction = TestRun::new("extraction-failure");
    let factory = ScriptedFactory::new(|request| match request.context.as_str() {
        "observe.describe.frame" => generated(request, category("messaging", "none", true)),
        "observe.describe.messaging" => generated(request, "not JSON"),
        _ => default_response(request),
    });
    run_decoded(
        extraction.options(false, Vec::new()),
        &factory,
        decoded(&[1]),
    )
    .expect("row failure is recorded rather than aborting");
    assert_eq!(
        phase_requests(&factory.requests(), "observe.describe.messaging").len(),
        5
    );
    let rows = extraction.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    assert!(rows[1].get("error").is_some());
    assert_eq!(rows[1]["requests"][1]["retries"], 4);
}

#[test]
fn synthetic_decode_covers_selection_schema_and_category_branches() {
    let test = TestRun::new("selection");
    let factory = ScriptedFactory::new(|request| match request.context.as_str() {
        "observe.describe.frame" => match request.id.as_deref() {
            Some("frame:1:attempt:0") => generated_with_finish(
                request,
                category("code", "none", true),
                "unknown",
                Some(json!({"valid": false, "errors":[{"message":"synthetic"}]})),
            ),
            Some("frame:2:attempt:0") => {
                generated(request, category("not-a-real-category", "none", true))
            }
            _ => generated(request, category("code", "messaging", false)),
        },
        "observe.extract.selection" => generated(request, "not JSON"),
        _ => default_response(request),
    });
    run_decoded(
        test.options(false, Vec::new()),
        &factory,
        decoded(&[1, 2, 3]),
    )
    .expect("schema row failure and fallback selection complete");

    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    let invalid = rows
        .iter()
        .find(|row| row["frame_id"] == 1)
        .expect("frame 1");
    assert_eq!(invalid["error"], "Invalid JSON response");
    assert_eq!(invalid["enhanced"], false);
    let unextractable = rows
        .iter()
        .find(|row| row["frame_id"] == 2)
        .expect("frame 2");
    assert_eq!(unextractable["enhanced"], false);
    assert!(unextractable.get("content").is_none());
    let secondary = rows
        .iter()
        .find(|row| row["frame_id"] == 3)
        .expect("frame 3");
    assert_eq!(secondary["content"]["code"], "# extracted markdown");
    assert_eq!(secondary["content"]["messaging"]["ok"], true);

    let selection = phase_requests(&factory.requests(), "observe.extract.selection");
    assert_eq!(selection.len(), 1);
    let selection_text = match &selection[0].contents[0] {
        solstone_core_generate::ContentPart::Text { text } => text,
        solstone_core_generate::ContentPart::Image { .. } => panic!("selection is text"),
    };
    let summaries: Vec<Value> = serde_json::from_str(selection_text).expect("selection summaries");
    assert_eq!(summaries.len(), 2, "invalid categorization is omitted");
    assert!(phase_requests(&factory.requests(), "observe.describe.messaging").len() == 1);
}

#[test]
fn synthetic_decode_blocks_without_promoting_and_preserves_clean_reentry() {
    let blocked = TestRun::new("blocked");
    let factory = ScriptedFactory::new(|request| {
        if request.context == "observe.describe.frame" {
            refused(request, true, true, Some("binary_missing"))
        } else {
            default_response(request)
        }
    });
    assert!(matches!(
        run_decoded(blocked.options(false, Vec::new()), &factory, decoded(&[1])),
        Err(RunError::Blocked(Some(code))) if code == "binary_missing"
    ));
    assert!(!blocked.artifact().exists());

    let launch = TestRun::new("launch");
    assert!(matches!(
        run_decoded(
            launch.options(false, Vec::new()),
            &ScriptedFactory::failing(),
            decoded(&[1])
        ),
        Err(RunError::Blocked(None))
    ));
    assert!(!launch.artifact().exists());

    let reentry = TestRun::new("reentry");
    let initial = ScriptedFactory::new(default_response);
    run_decoded(
        reentry.options(false, Vec::new()),
        &initial,
        decoded(&[1, 2]),
    )
    .expect("initial artifact");
    let original = fs::read(reentry.artifact()).expect("initial artifact bytes");
    let skipped = ScriptedFactory::failing();
    run_decoded(
        reentry.options(false, Vec::new()),
        &skipped,
        decoded(&[1, 2]),
    )
    .expect("clean artifact skips before a session starts");
    assert!(
        skipped.requests().is_empty(),
        "clean artifact does not start a session"
    );
    assert_eq!(
        fs::read(reentry.artifact()).expect("unchanged artifact"),
        original,
        "clean artifacts remain byte-for-byte unchanged"
    );
}

#[test]
fn synthetic_decode_promotes_empty_and_corrupt_results() {
    let empty = TestRun::new("empty");
    run_decoded(
        empty.options(false, Vec::new()),
        &ScriptedFactory::new(default_response),
        DescribeResult::default(),
    )
    .expect("empty decode still promotes an artifact");
    let rows = empty.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["_solstone_processing"]["state"], "empty");
    assert_eq!(
        rows[0]["_solstone_processing"]["reason_code"],
        "no_decodable_frames"
    );
    assert!(rows[0].get("_solstone_thinking").is_none());

    let corrupt = TestRun::new("corrupt");
    run_decoded(
        corrupt.options(false, Vec::new()),
        &ScriptedFactory::new(default_response),
        DescribeResult {
            decode_failed: true,
            ..DescribeResult::default()
        },
    )
    .expect("corrupt decode promotes failure record");
    let rows = corrupt.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    assert_eq!(
        rows[0]["_solstone_processing"]["reason_code"],
        "corrupt_input"
    );
    assert!(rows[0].get("_solstone_thinking").is_none());
}

#[test]
fn synthetic_decode_preserves_category_contracts_and_extraction_outcomes() {
    for (primary, context, tokens, thinking, json_output) in [
        ("browsing", "observe.describe.browsing", 2048, 4096, false),
        ("messaging", "observe.describe.messaging", 8192, 6144, true),
        ("meeting", "observe.describe.meeting", 4096, 6144, true),
    ] {
        let test = TestRun::new(primary);
        let primary = primary.to_owned();
        let response_primary = primary.clone();
        let factory = ScriptedFactory::new(move |request| {
            if request.context == "observe.describe.frame" {
                generated(request, category(&response_primary, "none", true))
            } else {
                default_response(request)
            }
        });
        run_decoded(
            test.options(false, vec!["secret".to_owned()]),
            &factory,
            decoded(&[1]),
        )
        .expect("category pipeline");
        let extracts = phase_requests(&factory.requests(), context);
        assert_eq!(extracts.len(), 1, "{primary}");
        assert_eq!(extracts[0].max_output_tokens, tokens, "{primary}");
        assert_eq!(extracts[0].thinking_budget, Some(thinking), "{primary}");
        assert_eq!(extracts[0].json_output, json_output, "{primary}");
        assert_eq!(extracts[0].temperature, 0.3, "{primary}");
        assert!(
            extracts[0]
                .system_instruction
                .as_deref()
                .is_some_and(|instruction| instruction.ends_with("- secret\n")),
            "{primary} keeps redaction rules"
        );
    }

    let secondary = TestRun::new("secondary");
    let factory = ScriptedFactory::new(|request| {
        if request.context == "observe.describe.frame" {
            generated(request, category("code", "messaging", false))
        } else {
            default_response(request)
        }
    });
    run_decoded(
        secondary.options(false, Vec::new()),
        &factory,
        decoded(&[1]),
    )
    .expect("secondary category pipeline");
    let extraction_contexts = factory
        .requests()
        .into_iter()
        .filter(|request| request.context.starts_with("observe.describe."))
        .map(|request| request.context)
        .collect::<Vec<_>>();
    assert_eq!(
        extraction_contexts,
        vec![
            "observe.describe.frame",
            "observe.describe.code",
            "observe.describe.messaging"
        ]
    );

    let markdown_failure = TestRun::new("markdown-failure");
    let factory = ScriptedFactory::new(|request| match request.context.as_str() {
        "observe.describe.frame" => generated(request, category("code", "none", true)),
        "observe.describe.code" => generated(request, " \n"),
        _ => default_response(request),
    });
    run_decoded(
        markdown_failure.options(false, Vec::new()),
        &factory,
        decoded(&[1]),
    )
    .expect("row-level extraction failure completes");
    assert_eq!(
        phase_requests(&factory.requests(), "observe.describe.code").len(),
        5
    );
    let rows = markdown_failure.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    assert!(rows[1].get("error").is_some());
    assert!(
        rows[1]
            .get("content")
            .and_then(|content| content.get("code"))
            .is_none()
    );
}

#[test]
fn synthetic_decode_handles_session_and_phase_blockers_without_artifacts() {
    let no_engine_test = TestRun::new("no-engine");
    let no_engine_factory = ScriptedFactory::new(|request| {
        if request.context == "observe.describe.frame" {
            no_engine(request)
        } else {
            default_response(request)
        }
    });
    assert!(matches!(
        run_decoded(
            no_engine_test.options(false, Vec::new()),
            &no_engine_factory,
            decoded(&[1])
        ),
        Err(RunError::Blocked(None))
    ));
    assert!(!no_engine_test.artifact().exists());

    let session_test = TestRun::new("session-failure");
    let session_factory = ScriptedFactory::new(|request| {
        SessionCompletion::Failure(SessionFailure {
            id: request.id.clone().expect("request id"),
            reason: SessionFailureReason::ChildExited,
            retryable: false,
            blocking: true,
        })
    });
    assert!(matches!(
        run_decoded(
            session_test.options(false, Vec::new()),
            &session_factory,
            decoded(&[1])
        ),
        Err(RunError::Blocked(None))
    ));
    assert!(!session_test.artifact().exists());

    let selection_test = TestRun::new("selection-blocked");
    let selection_factory = ScriptedFactory::new(|request| {
        if request.context == "observe.extract.selection" {
            refused(request, true, true, Some("binary_missing"))
        } else {
            default_response(request)
        }
    });
    assert!(matches!(
        run_decoded(
            selection_test.options(false, Vec::new()),
            &selection_factory,
            decoded(&[1])
        ),
        Err(RunError::Blocked(Some(code))) if code == "binary_missing"
    ));
    assert_eq!(
        phase_requests(&selection_factory.requests(), "observe.extract.selection").len(),
        1
    );
    assert!(!selection_test.artifact().exists());

    let extraction_test = TestRun::new("extraction-blocked");
    let extraction_factory = ScriptedFactory::new(|request| match request.context.as_str() {
        "observe.describe.frame" => generated(request, category("code", "none", true)),
        "observe.describe.code" => refused(request, true, true, Some("binary_missing")),
        _ => default_response(request),
    });
    assert!(matches!(
        run_decoded(
            extraction_test.options(false, Vec::new()),
            &extraction_factory,
            decoded(&[1])
        ),
        Err(RunError::Blocked(Some(code))) if code == "binary_missing"
    ));
    assert!(!extraction_test.artifact().exists());
}

#[test]
fn synthetic_decode_reenters_gaps_without_rewriting_clean_raw_rows() {
    let test = TestRun::new("reentry-gaps");
    let initial = ScriptedFactory::new(default_response);
    run_decoded(
        test.options(false, Vec::new()),
        &initial,
        decoded(&[1, 2, 3]),
    )
    .expect("initial artifact");

    let mut rows = test.rows();
    let mut header = rows.remove(0);
    header["_solstone_processing"]["state"] = json!("failed");
    header["_solstone_processing"]["reason_code"] = json!("analysis_failed");
    header["_solstone_processing"]["attempts"] = json!(1);
    rows[1]["analysis"] = Value::Null;
    rows[1]["enhanced"] = json!(false);
    rows[2]["enhanced"] = json!(true);
    rows[2]["content"] = json!({});
    rows[2]["error"] = json!("prior failure");
    let raw_reusable = "{\"timestamp\":0.0,\"analysis\":{\"visual_description\":\"kept\",\"primary\":\"code\",\"secondary\":\"none\",\"overlap\":true},\"enhanced\":false,\"frame_id\":1,\"requests\":[]}\n";
    let mut fixture = format!("{header}\n{raw_reusable}");
    for row in rows.into_iter().skip(1) {
        fixture.push_str(&format!("{row}\n"));
    }
    fs::write(test.artifact(), fixture).expect("write reentry fixture");

    let reentry = ScriptedFactory::new(default_response);
    run_decoded(
        test.options(false, Vec::new()),
        &reentry,
        decoded(&[1, 2, 3]),
    )
    .expect("reentry fills gaps");
    let requests = reentry.requests();
    assert!(
        phase_requests(&requests, "observe.describe.frame")
            .iter()
            .any(|request| request.id.as_deref() == Some("frame:2:attempt:0"))
    );
    assert!(
        phase_requests(&requests, "observe.describe.code")
            .iter()
            .any(|request| request.id.as_deref() == Some("extract:3:code:attempt:0"))
    );
    assert!(artifact_body(&test.artifact()).starts_with(raw_reusable));
    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "analyzed");
    assert!(rows[0]["_solstone_processing"].get("attempts").is_none());
    let third = rows
        .iter()
        .find(|row| row["frame_id"] == 3)
        .expect("third row");
    assert_eq!(third["content"]["code"], "# extracted markdown");
    assert!(third.get("error").is_none());

    rewrite_header(&test.artifact(), |header| {
        header["_solstone_processing"]["state"] = json!("failed");
        header["_solstone_processing"]["reason_code"] = json!("analysis_failed");
    });
    let redo = ScriptedFactory::new(default_response);
    run_decoded(test.options(true, Vec::new()), &redo, decoded(&[1, 2, 3]))
        .expect("redo starts a fresh run");
    assert_eq!(
        phase_requests(&redo.requests(), "observe.describe.frame").len(),
        3
    );
}
