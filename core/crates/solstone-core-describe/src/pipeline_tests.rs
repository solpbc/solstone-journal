// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

//! Private, in-process coverage for describe pipeline semantics.
//!
//! The `cli` integration harness retains the native decode, detector,
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
use crate::{
    WinnowConfig,
    selection::{CategoryOverride, Importance},
};

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
    let mut options = test.options(
        false,
        vec!["secret one".to_owned(), "secret two".to_owned()],
    );
    options.category_overrides.insert(
        "code".to_owned(),
        CategoryOverride {
            importance: Some(Importance::Normal),
            extraction: None,
        },
    );

    run_decoded(options, &factory, decoded(&[1, 2, 3])).expect("synthetic pipeline succeeds");

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
            && request.temperature == 0.0
            && request.max_output_tokens == 256
            && request
                .system_instruction
                .as_deref()
                .is_some_and(|instruction| instruction.ends_with("- secret one\n- secret two\n"))
    }));
    let selection = phase_requests(&requests, "observe.extract.selection");
    assert_eq!(selection.len(), 1);
    assert_eq!(selection[0].temperature, 0.3);
    assert_eq!(selection[0].max_output_tokens, 512);
    let extracts = phase_requests(&requests, "observe.describe.code");
    assert_eq!(extracts.len(), 3);
    assert!(extracts.iter().all(|request| !request.json_output));
}

#[test]
fn synthetic_decode_reads_a_single_frame_without_asking_the_selection_model() {
    let test = TestRun::new("single-frame");
    let factory = ScriptedFactory::new(default_response);

    run_decoded(test.options(false, Vec::new()), &factory, decoded(&[1]))
        .expect("synthetic pipeline succeeds");

    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "analyzed");
    assert_eq!(rows[1..].len(), 1);
    assert_eq!(rows[1]["enhanced"], true);
    assert_eq!(rows[1]["content"]["code"], "# extracted markdown");
    let requests = factory.requests();
    assert!(phase_requests(&requests, "observe.extract.selection").is_empty());
    assert_eq!(phase_requests(&requests, "observe.describe.code").len(), 1);
}

#[test]
fn synthetic_decode_never_reads_an_ignored_category_in_detail() {
    let test = TestRun::new("ignored-category");
    // Frame 1 is gaming (built-in importance ignore) and is kept as the first
    // frame; frame 2 is code with a visible gaming secondary.
    let factory = ScriptedFactory::new(|request| match request.id.as_deref() {
        Some(id) if id.starts_with("frame:1:") => {
            generated(request, category("gaming", "none", true))
        }
        Some(id) if id.starts_with("frame:2:") => {
            generated(request, category("code", "gaming", false))
        }
        _ => default_response(request),
    });

    run_decoded(test.options(false, Vec::new()), &factory, decoded(&[1, 2]))
        .expect("synthetic pipeline succeeds");

    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "analyzed");
    let row = |frame_id: u64| {
        rows[1..]
            .iter()
            .find(|row| row["frame_id"] == frame_id)
            .expect("row for frame")
    };
    assert_eq!(row(1)["analysis"]["primary"], "gaming");
    assert_eq!(row(1)["enhanced"], false);
    assert!(row(1).get("content").is_none());
    assert_eq!(row(2)["enhanced"], true);
    assert_eq!(row(2)["content"]["code"], "# extracted markdown");
    assert!(row(2)["content"].get("gaming").is_none());
    let requests = factory.requests();
    assert!(phase_requests(&requests, "observe.describe.gaming").is_empty());
    assert_eq!(phase_requests(&requests, "observe.describe.code").len(), 1);
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
fn failed_retry_keeps_a_partial_reading_and_still_exhausts() {
    let test = TestRun::new("keep-partial");
    run_decoded(
        test.options(false, Vec::new()),
        &ScriptedFactory::new(default_response),
        decoded(&[1, 2]),
    )
    .expect("initial reading");
    rewrite_header(&test.artifact(), |header| {
        header["_solstone_processing"]["state"] = json!("failed");
        header["_solstone_processing"]["reason_code"] = json!("analysis_failed");
        header["_solstone_processing"]["attempts"] = json!(1);
    });
    let body = artifact_body(&test.artifact());

    run_decoded(
        test.options(false, Vec::new()),
        &ScriptedFactory::new(default_response),
        DescribeResult {
            decode_failed: true,
            ..DescribeResult::default()
        },
    )
    .expect("a retry that cannot decode still completes");
    assert_eq!(artifact_body(&test.artifact()), body);
    let header = &test.rows()[0];
    assert_eq!(header["_solstone_processing"]["state"], "failed");
    assert_eq!(
        header["_solstone_processing"]["reason_code"],
        "analysis_failed"
    );
    assert_eq!(header["_solstone_processing"]["attempts"], 2);

    let invalid = ScriptedFactory::new(|request| generated(request, "not json"));
    assert!(matches!(
        run_decoded(
            test.options(false, Vec::new()),
            &invalid,
            decoded(&[1, 2, 3])
        ),
        Err(RunError::Internal(_))
    ));
    assert_eq!(artifact_body(&test.artifact()), body);
    assert_eq!(test.rows()[0]["_solstone_processing"]["attempts"], 3);

    let exhausted = ScriptedFactory::new(default_response);
    run_decoded(
        test.options(false, Vec::new()),
        &exhausted,
        decoded(&[1, 2]),
    )
    .expect("an exhausted reading is skipped");
    assert!(exhausted.requests().is_empty());
    assert_eq!(artifact_body(&test.artifact()), body);

    let unread = TestRun::new("replace-unread");
    fs::write(
        unread.artifact(),
        "{\"_solstone_processing\":{\"state\":\"failed\",\"reason_code\":\"analysis_failed\",\"handler\":\"describe\",\"attempts\":1}}\n{\"frame_id\":1,\"timestamp\":0.0,\"requests\":[],\"error\":\"prior failure\",\"enhanced\":false}\n",
    )
    .expect("write unread fixture");
    run_decoded(
        unread.options(false, Vec::new()),
        &ScriptedFactory::new(default_response),
        DescribeResult {
            decode_failed: true,
            ..DescribeResult::default()
        },
    )
    .expect("corrupt retry promotes");
    let rows = unread.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["_solstone_processing"]["reason_code"],
        "corrupt_input"
    );
}

#[test]
fn synthetic_decode_preserves_category_contracts_and_extraction_outcomes() {
    for (primary, context, tokens, json_output) in [
        ("browsing", "observe.describe.browsing", 3072, false),
        ("messaging", "observe.describe.messaging", 9728, true),
        ("meeting", "observe.describe.meeting", 3840, true),
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
            decoded(&[1, 2])
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
    let code_overrides = BTreeMap::from([(
        "code".to_owned(),
        CategoryOverride {
            importance: Some(Importance::Normal),
            extraction: None,
        },
    )]);
    let mut initial_options = test.options(false, Vec::new());
    initial_options.category_overrides = code_overrides.clone();
    run_decoded(initial_options, &initial, decoded(&[1, 2, 3])).expect("initial artifact");

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
    let mut reentry_options = test.options(false, Vec::new());
    reentry_options.category_overrides = code_overrides.clone();
    run_decoded(reentry_options, &reentry, decoded(&[1, 2, 3])).expect("reentry fills gaps");
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
    let mut redo_options = test.options(true, Vec::new());
    redo_options.category_overrides = code_overrides;
    run_decoded(redo_options, &redo, decoded(&[1, 2, 3])).expect("redo starts a fresh run");
    assert_eq!(
        phase_requests(&redo.requests(), "observe.describe.frame").len(),
        3
    );
}

fn oracle_fixture(category_name: &str, body_json: &str) -> (String, Option<Value>) {
    let category = crate::categories::CATEGORIES_META
        .iter()
        .find(|c| c.name == category_name)
        .unwrap_or_else(|| panic!("category {category_name} not found in CATEGORIES_META"));
    let schema_text = category
        .schema
        .unwrap_or_else(|| panic!("category {category_name} has no schema"));
    let schema: Value = serde_json::from_str(schema_text).expect("valid schema JSON");
    let result = solstone_core_generate_wire::validate_schema_with_annotations(body_json, &schema);
    (result.text, Some(result.validation))
}

fn invalid_calendar_json() -> String {
    json!({
        "app": "Google Calendar",
        "view": "agenda",
        "range": "2026-09-14",
        "events": [
            {
                "title": "Planning Sync",
                "start": "2026-09-14T09:00:00Z",
                "end": "2026-09-14T10:00:00Z",
                "location": "Room 101",
                "conferencing": null,
                "guests": [
                    "g1@example.com","g2@example.com","g3@example.com","g4@example.com","g5@example.com",
                    "g6@example.com","g7@example.com","g8@example.com","g9@example.com","g10@example.com",
                    "g11@example.com","g12@example.com","g13@example.com","g14@example.com","g15@example.com",
                    "g16@example.com","g17@example.com","g18@example.com","g19@example.com","g20@example.com",
                    "g21@example.com"
                ],
                "status": "accepted",
                "recurrence": null,
                "calendar": "Work",
                "description": "Team Sync"
            }
        ],
        "availability": []
    })
    .to_string()
}

fn valid_calendar_json() -> String {
    json!({
        "app": "Google Calendar",
        "view": "agenda",
        "range": "2026-09-14",
        "events": [],
        "availability": [],
        "notes": "No upcoming events"
    })
    .to_string()
}

fn invalid_messaging_json() -> String {
    let invalid_messages: Vec<Value> = (0..61)
        .map(|i| {
            json!({
                "sender": format!("user{i}@example.com"),
                "timestamp": "2026-09-14T09:00:00Z",
                "subject": null,
                "text": "hello"
            })
        })
        .collect();
    json!({
        "app": "Slack",
        "thread": "project-updates",
        "messages": invalid_messages
    })
    .to_string()
}

#[test]
fn synthetic_decode_schema_invalid_extraction_fails_terminal_without_retries() {
    let test = TestRun::new("schema-invalid-extraction");
    let (body, validation) = oracle_fixture("calendar", &invalid_calendar_json());
    let factory = ScriptedFactory::new(move |request| match request.context.as_str() {
        "observe.describe.frame" => generated(request, category("calendar", "none", true)),
        "observe.describe.calendar" => {
            generated_with_finish(request, &body, "stop", validation.clone())
        }
        _ => default_response(request),
    });
    run_decoded(test.options(false, Vec::new()), &factory, decoded(&[1]))
        .expect("pipeline completes and latches row failure");

    let requests = factory.requests();
    let calendar_requests = phase_requests(&requests, "observe.describe.calendar");
    assert_eq!(
        calendar_requests.len(),
        1,
        "schema validation failure must be terminal without retries"
    );

    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    assert_eq!(
        rows[0]["_solstone_processing"]["reason_code"],
        "analysis_failed"
    );

    let frame = rows
        .iter()
        .find(|row| row["frame_id"] == 1)
        .expect("frame 1");
    assert!(
        frame
            .get("content")
            .and_then(|c| c.get("calendar"))
            .is_none(),
        "invalid schema extraction must not be written to content"
    );
    let error = frame["error"].as_str().expect("error string present");
    assert!(
        error.starts_with("Schema validation failed for calendar:"),
        "error message must name the category: {error}"
    );
    assert!(
        error.contains("required"),
        "error must mention required constraint: {error}"
    );
    assert!(
        error.contains("maxItems") && error.contains("/events/0/guests"),
        "error must contain nested bound constraint and path: {error}"
    );
}

#[test]
fn synthetic_decode_schema_valid_advisory_writes_content_successfully() {
    let test = TestRun::new("schema-valid-advisory");
    let (body, validation) = oracle_fixture("calendar", &valid_calendar_json());
    assert_eq!(
        validation.as_ref().and_then(|v| v.get("valid")),
        Some(&Value::Bool(true))
    );
    let factory = ScriptedFactory::new(move |request| match request.context.as_str() {
        "observe.describe.frame" => generated(request, category("calendar", "none", true)),
        "observe.describe.calendar" => {
            generated_with_finish(request, &body, "stop", validation.clone())
        }
        _ => default_response(request),
    });
    run_decoded(test.options(false, Vec::new()), &factory, decoded(&[1]))
        .expect("pipeline completes successfully");

    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "analyzed");
    let frame = rows
        .iter()
        .find(|row| row["frame_id"] == 1)
        .expect("frame 1");
    assert!(frame.get("error").is_none());
    assert_eq!(frame["content"]["calendar"]["app"], "Google Calendar");
}

#[test]
fn synthetic_decode_absent_schema_validation_accepts_json_content() {
    let test = TestRun::new("absent-schema-validation");
    let factory = ScriptedFactory::new(|request| match request.context.as_str() {
        "observe.describe.frame" => generated(request, category("messaging", "none", true)),
        _ => default_response(request),
    });
    run_decoded(test.options(false, Vec::new()), &factory, decoded(&[1]))
        .expect("absent schema_validation accepts");

    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "analyzed");
    let frame = rows
        .iter()
        .find(|row| row["frame_id"] == 1)
        .expect("frame 1");
    assert!(frame.get("error").is_none());
    assert_eq!(frame["content"]["messaging"]["ok"], true);
}

#[test]
fn synthetic_decode_mixed_success_and_schema_failure_keeps_success_content_and_latches_error() {
    let test = TestRun::new("mixed-success-schema-fail");
    let (calendar_body, calendar_val) = oracle_fixture("calendar", &invalid_calendar_json());
    let factory = ScriptedFactory::new(move |request| match request.context.as_str() {
        "observe.describe.frame" => generated(request, category("code", "calendar", false)),
        "observe.describe.code" => generated(request, "# markdown code extracted"),
        "observe.describe.calendar" => {
            generated_with_finish(request, &calendar_body, "stop", calendar_val.clone())
        }
        _ => default_response(request),
    });
    run_decoded(test.options(false, Vec::new()), &factory, decoded(&[1]))
        .expect("pipeline completes with mixed row outcome");

    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    let frame = rows
        .iter()
        .find(|row| row["frame_id"] == 1)
        .expect("frame 1");
    assert_eq!(frame["content"]["code"], "# markdown code extracted");
    assert!(
        frame
            .get("content")
            .and_then(|c| c.get("calendar"))
            .is_none(),
        "failed category must be absent from content"
    );
    let error = frame["error"].as_str().expect("error string present");
    assert!(
        error.starts_with("Schema validation failed for calendar:"),
        "error must be calendar schema failure: {error}"
    );
}

#[test]
fn synthetic_decode_double_schema_failure_latches_first_error_and_suppresses_second() {
    let test = TestRun::new("double-schema-fail");
    let (calendar_body, calendar_val) = oracle_fixture("calendar", &invalid_calendar_json());
    let (messaging_body, messaging_val) = oracle_fixture("messaging", &invalid_messaging_json());
    let factory = ScriptedFactory::new(move |request| match request.context.as_str() {
        "observe.describe.frame" => generated(request, category("calendar", "messaging", false)),
        "observe.describe.calendar" => {
            generated_with_finish(request, &calendar_body, "stop", calendar_val.clone())
        }
        "observe.describe.messaging" => {
            generated_with_finish(request, &messaging_body, "stop", messaging_val.clone())
        }
        _ => default_response(request),
    });
    run_decoded(test.options(false, Vec::new()), &factory, decoded(&[1]))
        .expect("pipeline completes with double schema failure");

    let rows = test.rows();
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    let frame = rows
        .iter()
        .find(|row| row["frame_id"] == 1)
        .expect("frame 1");
    assert!(
        frame
            .get("content")
            .and_then(|c| c.get("calendar"))
            .is_none(),
        "calendar must be absent from content"
    );
    assert!(
        frame
            .get("content")
            .and_then(|c| c.get("messaging"))
            .is_none(),
        "messaging must be absent from content"
    );
    let error = frame["error"].as_str().expect("error string present");
    assert!(
        error.starts_with("Schema validation failed for calendar:"),
        "error must latch first category failure (calendar): {error}"
    );
    assert!(
        error.contains("/events/0/guests"),
        "error must contain first category's nested constraint: {error}"
    );
    assert!(
        !error.contains("/messages"),
        "error must suppress second category's error details: {error}"
    );
}

#[test]
fn writer_commit_admits_unchanged_success_and_empty_but_refuses_stale_or_deleted_input() {
    use solstone_core_journal_io::{LockOptions, hold_lock};
    for frames in [vec![], vec![1]] {
        for mutation in ["unchanged", "bytes", "deleted", "missing", "replaced"] {
            let run = TestRun::new("writer-commit");
            let factory = ScriptedFactory::new(default_response);
            let result = super::run_with_decoder(run.options(false, Vec::new()), &factory, || {
                let options = LockOptions {
                    timeout: Duration::ZERO,
                    ..LockOptions::default()
                };
                // The whole attempt excludes another writer; inference/decode
                // does not retain the short segment mutation lock.
                assert!(hold_lock(run.artifact(), options).is_err());
                assert!(hold_lock(&run.root, options).is_ok());
                match mutation {
                    "bytes" => {
                        let mut bytes = fs::read(&run.video).unwrap();
                        bytes[0] ^= 1;
                        fs::write(&run.video, bytes).unwrap();
                    }
                    "deleted" => fs::write(run.root.join("tombstone.json"), b"{}").unwrap(),
                    "missing" => fs::remove_file(&run.video).unwrap(),
                    "replaced" => {
                        let bytes = fs::read(&run.video).unwrap();
                        fs::remove_file(&run.video).unwrap();
                        fs::write(&run.video, bytes).unwrap();
                    }
                    _ => (),
                }
                decoded(&frames)
            });
            if mutation == "unchanged" {
                result.unwrap();
                let rows = run.rows();
                assert_eq!(rows[0]["_solstone_processing"]["handler"], "describe");
                assert_eq!(
                    rows[0]["_solstone_processing"]["state"],
                    if frames.is_empty() {
                        "empty"
                    } else {
                        "analyzed"
                    }
                );
            } else {
                assert!(result.is_err(), "{mutation}");
                assert!(!run.artifact().exists(), "{mutation}");
            }
        }
    }
}

#[test]
fn owner_retry_reaches_describe_writer_and_keeps_retained_partial_rows() {
    use solstone_core_processing_record::vocab;
    use solstone_core_system::work_retry::{self, raw};
    if std::env::var_os("DESCRIBE_RETRY_TEST_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "pipeline::pipeline_tests::owner_retry_reaches_describe_writer_and_keeps_retained_partial_rows", "--nocapture"])
            .env("DESCRIBE_RETRY_TEST_CHILD", "1").output().unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        assert!(
            String::from_utf8_lossy(&status.stdout).contains("1 passed"),
            "{}",
            String::from_utf8_lossy(&status.stdout)
        );
        return;
    }
    let mut run = TestRun::new("owner-retry");
    run.video = run
        .journal
        .join("chronicle")
        .join("20260101")
        .join("desktop")
        .join("120000_300")
        .join("screen.webm");
    fs::create_dir_all(run.video.parent().unwrap()).unwrap();
    fs::write(&run.video, "synthetic video bytes").unwrap();
    let factory = ScriptedFactory::new(default_response);
    let header = json!({"raw":"screen.webm","_solstone_processing":{"schema":"solstone.processing.v1","state":"failed","handler":"describe","reason_code":"analysis_failed","attempts":3}});
    fs::write(run.artifact(), format!("{header}\n")).unwrap();
    // Use the owning vocabulary, so this fixture detects schema drift.
    rewrite_header(&run.artifact(), |h| {
        h["_solstone_processing"]["schema"] = json!(vocab::SCHEMA)
    });
    run_decoded(run.options(false, vec![]), &factory, decoded(&[1])).unwrap();
    assert!(factory.requests().is_empty());
    let evidence = raw::inspect_output(&run.journal, &run.artifact())
        .unwrap()
        .unwrap();
    raw::observe(&run.journal, &evidence).unwrap();
    work_retry::request(&run.journal, &evidence.unit).unwrap();
    let id = evidence.unit.id().unwrap();
    let attempt = work_retry::read(&run.journal, &id)
        .unwrap()
        .unwrap()
        .permission(&evidence.input, &[])
        .unwrap();
    temp_env::with_vars(
        [
            (raw::JOURNAL_ENV, Some(run.journal.to_str().unwrap())),
            (raw::UNIT_ENV, Some(id.as_str())),
            (raw::CAUSE_ENV, Some(attempt.cause.as_str())),
            (
                raw::ATTEMPT_ENV,
                Some(serde_json::to_string(&attempt).unwrap().as_str()),
            ),
        ],
        || {
            run_decoded(run.options(false, vec![]), &factory, decoded(&[1])).unwrap();
            assert!(!factory.requests().is_empty());
        },
    );
    assert_eq!(run.rows()[0]["_solstone_processing"]["state"], "analyzed");
    raw::finish_output(&run.journal, &evidence.unit, &attempt).unwrap();
    assert!(!work_retry::read(&run.journal, &id).unwrap().unwrap().held);
    let calls = factory.requests().len();
    run_decoded(run.options(false, vec![]), &factory, decoded(&[1])).unwrap();
    assert_eq!(factory.requests().len(), calls);
    // Main's retained-partial-reading path must publish this attempt's receipt
    // too, without replacing its rows with an empty decode failure.
    let body = artifact_body(&run.artifact());
    rewrite_header(&run.artifact(), |h| {
        let record = &mut h["_solstone_processing"];
        record["state"] = json!("failed");
        record["reason_code"] = json!("analysis_failed");
        record["attempts"] = json!(3);
    });
    let evidence = raw::inspect_output(&run.journal, &run.artifact())
        .unwrap()
        .unwrap();
    raw::observe(&run.journal, &evidence).unwrap();
    work_retry::request(&run.journal, &evidence.unit).unwrap();
    let attempt = work_retry::read(&run.journal, &id)
        .unwrap()
        .unwrap()
        .permission(&evidence.input, &[])
        .unwrap();
    temp_env::with_vars(
        [
            (raw::JOURNAL_ENV, Some(run.journal.to_str().unwrap())),
            (raw::UNIT_ENV, Some(id.as_str())),
            (raw::CAUSE_ENV, Some(attempt.cause.as_str())),
            (
                raw::ATTEMPT_ENV,
                Some(serde_json::to_string(&attempt).unwrap().as_str()),
            ),
        ],
        || {
            run_decoded(
                run.options(false, vec![]),
                &factory,
                DescribeResult {
                    decode_failed: true,
                    ..DescribeResult::default()
                },
            )
            .unwrap();
        },
    );
    assert_eq!(artifact_body(&run.artifact()), body);
    raw::finish_output(&run.journal, &evidence.unit, &attempt).unwrap();
    let record = work_retry::read(&run.journal, &id).unwrap().unwrap();
    assert!(record.held);
    assert!(record.attempt.is_none());
}

#[test]
fn ocr_saved_record_and_faults_preserve_actual_pipeline_requests_and_verdicts() {
    use crate::screen_ocr::{OcrFailure, OcrWorker, OcrWorkerFactory, RawOccurrence};
    struct Worker {
        failure: Option<OcrFailure>,
        ordinal: u64,
    }
    impl OcrWorker for Worker {
        fn recognize(&mut self, _: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure> {
            if let Some(failure) = self.failure {
                return Err(failure);
            }
            self.ordinal += 1;
            Ok(vec![RawOccurrence {
                line_order: 0,
                text: format!("extra literal {}", self.ordinal),
                confidence: Some(0.9),
                geometry: Vec::new(),
                recurrence: 0,
            }])
        }
    }
    struct Ocr {
        available: bool,
        failure: Option<OcrFailure>,
    }
    impl OcrWorkerFactory for Ocr {
        fn recipe_id(&mut self) -> String {
            "authored-test-recipe".to_owned()
        }
        fn create(&mut self) -> Result<Option<Box<dyn OcrWorker>>, OcrFailure> {
            Ok(self.available.then(|| {
                Box::new(Worker {
                    failure: self.failure,
                    ordinal: 0,
                }) as Box<dyn OcrWorker>
            }))
        }
    }
    fn rows(test: &TestRun) -> Vec<Value> {
        if !test.artifact().exists() {
            return Vec::new();
        }
        test.rows().into_iter().map(|row| {
            if row.get("_solstone_processing").is_some() {
                json!({"state":row["_solstone_processing"]["state"], "reason":row["_solstone_processing"]["reason_code"]})
            } else {
                json!({"frame_id":row["frame_id"],"timestamp":row["timestamp"],"analysis":row["analysis"],"content":row["content"],"enhanced":row["enhanced"],"error":row["error"],"pending":row["pending"]})
            }
        }).collect()
    }
    for mode in [
        "success",
        "all-c-failed",
        "provider-blocked",
        "extraction-blocked",
    ] {
        let mut baseline = None;
        for (available, failure) in [
            (false, None),
            (true, None),
            (true, Some(OcrFailure::Protocol)),
            (true, Some(OcrFailure::Timeout)),
        ] {
            let mut test = TestRun::new("ocr-paired");
            let segment = test.journal.join("chronicle/20261020/desktop/090000_300");
            fs::create_dir_all(&segment).unwrap();
            test.video = segment.join("a_screen.webm");
            fs::write(&test.video, "authored raw input").unwrap();
            let factory = ScriptedFactory::new(move |request| {
                if mode == "provider-blocked"
                    || (mode == "extraction-blocked"
                        && request.context.starts_with("observe.describe.")
                        && request.context != "observe.describe.frame")
                {
                    return no_engine(request);
                }
                if mode == "all-c-failed" && request.context == "observe.describe.frame" {
                    return refused(request, false, false, None);
                }
                default_response(request)
            });
            let mut options = test.options(false, vec!["model-only redaction rule".to_owned()]);
            options.max_extractions = 1;
            let outcome = super::run_decoded_with_ocr_factory(
                options,
                &factory,
                decoded(&[1, 2, 3, 4]),
                &mut Ocr { available, failure },
            );
            let observation = (
                outcome.as_ref().err().map(|error| match error {
                    RunError::Blocked(_) => "blocked",
                    RunError::Internal(_) => "internal",
                }),
                factory.requests(),
                rows(&test),
            );
            if let Some(expected) = &baseline {
                assert_eq!(&observation, expected, "mode {mode}");
            } else {
                baseline = Some(observation);
            }
            let public = segment.join("talents/screen_text.md");
            if available && failure.is_none() {
                let record = fs::read_to_string(public).unwrap();
                assert!(
                    record.contains("extra literal 4"),
                    "even non-E/failed-C frames contribute in {mode}"
                );
                assert!(!record.contains("model-only redaction rule"));
                if mode == "extraction-blocked" {
                    let sources = crate::screen_ocr::load_segment_sidecars(&segment);
                    assert_eq!(sources.len(), 1);
                    assert_eq!(
                        sources[0].frames[0].word_overlap.extraction,
                        crate::screen_ocr::ExtractionTextState::Failed
                    );
                    assert_eq!(
                        sources[0].frames[1].word_overlap.extraction,
                        crate::screen_ocr::ExtractionTextState::NotReached
                    );
                }
                let pending = solstone_core_indexer_todo::inspect(&test.journal).unwrap();
                assert_eq!(
                    pending.keys,
                    vec!["20261020/desktop/090000_300/talents/screen_text.md"]
                );
            } else {
                assert!(!public.exists());
            }
        }
    }
}
