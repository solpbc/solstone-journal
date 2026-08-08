// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core-describe");
const SESSION_STUB: &str = env!("CARGO_BIN_EXE_solstone-describe-session-stub");
const DETECT_STUB: &str = env!("CARGO_BIN_EXE_solstone-describe-detect-stub");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    file: String,
    frames: Vec<FixtureFrame>,
    height: Option<u32>,
    width: Option<u32>,
}

#[derive(Deserialize)]
struct FixtureFrame {
    frame_id: u64,
    timestamp: f64,
}

fn corpus_path(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/describe_corpus")
        .join(file)
}

fn masked_corpus_path(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/describe_masked")
        .join(file)
}

fn fixture_case(file: &str) -> FixtureCase {
    let fixture: Fixture =
        serde_json::from_str(include_str!("../../../fixtures/describe_frames.json"))
            .expect("valid describe fixture");
    fixture
        .cases
        .into_iter()
        .find(|case| case.file == file)
        .expect("fixture case")
}

fn frames_only(file: &str) -> Command {
    let mut command = Command::new(BINARY);
    command.arg("--frames-only").arg(corpus_path(file));
    command
}

fn temporary_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "solstone-describe-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("create temporary root");
    path
}

fn copied_video(root: &Path, file: &str) -> PathBuf {
    let video = root.join(file);
    fs::copy(corpus_path(file), &video).expect("copy test video");
    video
}

fn describe(root: &Path, video: &Path, mode: &str) -> Command {
    let mut command = Command::new(BINARY);
    command
        .arg("--describe")
        .arg(video)
        .arg("--journal")
        .arg(root)
        .arg("-j")
        .arg("2");
    command.env("SOLSTONE_DESCRIBE_GENERATE_WIRE", SESSION_STUB);
    command.env("SOLSTONE_DESCRIBE_SESSION_STUB_MODE", mode);
    command
}

fn describe_redo(root: &Path, video: &Path, mode: &str) -> Command {
    let mut command = describe(root, video, mode);
    command.arg("--redo");
    command
}

fn rewrite_header(path: &Path, update: impl FnOnce(&mut Value)) {
    let contents = fs::read_to_string(path).expect("read artifact");
    let (header, rows) = contents.split_once('\n').expect("header newline");
    let mut header: Value = serde_json::from_str(header).expect("header JSON");
    update(&mut header);
    fs::write(path, format!("{header}\n{rows}")).expect("write artifact");
}

fn mark_for_reentry(path: &Path, attempts: i64) {
    rewrite_header(path, |header| {
        header["_solstone_processing"]["state"] = json!("failed");
        header["_solstone_processing"]["reason_code"] = json!("analysis_failed");
        header["_solstone_processing"]["attempts"] = json!(attempts);
    });
}

fn jsonl_body(path: &Path) -> String {
    fs::read_to_string(path)
        .expect("read artifact")
        .split_once('\n')
        .expect("header newline")
        .1
        .to_owned()
}

fn read_jsonl(path: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .expect("read JSONL")
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid JSONL row"))
        .collect()
}

fn selection_requests(path: &Path) -> Vec<serde_json::Value> {
    read_jsonl(path)
        .into_iter()
        .filter(|request| request["context"] == "observe.extract.selection")
        .collect()
}

fn extraction_requests(path: &Path) -> Vec<serde_json::Value> {
    read_jsonl(path)
        .into_iter()
        .filter(|request| {
            let context = request["context"].as_str().unwrap_or_default();
            context.starts_with("observe.describe.") && context != "observe.describe.frame"
        })
        .collect()
}

fn detector_requests(path: &Path) -> Vec<serde_json::Value> {
    read_jsonl(path)
}

fn no_temp_files(root: &Path) -> bool {
    fs::read_dir(root).expect("read root").all(|entry| {
        !entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
    })
}

fn remove_temporary_root(root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match fs::remove_dir_all(root) {
            Ok(()) => return,
            Err(error)
                if Instant::now() < deadline
                    && error.kind() == std::io::ErrorKind::DirectoryNotEmpty =>
            {
                thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("remove temporary root: {error}"),
        }
    }
}

fn notification_listener(root: &Path) -> UnixListener {
    let health = root.join("health");
    fs::create_dir_all(&health).expect("create health directory");
    UnixListener::bind(health.join("callosum.sock")).expect("bind callosum socket")
}

fn notification(listener: &UnixListener) -> serde_json::Value {
    let (mut stream, _) = listener.accept().expect("accept notification");
    let mut line = String::new();
    stream.read_to_string(&mut line).expect("read notification");
    serde_json::from_str(line.trim()).expect("valid notification JSON")
}

#[test]
fn frames_only_matches_the_frozen_oracle() {
    let case = fixture_case("mixed_vp8_screen.webm");
    let output = frames_only(&case.file)
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON output");
    assert_eq!(actual["width"], serde_json::json!(case.width));
    assert_eq!(actual["height"], serde_json::json!(case.height));
    let expected_frames: Vec<serde_json::Value> = case
        .frames
        .into_iter()
        .map(|frame| serde_json::json!({"frame_id": frame.frame_id, "timestamp": frame.timestamp}))
        .collect();
    assert_eq!(actual["frames"], serde_json::json!(expected_frames));
}

#[test]
fn describe_uses_convey_mask_for_live_handler_decode() {
    for (file, expected_requests) in [
        ("convey_masked_inside_screen.webm", 1),
        ("convey_masked_outside_screen.webm", 7),
        ("convey_skipped_screen.webm", 0),
    ] {
        let root = temporary_root(file);
        let video = root.join(file);
        fs::copy(masked_corpus_path(file), &video).expect("copy masked corpus video");
        let request_log = root.join("requests.jsonl");
        let output = describe(&root, &video, "generated")
            .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
            .output()
            .expect("describe");
        assert!(output.status.success(), "{file}");
        let requests = if request_log.exists() {
            read_jsonl(&request_log)
                .into_iter()
                .filter(|request| request["context"] == "observe.describe.frame")
                .count()
        } else {
            0
        };
        assert_eq!(requests, expected_requests, "{file}");
        let rows = read_jsonl(&video.with_extension("jsonl"));
        if expected_requests == 0 {
            assert_eq!(rows[0]["_solstone_processing"]["state"], "empty");
            assert_eq!(
                rows[0]["_solstone_processing"]["reason_code"],
                "no_decodable_frames"
            );
        }
        fs::remove_dir_all(root).expect("remove root");
    }
}

#[test]
fn explicit_empty_journal_uses_defaults() {
    let journal =
        std::env::temp_dir().join(format!("solstone-describe-cli-{}", std::process::id()));
    fs::create_dir(&journal).expect("create temporary journal");
    let output = frames_only("mixed_vp8_screen.webm")
        .arg("--journal")
        .arg(&journal)
        .output()
        .expect("run describe binary");
    fs::remove_dir(&journal).expect("remove temporary journal");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn malformed_invocation_is_a_usage_error() {
    let output = Command::new(BINARY).output().expect("run describe binary");
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn bare_video_path_defaults_to_describe_and_accepts_dispatcher_verbosity_flags() {
    let root = temporary_root("bare-describe");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let output = Command::new(BINARY)
        .arg(&video)
        .arg("--journal")
        .arg(&root)
        .arg("-j")
        .arg("2")
        .arg("-d")
        .arg("-v")
        .env("SOLSTONE_DESCRIBE_GENERATE_WIRE", SESSION_STUB)
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_MODE", "generated")
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(video.with_extension("jsonl").exists());
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn categories_mode_prints_the_native_category_registry() {
    let output = Command::new(BINARY)
        .arg("--categories")
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let registry: Value =
        serde_json::from_slice(&output.stdout).expect("categories output is valid JSON");
    assert_eq!(registry["schema"], json!("solstone-describe-categories-v1"));
    assert_eq!(registry["default_max_extractions"], json!(20));
    let categories = registry["categories"]
        .as_object()
        .expect("categories object");
    assert_eq!(categories.len(), 11);
    let names: Vec<_> = categories.keys().collect();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    for (name, metadata) in categories {
        let metadata = metadata.as_object().expect("category metadata object");
        for field in [
            "description",
            "output",
            "max_output_tokens",
            "context",
            "label",
            "group",
        ] {
            assert!(metadata.contains_key(field), "{name} missing {field}");
        }
        assert_eq!(
            metadata["context"],
            json!(format!("observe.describe.{name}"))
        );
        assert_eq!(metadata["group"], json!("Screen Analysis"));
        assert_eq!(
            metadata["label"],
            json!(
                name.split('_')
                    .map(|word| {
                        let mut chars = word.chars();
                        let first = chars.next().expect("nonempty category word");
                        first.to_uppercase().collect::<String>() + chars.as_str()
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        );
    }
    assert_eq!(categories["calendar"]["importance"], json!("high"));
    assert!(categories["browsing"].get("importance").is_none());
    for name in ["calendar", "meeting", "messaging"] {
        assert!(categories[name].get("json_schema").is_some(), "{name}");
    }
    for (name, metadata) in categories {
        if !["calendar", "meeting", "messaging"].contains(&name.as_str()) {
            assert!(metadata.get("json_schema").is_none(), "{name}");
        }
    }
}

#[test]
fn version_names_libavcodec() {
    let output = Command::new(BINARY)
        .arg("--version")
        .output()
        .expect("run describe binary");
    assert!(output.status.success());
    let version = String::from_utf8(output.stdout).expect("UTF-8 version output");
    assert!(version.contains("libavcodec "));
    assert_ne!(version.trim(), "libavcodec");
}

#[test]
fn decode_failures_use_exit_code_two() {
    for file in [
        "audio_only_screen.mov",
        "not_a_video_screen.webm",
        "corrupted_mid_screen.webm",
    ] {
        let output = frames_only(file).output().expect("run describe binary");
        assert_eq!(output.status.code(), Some(2), "{file}");
        assert!(output.stdout.is_empty(), "{file}");
    }
}

#[test]
fn describe_runs_one_session_and_promotes_an_analyzed_artifact() {
    let root = temporary_root("generated");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let pid_path = root.join("child.pid");
    let output = describe(&root, &video, "generated")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_PID_PATH", &pid_path)
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(pid_path.exists(), "the one generate child records its pid");
    let rows = read_jsonl(&video.with_extension("jsonl"));
    assert_eq!(rows[0]["_solstone_processing"]["state"], "analyzed");
    assert_eq!(rows[0]["_solstone_processing"]["reason_code"], "ok");
    assert_eq!(rows[0]["_solstone_thinking"]["model"], "describe-stub");
    assert!(
        rows[1..]
            .iter()
            .all(|row| row.get("finish_reason").is_none())
    );
    assert!(no_temp_files(&root));
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn retry_uses_a_fresh_id_and_attempt_index() {
    let root = temporary_root("retry");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    let request_log = root.join("requests.jsonl");
    let output = describe(&root, &video, "retryable_then_generated")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = read_jsonl(&request_log);
    let frame_id = fixture_case("mixed_vp8_screen.webm").frames[0].frame_id;
    assert!(
        requests
            .iter()
            .any(|row| row["id"] == format!("frame:{frame_id}:attempt:0")
                && row["attempt_index"] == 0)
    );
    assert!(
        requests
            .iter()
            .any(|row| row["id"] == format!("frame:{frame_id}:attempt:1")
                && row["attempt_index"] == 1)
    );
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn blocking_or_unknown_refusals_abort_without_an_artifact() {
    for mode in ["blocking_retryable", "unknown_code"] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "mixed_vp8_screen.webm");
        let request_log = root.join("requests.jsonl");
        let output = describe(&root, &video, mode)
            .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
            .output()
            .expect("run describe binary");
        assert_eq!(output.status.code(), Some(69), "{mode}");
        assert!(!video.with_extension("jsonl").exists(), "{mode}");
        assert!(no_temp_files(&root), "{mode}");
        let first_frame = fixture_case("mixed_vp8_screen.webm").frames[0].frame_id;
        let request_text = fs::read_to_string(&request_log).expect("read request log");
        assert_eq!(
            request_text
                .matches(&format!("\"id\":\"frame:{first_frame}:attempt:0\""))
                .count(),
            1,
            "{mode} must not retry the blocking response"
        );
        remove_temporary_root(&root);
    }
}

#[test]
fn fresh_all_failed_promotes_then_returns_an_error() {
    let root = temporary_root("non-responsive");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let output = describe(&root, &video, "non_responsive")
        .output()
        .expect("run describe binary");
    assert!(!output.status.success());
    let rows = read_jsonl(&video.with_extension("jsonl"));
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    assert_eq!(
        rows[0]["_solstone_processing"]["reason_code"],
        "analysis_failed"
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["_solstone_processing"]["attempts"], 1);
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn launch_failure_is_blocked_not_empty() {
    let root = temporary_root("launch-failure");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let output = Command::new(BINARY)
        .arg("--describe")
        .arg(&video)
        .arg("--journal")
        .arg(&root)
        .env(
            "SOLSTONE_DESCRIBE_GENERATE_WIRE",
            root.join("does-not-exist"),
        )
        .output()
        .expect("run describe binary");
    assert_eq!(output.status.code(), Some(69));
    assert!(!video.with_extension("jsonl").exists());
    assert!(no_temp_files(&root));
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn segment_meta_is_merged_before_describe_owned_values() {
    let root = temporary_root("metadata");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let meta = serde_json::json!({
        "raw": "meta-raw", "observer": "meta-observer", "qualified_count": 999,
        "_solstone_thinking": {"model":"meta"}, "_solstone_processing": {"state":"meta"}
    });
    let output = describe(&root, &video, "generated")
        .env("OBSERVER_NAME", "observer-name")
        .env("SEGMENT_META", meta.to_string())
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = read_jsonl(&video.with_extension("jsonl"));
    let header = &rows[0];
    assert_eq!(header["raw"], "meta-raw");
    assert_eq!(header["observer"], "meta-observer");
    assert_ne!(header["qualified_count"], 999);
    assert_eq!(header["_solstone_thinking"]["model"], "describe-stub");
    assert_eq!(header["_solstone_processing"]["state"], "analyzed");
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn retryable_refusals_stop_after_five_attempts() {
    let root = temporary_root("retry-exhaustion");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let request_log = root.join("requests.jsonl");
    let output = describe(&root, &video, "always_retryable")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .output()
        .expect("run describe binary");
    assert!(!output.status.success());
    let requests = read_jsonl(&request_log);
    let requests = requests
        .into_iter()
        .filter(|request| request["context"] == "observe.describe.frame")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests
            .iter()
            .map(|row| row["attempt_index"].as_u64())
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2), Some(3), Some(4)]
    );
    let rows = read_jsonl(&video.with_extension("jsonl"));
    assert_eq!(rows.len(), 1);
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn no_engine_and_session_child_failures_abort_without_artifacts() {
    for (mode, file) in [
        ("no_engine_configured", "single_frame_vp8_screen.webm"),
        ("exit_after_all", "mixed_vp8_screen.webm"),
        ("exit_after_one", "mixed_vp8_screen.webm"),
    ] {
        let root = temporary_root(mode);
        let video = copied_video(&root, file);
        let output = describe(&root, &video, mode)
            .output()
            .expect("run describe binary");
        assert_eq!(
            output.status.code(),
            Some(69),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!video.with_extension("jsonl").exists(), "{mode}");
        assert!(no_temp_files(&root), "{mode}");
        fs::remove_dir_all(root).expect("remove temporary root");
    }
}

#[test]
fn processing_record_is_complete_and_decode_failure_has_no_thinking() {
    let root = temporary_root("record");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let output = describe(&root, &video, "generated")
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = read_jsonl(&video.with_extension("jsonl"));
    let record = &rows[0]["_solstone_processing"];
    assert_eq!(record["schema"], "solstone.processing.v1");
    assert_eq!(record["handler"], "describe");
    chrono::DateTime::parse_from_rfc3339(record["attempted_at"].as_str().expect("timestamp"))
        .expect("RFC 3339 timestamp");
    assert_eq!(
        record["input_size"],
        fs::metadata(&video).expect("metadata").len()
    );

    let corrupt = copied_video(&root, "audio_only_screen.mov");
    let output = describe(&root, &corrupt, "generated")
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let header = &read_jsonl(&corrupt.with_extension("jsonl"))[0];
    assert_eq!(
        header["_solstone_processing"]["reason_code"],
        "corrupt_input"
    );
    assert!(header.get("_solstone_thinking").is_none());
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn unknown_finish_reason_is_clean_and_request_uses_phase_one_contract() {
    let root = temporary_root("request");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let request_log = root.join("requests.jsonl");
    let output = describe(&root, &video, "generated_unknown_finish")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = &read_jsonl(&request_log)[0];
    assert_eq!(request["context"], "observe.describe.frame");
    assert_eq!(request["json_output"], true);
    assert_eq!(request["temperature"], 0.7);
    assert_eq!(request["max_output_tokens"], 512);
    assert_eq!(request["thinking_budget"], 1024);
    let rows = read_jsonl(&video.with_extension("jsonl"));
    assert!(rows[1].get("finish_reason").is_none());
    assert!(rows[1].get("error").is_none());
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn selection_accepts_bare_and_wrapped_responses_and_uses_selection_contract() {
    for mode in ["selection_bare_array", "selection_over_cap"] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "mixed_vp8_screen.webm");
        let request_log = root.join("requests.jsonl");
        let output = describe(&root, &video, mode)
            .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
            .output()
            .expect("run describe binary");
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let requests = selection_requests(&request_log);
        assert_eq!(requests.len(), 1, "{mode}");
        let request = &requests[0];
        assert_eq!(request["json_output"], true);
        assert_eq!(request["temperature"], 0.3);
        assert_eq!(request["max_output_tokens"], 1024);
        assert_eq!(request["thinking_budget"], 4096);
        assert_eq!(
            request["json_schema"],
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../../../solstone/observe/extract.schema.json"
            ))
            .expect("selection schema")
        );
        let summaries = request["contents"]
            .as_array()
            .and_then(|contents| contents.first())
            .and_then(|content| content["text"].as_str())
            .map(serde_json::from_str::<Vec<serde_json::Value>>)
            .expect("selection summaries")
            .expect("selection summaries JSON");
        assert_eq!(
            summaries
                .iter()
                .map(|summary| summary["frame_id"].as_u64().expect("frame id"))
                .collect::<Vec<_>>(),
            vec![1, 7, 13],
            "summaries stay in frame order"
        );
        for summary in summaries {
            assert_eq!(
                summary
                    .as_object()
                    .expect("summary object")
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
                [
                    "frame_id",
                    "overlap",
                    "primary",
                    "secondary",
                    "timestamp",
                    "visual_description",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect()
            );
        }
        assert!(
            request["system_instruction"]
                .as_str()
                .expect("system instruction")
                .contains("select the frames most valuable for text extraction")
        );
        fs::remove_dir_all(root).expect("remove temporary root");
    }
}

#[test]
fn selection_excludes_failed_categorization_frames() {
    let root = temporary_root("selection-failed-frame");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    fs::create_dir_all(root.join("config")).expect("config directory");
    fs::write(
        root.join("config/journal.json"),
        r#"{"describe":{"max_extractions":1}}"#,
    )
    .expect("config");
    let request_log = root.join("requests.jsonl");
    let output = describe(&root, &video, "selection_skips_failed_first")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .output()
        .expect("describe");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let selection = selection_requests(&request_log);
    assert_eq!(selection.len(), 1);
    let summaries = selection[0]["contents"][0]["text"]
        .as_str()
        .map(serde_json::from_str::<Vec<serde_json::Value>>)
        .expect("selection summaries")
        .expect("selection summary JSON");
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary["frame_id"].as_u64().expect("frame id"))
            .collect::<Vec<_>>(),
        vec![7, 13]
    );
    assert_eq!(
        read_jsonl(&request_log)
            .iter()
            .filter(|request| {
                request["context"] == "observe.describe.frame"
                    && request["id"]
                        .as_str()
                        .is_some_and(|id| id.starts_with("frame:1:"))
            })
            .count(),
        5
    );
    let rows = read_jsonl(&video.with_extension("jsonl"));
    let first_success = rows[1..]
        .iter()
        .find(|row| row["frame_id"] == 7)
        .expect("frame 7 row");
    assert_eq!(first_success["enhanced"], true);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn selection_blocking_and_unknown_refusals_abort_after_one_selection_request() {
    for mode in ["selection_blocking_refusal", "selection_unknown_code"] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "mixed_vp8_screen.webm");
        let request_log = root.join("requests.jsonl");
        let output = describe(&root, &video, mode)
            .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
            .output()
            .expect("run describe binary");
        assert_eq!(output.status.code(), Some(69), "{mode}");
        assert_eq!(selection_requests(&request_log).len(), 1, "{mode}");
        assert!(!video.with_extension("jsonl").exists(), "{mode}");
        assert!(no_temp_files(&root), "{mode}");
        remove_temporary_root(&root);
    }
}

#[test]
fn selection_nonblocking_or_unparseable_response_uses_fallback() {
    for mode in [
        "selection_nonblocking_retryable_refusal",
        "selection_unparseable",
    ] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "mixed_vp8_screen.webm");
        let request_log = root.join("requests.jsonl");
        let output = describe(&root, &video, mode)
            .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
            .output()
            .expect("run describe binary");
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(selection_requests(&request_log).len(), 1, "{mode}");
        assert!(video.with_extension("jsonl").exists(), "{mode}");
        fs::remove_dir_all(root).expect("remove temporary root");
    }
}

#[test]
fn extraction_uses_per_category_contracts_and_redaction() {
    for (mode, context, tokens, thinking, json_output, prompt_fragment) in [
        (
            "category_browsing",
            "observe.describe.browsing",
            2048,
            4096,
            false,
            "# Web Browsing Text Extraction",
        ),
        (
            "category_messaging",
            "observe.describe.messaging",
            8192,
            6144,
            true,
            "# Messaging Extraction",
        ),
        (
            "category_meeting",
            "observe.describe.meeting",
            4096,
            6144,
            true,
            "# Meeting State Analysis",
        ),
    ] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "single_frame_vp8_screen.webm");
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            r#"{"describe":{"redact":["secret"]}}"#,
        )
        .expect("config");
        let request_log = root.join("requests.jsonl");
        let output = describe(&root, &video, mode)
            .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
            .output()
            .expect("describe");
        assert!(output.status.success(), "{mode}");
        let requests = extraction_requests(&request_log);
        assert_eq!(requests.len(), 1, "{mode}");
        let request = &requests[0];
        assert_eq!(request["context"], context);
        assert_eq!(request["max_output_tokens"], tokens);
        assert_eq!(request["thinking_budget"], thinking);
        assert_eq!(request["json_output"], json_output);
        assert_eq!(request["temperature"], 0.3);
        assert!(
            request["system_instruction"]
                .as_str()
                .expect("instruction")
                .ends_with("- secret\n")
        );
        assert!(
            request["system_instruction"]
                .as_str()
                .expect("instruction")
                .contains(prompt_fragment),
            "{mode} uses its category prompt body"
        );
        fs::remove_dir_all(root).expect("remove root");
    }
}

#[test]
fn extraction_secondary_and_retry_paths_are_independent() {
    let root = temporary_root("extraction-secondary");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let request_log = root.join("requests.jsonl");
    let launch_log = root.join("launches.log");
    fs::create_dir_all(root.join("config")).expect("config directory");
    fs::write(
        root.join("config/journal.json"),
        r#"{"describe":{"redact":["secret"]}}"#,
    )
    .expect("config");
    let output = describe(&root, &video, "category_secondary")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .env(
            "SOLSTONE_DESCRIBE_SESSION_STUB_LAUNCH_LOG_PATH",
            &launch_log,
        )
        .output()
        .expect("describe");
    assert!(output.status.success());
    let category_requests = extraction_requests(&request_log);
    let contexts = category_requests
        .iter()
        .map(|request| request["context"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        contexts,
        vec!["observe.describe.code", "observe.describe.messaging"]
    );
    assert!(category_requests.iter().all(|request| {
        request["system_instruction"]
            .as_str()
            .is_some_and(|instruction| instruction.ends_with("- secret\n"))
    }));
    assert_eq!(
        fs::read_to_string(&launch_log)
            .expect("launch log")
            .lines()
            .count(),
        1,
        "all describe phases share one session child"
    );
    fs::remove_dir_all(&root).expect("remove root");

    let root = temporary_root("extraction-retry");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let request_log = root.join("requests.jsonl");
    let output = describe(&root, &video, "extraction_json_retry_then_succeed")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .output()
        .expect("describe");
    assert!(output.status.success());
    let requests = extraction_requests(&request_log);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["attempt_index"], 0);
    assert_eq!(requests[1]["attempt_index"], 1);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn extraction_exhaustion_fails_and_blocking_refusal_aborts() {
    for mode in ["extraction_refusal", "extraction_markdown_length"] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "single_frame_vp8_screen.webm");
        let request_log = root.join("requests.jsonl");
        let output = describe(&root, &video, mode)
            .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
            .output()
            .expect("describe");
        assert!(output.status.success(), "{mode}");
        if mode == "extraction_markdown_length" {
            assert_eq!(extraction_requests(&request_log).len(), 5, "{mode}");
        }
        let rows = read_jsonl(&video.with_extension("jsonl"));
        assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
        assert_eq!(
            rows[0]["_solstone_processing"]["reason_code"],
            "analysis_failed"
        );
        fs::remove_dir_all(root).expect("remove root");
    }
    let root = temporary_root("extraction-blocked");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let output = describe(&root, &video, "extraction_blocking_refusal")
        .output()
        .expect("describe");
    assert_eq!(output.status.code(), Some(69));
    assert!(!video.with_extension("jsonl").exists());
    assert!(no_temp_files(&root));
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn extraction_unparseable_json_retries_to_its_own_ceiling() {
    let root = temporary_root("extraction-json-unparseable");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let request_log = root.join("requests.jsonl");
    let output = describe(&root, &video, "extraction_json_unparseable")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .output()
        .expect("describe");
    assert!(output.status.success());
    assert_eq!(extraction_requests(&request_log).len(), 5);
    let rows = read_jsonl(&video.with_extension("jsonl"));
    assert!(rows[1].get("error").is_some());
    let record = rows[1]["requests"][1].as_object().expect("category record");
    assert_eq!(record.get("retries"), Some(&serde_json::json!(4)));
    assert_eq!(
        record.keys().cloned().collect::<BTreeSet<_>>(),
        ["category", "duration", "model", "retries", "type"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    assert_eq!(
        rows[0]["_solstone_processing"]["reason_code"],
        "analysis_failed"
    );
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn journal_request_records_match_phase_one_and_category_shapes() {
    let root = temporary_root("request-record-shape");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let output = describe(&root, &video, "generated")
        .output()
        .expect("describe");
    assert!(output.status.success());
    let rows = read_jsonl(&video.with_extension("jsonl"));
    let requests = rows[1]["requests"].as_array().expect("requests array");
    assert_eq!(requests.len(), 2);
    let describe = requests[0].as_object().expect("describe record");
    assert_eq!(
        describe.keys().cloned().collect::<BTreeSet<_>>(),
        ["duration", "model", "type"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert_eq!(describe["type"], "describe");
    assert_eq!(describe["model"], "describe-stub");
    assert!(describe["duration"].is_number());
    let category = requests[1].as_object().expect("category record");
    assert_eq!(
        category.keys().cloned().collect::<BTreeSet<_>>(),
        ["category", "duration", "model", "type"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert_eq!(category["type"], "category");
    assert_eq!(category["category"], "code");
    assert_eq!(category["model"], "describe-stub");
    assert!(category["duration"].is_number());
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn extraction_markdown_stop_unknown_and_empty_are_clean_without_retry() {
    for mode in [
        "extraction_markdown_success",
        "extraction_markdown_unknown",
        "extraction_markdown_empty",
    ] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "single_frame_vp8_screen.webm");
        let request_log = root.join("requests.jsonl");
        let output = describe(&root, &video, mode)
            .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
            .output()
            .expect("describe");
        assert!(output.status.success(), "{mode}");
        assert_eq!(extraction_requests(&request_log).len(), 1, "{mode}");
        let rows = read_jsonl(&video.with_extension("jsonl"));
        assert_eq!(rows[1]["content"]["code"], "# extracted markdown", "{mode}");
        assert!(rows[1].get("error").is_none(), "{mode}");
        fs::remove_dir_all(root).expect("remove root");
    }
}

#[test]
fn selected_unknown_category_emits_an_unenhanced_clean_row() {
    let root = temporary_root("unextractable-category");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let output = describe(&root, &video, "category_unextractable")
        .output()
        .expect("describe");
    assert!(output.status.success());
    let rows = read_jsonl(&video.with_extension("jsonl"));
    assert_eq!(rows[1]["enhanced"], false);
    assert!(rows[1].get("content").is_none());
    assert!(rows[1].get("error").is_none());
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn detection_runs_for_unselected_media_and_preserves_unfiltered_objects() {
    let root = temporary_root("detect-media");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    fs::create_dir_all(root.join("config")).expect("config directory");
    fs::write(
        root.join("config/journal.json"),
        r#"{"describe":{"max_extractions":1}}"#,
    )
    .expect("config");
    let detector_log = root.join("detector.jsonl");
    let output = describe(&root, &video, "category_media")
        .env("SOLSTONE_DESCRIBE_DETECT_BINARY", DETECT_STUB)
        .env("SOLSTONE_DESCRIBE_DETECT_STUB_REQUESTS_PATH", &detector_log)
        .output()
        .expect("describe");
    assert!(output.status.success());
    let rows = read_jsonl(&video.with_extension("jsonl"));
    let unselected = rows[1..]
        .iter()
        .find(|row| row["enhanced"] == false)
        .expect("unselected row");
    assert_eq!(unselected["detections"]["gate"], "primary:media");
    assert_eq!(
        unselected["detections"]["objects"][1]["class_name"],
        "person"
    );
    assert_eq!(unselected["detections"]["objects"][1]["score"], 0.1);
    let requests = detector_requests(&detector_log);
    assert!(!requests.is_empty());
    assert!(requests[0]["input_bytes"].as_u64().expect("input bytes") > 0);
    assert_eq!(requests[0]["threshold"], "0.25");
    assert_eq!(requests[0]["threads"], "4");
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn detection_secondary_gate_uses_secondary_label() {
    let root = temporary_root("detect-secondary");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let output = describe(&root, &video, "category_secondary_social")
        .env("SOLSTONE_DESCRIBE_DETECT_BINARY", DETECT_STUB)
        .output()
        .expect("describe");
    assert!(output.status.success());
    let rows = read_jsonl(&video.with_extension("jsonl"));
    assert_eq!(rows[1]["detections"]["gate"], "secondary:social");
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn detection_failure_and_timeout_latch_after_one_attempt() {
    for (mode, timeout) in [
        ("invalid_json", None),
        ("exit_failure", None),
        ("hang", Some("100")),
    ] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "mixed_vp8_screen.webm");
        let detector_log = root.join("detector.jsonl");
        let start = Instant::now();
        let mut command = describe(&root, &video, "category_media");
        command
            .env("SOLSTONE_DESCRIBE_DETECT_BINARY", DETECT_STUB)
            .env("SOLSTONE_DESCRIBE_DETECT_STUB_MODE", mode)
            .env("SOLSTONE_DESCRIBE_DETECT_STUB_REQUESTS_PATH", &detector_log);
        if let Some(timeout) = timeout {
            command.env("SOLSTONE_DESCRIBE_DETECT_TIMEOUT_MS", timeout);
        }
        let output = command.output().expect("describe");
        assert!(output.status.success(), "{mode}");
        assert_eq!(detector_requests(&detector_log).len(), 1, "{mode}");
        if mode == "hang" {
            assert!(start.elapsed() < Duration::from_secs(10));
        }
        let rows = read_jsonl(&video.with_extension("jsonl"));
        assert_eq!(rows[0]["_solstone_processing"]["state"], "analyzed");
        assert!(rows[1..].iter().all(|row| row.get("detections").is_none()));
        fs::remove_dir_all(root).expect("remove root");
    }
}

#[test]
fn multi_frame_rows_have_exact_reference_keys_and_never_leak_pending() {
    let root = temporary_root("complete-row-keys");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    fs::create_dir_all(root.join("config")).expect("config directory");
    fs::write(
        root.join("config/journal.json"),
        r#"{"describe":{"max_extractions":1}}"#,
    )
    .expect("config");
    let output = describe(&root, &video, "category_media")
        .env("SOLSTONE_DESCRIBE_DETECT_BINARY", DETECT_STUB)
        .output()
        .expect("describe");
    assert!(output.status.success());
    let rows = read_jsonl(&video.with_extension("jsonl"));
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().skip(1).all(|row| row.get("pending").is_none()));
    let expected_extracted = [
        "analysis",
        "content",
        "detections",
        "enhanced",
        "frame_id",
        "requests",
        "timestamp",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    let expected_unselected = [
        "analysis",
        "detections",
        "enhanced",
        "frame_id",
        "requests",
        "timestamp",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    for row in &rows[1..] {
        let keys = row
            .as_object()
            .expect("row object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            if row["enhanced"] == true {
                expected_extracted.clone()
            } else {
                expected_unselected.clone()
            }
        );
    }
    assert!(rows[1..].iter().any(|row| row["enhanced"] == true));
    assert!(rows[1..].iter().any(|row| row["enhanced"] == false));
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn redact_config_is_appended_in_order() {
    let root = temporary_root("redact");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    fs::create_dir_all(root.join("config")).expect("create config dir");
    fs::write(
        root.join("config/journal.json"),
        r#"{"describe":{"redact":["rule one","rule two"]}}"#,
    )
    .expect("write config");
    let request_log = root.join("requests.jsonl");
    let output = describe(&root, &video, "generated")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let instruction = read_jsonl(&request_log)[0]["system_instruction"]
        .as_str()
        .expect("instruction")
        .to_owned();
    assert!(instruction.ends_with("Redaction rules (apply these exactly as written, do not generalize):\n- rule one\n- rule two\n"));
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn tier_one_temp_is_nonempty_before_atomic_promotion_and_is_removed_afterward() {
    let root = temporary_root("temp");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    let release = root.join("release");
    let request_log = root.join("requests.jsonl");
    let mut child = describe(&root, &video, "generated")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_RELEASE_PATH", &release)
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_PAUSE_AFTER", "5")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .spawn()
        .expect("spawn describe binary");
    // The stub holds the child paused indefinitely once it releases its 5th
    // response (see wait_for_release in session_stub.rs), so this loop never
    // races against how fast masking runs on this machine: it only needs to
    // distinguish "still working" from "genuinely stuck or dead", so the cap
    // is a hang-prevention ceiling, not a performance-calibrated margin.
    let deadline = Instant::now() + Duration::from_secs(60);
    let saw_nonempty = loop {
        let nonempty = fs::read_dir(&root)
            .expect("read root")
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".describe-")
                    && entry
                        .metadata()
                        .map(|metadata| metadata.len() > 0)
                        .unwrap_or(false)
            });
        if nonempty {
            break true;
        }
        if let Some(status) = child.try_wait().expect("poll describe binary") {
            panic!("describe binary exited before tier-one temp became nonempty: {status}");
        }
        if Instant::now() >= deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(
        saw_nonempty,
        "tier-one rows temp became nonempty while the run was active"
    );
    assert!(!video.with_extension("jsonl").exists());
    assert_eq!(read_jsonl(&request_log).len(), 5);
    fs::write(&release, b"release").expect("release stub");
    assert!(child.wait().expect("wait child").success());
    assert!(no_temp_files(&root));
    assert!(video.with_extension("jsonl").exists());
    assert_eq!(read_jsonl(&video.with_extension("jsonl")).len(), 4);
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn session_submits_a_later_request_before_the_first_response() {
    let root = temporary_root("submit-ahead");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    let output = describe(&root, &video, "hold_first_until_second")
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        read_jsonl(&video.with_extension("jsonl"))[0]["_solstone_processing"]["state"],
        "analyzed"
    );
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn blocking_and_session_abort_notifications_have_distinct_flat_shapes() {
    for (mode, is_refusal) in [("blocking_retryable", true), ("exit_after_one", false)] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "mixed_vp8_screen.webm");
        let listener = notification_listener(&root);
        let output = describe(&root, &video, mode)
            .output()
            .expect("run describe binary");
        assert_eq!(output.status.code(), Some(69), "{mode}");
        let row = notification(&listener);
        assert_eq!(row["tract"], "notification");
        assert_eq!(row["event"], "show");
        assert!(row.get("work_key").is_some());
        if is_refusal {
            assert_eq!(row["reason_code"], "binary_missing");
            assert_eq!(row["provider"], "stub");
        } else {
            assert!(row.get("reason_code").is_none());
            assert!(row.get("provider").is_none());
        }
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        fs::remove_dir_all(root).expect("remove temporary root");
    }
}

#[test]
fn unwritable_output_parent_is_an_internal_non_boundary_error() {
    let root = temporary_root("unwritable");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).expect("make parent unwritable");
    let output = describe(&root, &video, "generated")
        .output()
        .expect("run describe binary");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755))
        .expect("restore parent permissions");
    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(69));
    assert!(!video.with_extension("jsonl").exists());
    fs::remove_dir_all(root).expect("remove temporary root");
}

#[test]
fn reentry_skips_clean_artifacts_and_redo_starts_a_new_attempt() {
    let root = temporary_root("reentry-skip-redo");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let artifact = video.with_extension("jsonl");
    assert!(
        describe(&root, &video, "generated")
            .output()
            .expect("initial")
            .status
            .success()
    );
    let original = fs::read(&artifact).expect("artifact bytes");
    let requests = root.join("requests.jsonl");
    let skipped = describe(&root, &video, "blocking_retryable")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &requests)
        .output()
        .expect("skip");
    assert!(skipped.status.success());
    assert_eq!(fs::read(&artifact).expect("artifact bytes"), original);
    assert!(!requests.exists(), "skip must not start a session");

    mark_for_reentry(&artifact, 2);
    let redo = describe_redo(&root, &video, "non_responsive")
        .output()
        .expect("redo");
    assert!(!redo.status.success(), "fresh all-failed redo is an error");
    let rows = read_jsonl(&artifact);
    assert_eq!(rows[0]["_solstone_processing"]["attempts"], 1);
    remove_temporary_root(&root);
}

#[test]
fn reentry_merges_gaps_and_preserves_reusable_raw_bytes() {
    let root = temporary_root("reentry-merge");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    let artifact = video.with_extension("jsonl");
    assert!(
        describe(&root, &video, "generated")
            .output()
            .expect("initial")
            .status
            .success()
    );
    let mut rows = read_jsonl(&artifact);
    let original_header = rows.remove(0);
    let reusable_id = rows[0]["frame_id"].as_u64().expect("frame id");
    let phase_one_id = rows[1]["frame_id"].as_u64().expect("frame id");
    let phase_three_id = rows[2]["frame_id"].as_u64().expect("frame id");
    rows[1]["analysis"] = Value::Null;
    rows[1]["enhanced"] = json!(false);
    rows[2]["enhanced"] = json!(true);
    rows[2]["content"] = json!({"kept":{"v":1}});
    rows[2]["error"] = json!("prior failure");
    let raw_reusable = format!(
        "{{\"timestamp\": 0.10000000000000001, \"z\": \"caf\\u00e9\", \"analysis\": {{\"primary\": \"code\", \"secondary\": \"none\", \"overlap\": true}}, \"enhanced\": false, \"frame_id\": {reusable_id}, \"requests\": []}}\n"
    );
    let mut header = original_header;
    header["_solstone_processing"]["state"] = json!("failed");
    header["_solstone_processing"]["reason_code"] = json!("analysis_failed");
    header["_solstone_processing"]["attempts"] = json!(1);
    let mut fixture = format!("{header}\n{raw_reusable}");
    for row in rows.into_iter().skip(1) {
        fixture.push_str(&format!("{row}\n"));
    }
    fs::write(&artifact, fixture).expect("write reentry fixture");
    let requests = root.join("requests.jsonl");
    let output = describe(&root, &video, "generated")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &requests)
        .output()
        .expect("reentry");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requested = read_jsonl(&requests);
    assert!(
        requested
            .iter()
            .any(|request| request["id"] == format!("frame:{phase_one_id}:attempt:0"))
    );
    assert!(requested.iter().all(|request| {
        request["id"].as_str().is_none_or(|id| {
            !id.starts_with(&format!("frame:{reusable_id}:"))
                && !id.starts_with(&format!("extract:{reusable_id}:"))
        })
    }));
    assert!(requested.iter().any(|request| {
        request["context"] == "observe.describe.code"
            && request["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(&format!("extract:{phase_three_id}:")))
    }));
    let body = jsonl_body(&artifact);
    assert!(
        body.contains(&raw_reusable),
        "reusable row is byte-for-byte preserved"
    );
    let final_rows = read_jsonl(&artifact);
    let phase_three = final_rows
        .iter()
        .find(|row| row["frame_id"] == phase_three_id)
        .expect("phase three row");
    assert_eq!(phase_three["content"]["kept"]["v"], 1);
    assert!(phase_three["content"].get("code").is_some());
    assert!(phase_three.get("error").is_none());
    assert_eq!(
        phase_three["requests"].as_array().expect("requests").len(),
        3
    );
    assert!(
        final_rows[0]["_solstone_processing"]
            .get("attempts")
            .is_none()
    );
    remove_temporary_root(&root);
}

#[test]
fn fresh_emits_completion_order_while_incremental_emits_frame_id_order() {
    let root = temporary_root("emission-order");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    assert!(
        describe(&root, &video, "hold_first_until_second")
            .output()
            .expect("fresh")
            .status
            .success()
    );
    let artifact = video.with_extension("jsonl");
    let fresh_ids = read_jsonl(&artifact)[1..]
        .iter()
        .map(|row| row["frame_id"].as_u64().expect("frame id"))
        .collect::<Vec<_>>();
    let mut sorted = fresh_ids.clone();
    sorted.sort_unstable();
    assert_ne!(fresh_ids, sorted, "fresh rows retain completion order");
    mark_for_reentry(&artifact, 1);
    assert!(
        describe(&root, &video, "generated")
            .output()
            .expect("reentry")
            .status
            .success()
    );
    let incremental_ids = read_jsonl(&artifact)[1..]
        .iter()
        .map(|row| row["frame_id"].as_u64().expect("frame id"))
        .collect::<Vec<_>>();
    assert!(incremental_ids.windows(2).all(|ids| ids[0] < ids[1]));
    remove_temporary_root(&root);
}

#[test]
fn observer_and_completion_event_follow_reentry_rules() {
    let root = temporary_root("observer-event");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let artifact = video.with_extension("jsonl");
    assert!(
        describe(&root, &video, "generated")
            .output()
            .expect("initial")
            .status
            .success()
    );
    mark_for_reentry(&artifact, 1);
    rewrite_header(&artifact, |header| {
        header["observer"] = json!("previous-observer");
        header["qualified_count"] = json!(999);
    });
    let listener = notification_listener(&root);
    let output = describe(&root, &video, "generated")
        .output()
        .expect("reentry");
    assert!(output.status.success());
    let event = notification(&listener);
    assert_eq!(event["tract"], "observe");
    assert_eq!(event["event"], "described");
    assert!(event["duration_ms"].is_u64());
    assert_eq!(read_jsonl(&artifact)[0]["observer"], "previous-observer");

    mark_for_reentry(&artifact, 2);
    let output = describe(&root, &video, "generated")
        .env("OBSERVER_NAME", "current-observer")
        .output()
        .expect("environment observer");
    assert!(output.status.success());
    assert_eq!(read_jsonl(&artifact)[0]["observer"], "current-observer");
    remove_temporary_root(&root);
}

#[test]
fn blocked_reentry_does_not_touch_the_existing_artifact() {
    let root = temporary_root("blocked-reentry");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let artifact = video.with_extension("jsonl");
    assert!(
        describe(&root, &video, "generated")
            .output()
            .expect("initial")
            .status
            .success()
    );
    mark_for_reentry(&artifact, 1);
    let mut rows = read_jsonl(&artifact);
    rows[1]["analysis"] = Value::Null;
    let mut fixture = format!("{}\n", rows[0]);
    fixture.push_str(&format!("{}\n", rows[1]));
    fs::write(&artifact, fixture).expect("write reentry fixture");
    let original = fs::read(&artifact).expect("artifact bytes");
    let output = describe(&root, &video, "blocking_retryable")
        .output()
        .expect("blocked reentry");
    assert_eq!(output.status.code(), Some(69));
    assert_eq!(fs::read(&artifact).expect("artifact bytes"), original);
    assert!(no_temp_files(&root));
    remove_temporary_root(&root);
}

#[test]
fn failed_reentries_converge_attempts_and_keep_clean_rows_raw() {
    let root = temporary_root("reentry-convergence");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    let artifact = video.with_extension("jsonl");
    assert!(
        describe(&root, &video, "generated")
            .output()
            .expect("initial")
            .status
            .success()
    );
    mark_for_reentry(&artifact, 1);
    let mut rows = read_jsonl(&artifact);
    rows[2]["content"] = json!({});
    rows[2]["error"] = json!("prior failure");
    let mut fixture = String::new();
    for row in rows {
        fixture.push_str(&format!("{row}\n"));
    }
    fs::write(&artifact, fixture).expect("write fixture");
    let clean_before = jsonl_body(&artifact)
        .lines()
        .next()
        .expect("first clean row")
        .to_owned();
    for attempts in [2, 3] {
        let output = describe(&root, &video, "extraction_refusal")
            .output()
            .expect("failed reentry");
        assert!(output.status.success());
        let rows = read_jsonl(&artifact);
        assert_eq!(rows[0]["_solstone_processing"]["attempts"], attempts);
        assert_eq!(
            jsonl_body(&artifact)
                .lines()
                .next()
                .expect("first clean row"),
            clean_before
        );
    }
    remove_temporary_root(&root);
}

#[test]
fn incremental_all_failures_complete_while_zero_qualified_reentry_discards_rows() {
    let root = temporary_root("incremental-all-failed");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    let artifact = video.with_extension("jsonl");
    assert!(
        describe(&root, &video, "generated")
            .output()
            .expect("initial")
            .status
            .success()
    );
    mark_for_reentry(&artifact, 1);
    let mut rows = read_jsonl(&artifact);
    for row in &mut rows[2..] {
        row["analysis"] = Value::Null;
        row["enhanced"] = json!(false);
    }
    let mut contents = String::new();
    for row in rows {
        contents.push_str(&format!("{row}\n"));
    }
    fs::write(&artifact, contents).expect("all-gap fixture");
    let listener = notification_listener(&root);
    let output = describe(&root, &video, "always_retryable")
        .output()
        .expect("incremental failure");
    assert!(
        output.status.success(),
        "incremental plan suppresses the fresh error"
    );
    assert_eq!(
        read_jsonl(&artifact)[0]["_solstone_processing"]["state"],
        "failed"
    );
    assert_eq!(
        read_jsonl(&artifact)[0]["_solstone_processing"]["attempts"],
        2
    );
    assert_eq!(notification(&listener)["event"], "described");

    let empty_video = root.join("convey_skipped_screen.webm");
    fs::copy(
        masked_corpus_path("convey_skipped_screen.webm"),
        &empty_video,
    )
    .expect("copy empty");
    let empty_artifact = empty_video.with_extension("jsonl");
    assert!(
        describe(&root, &empty_video, "generated")
            .output()
            .expect("empty initial")
            .status
            .success()
    );
    mark_for_reentry(&empty_artifact, 1);
    assert!(
        describe(&root, &empty_video, "generated")
            .output()
            .expect("empty reentry")
            .status
            .success()
    );
    let empty = read_jsonl(&empty_artifact);
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0]["_solstone_processing"]["state"], "empty");
    assert!(empty[0]["_solstone_processing"].get("attempts").is_none());

    let corrupt = copied_video(&root, "audio_only_screen.mov");
    let corrupt_artifact = corrupt.with_extension("jsonl");
    assert!(
        describe(&root, &corrupt, "generated")
            .output()
            .expect("corrupt initial")
            .status
            .success()
    );
    mark_for_reentry(&corrupt_artifact, 1);
    assert!(
        describe(&root, &corrupt, "generated")
            .output()
            .expect("corrupt reentry")
            .status
            .success()
    );
    let corrupt = read_jsonl(&corrupt_artifact);
    assert_eq!(corrupt.len(), 1);
    assert_eq!(
        corrupt[0]["_solstone_processing"]["reason_code"],
        "corrupt_input"
    );
    assert_eq!(corrupt[0]["_solstone_processing"]["attempts"], 2);
    remove_temporary_root(&root);
}

#[test]
fn incremental_all_failures_complete_with_zero_reusable_rows() {
    let root = temporary_root("incremental-all-gaps");
    let video = copied_video(&root, "mixed_vp8_screen.webm");
    let artifact = video.with_extension("jsonl");
    assert!(
        describe(&root, &video, "generated")
            .output()
            .expect("initial")
            .status
            .success()
    );
    mark_for_reentry(&artifact, 1);
    let mut rows = read_jsonl(&artifact);
    for row in &mut rows[1..] {
        row["analysis"] = Value::Null;
        row["enhanced"] = json!(false);
    }
    let mut contents = String::new();
    for row in rows {
        contents.push_str(&format!("{row}\n"));
    }
    fs::write(&artifact, contents).expect("all-gap fixture");
    let listener = notification_listener(&root);
    let output = describe(&root, &video, "always_retryable")
        .output()
        .expect("incremental failure");
    assert!(
        output.status.success(),
        "a plan with no reusable rows still suppresses the fresh error"
    );
    let rows = read_jsonl(&artifact);
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    assert_eq!(
        rows[0]["_solstone_processing"]["reason_code"],
        "analysis_failed"
    );
    assert_eq!(rows[0]["_solstone_processing"]["attempts"], 2);
    assert_eq!(notification(&listener)["event"], "described");
    remove_temporary_root(&root);
}
