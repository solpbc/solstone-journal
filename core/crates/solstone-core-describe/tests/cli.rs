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
fn non_responsive_is_a_nonblocking_failed_analysis() {
    let root = temporary_root("non-responsive");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let output = describe(&root, &video, "non_responsive")
        .output()
        .expect("run describe binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = read_jsonl(&video.with_extension("jsonl"));
    assert_eq!(rows[0]["_solstone_processing"]["state"], "failed");
    assert_eq!(
        rows[0]["_solstone_processing"]["reason_code"],
        "analysis_failed"
    );
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
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
    assert!(rows[1].get("error").is_some());
    assert_eq!(rows[1]["requests"][0]["retries"], 4);
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
    for (mode, context, tokens, thinking, json_output) in [
        (
            "category_browsing",
            "observe.describe.browsing",
            2048,
            4096,
            false,
        ),
        (
            "category_messaging",
            "observe.describe.messaging",
            8192,
            6144,
            true,
        ),
        (
            "category_meeting",
            "observe.describe.meeting",
            4096,
            6144,
            true,
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
        fs::remove_dir_all(root).expect("remove root");
    }
}

#[test]
fn extraction_secondary_and_retry_paths_are_independent() {
    let root = temporary_root("extraction-secondary");
    let video = copied_video(&root, "single_frame_vp8_screen.webm");
    let request_log = root.join("requests.jsonl");
    let output = describe(&root, &video, "category_secondary")
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH", &request_log)
        .output()
        .expect("describe");
    assert!(output.status.success());
    let contexts = extraction_requests(&request_log)
        .into_iter()
        .map(|request| request["context"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        contexts,
        vec!["observe.describe.code", "observe.describe.messaging"]
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
        let output = describe(&root, &video, mode).output().expect("describe");
        assert!(output.status.success(), "{mode}");
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
fn extraction_markdown_stop_and_unknown_are_clean_without_retry() {
    for mode in ["extraction_markdown_success", "extraction_markdown_unknown"] {
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
    for (mode, timeout) in [("invalid_json", None), ("hang", Some("100"))] {
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
    // Debug-build masking reached this point in at most 1,670ms; this leaves a
    // 4.2x margin for contended test workers, based on the profile cargo test uses.
    let deadline = Instant::now() + Duration::from_millis(7_000);
    let mut saw_nonempty = false;
    while Instant::now() < deadline {
        saw_nonempty = fs::read_dir(&root)
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
        if saw_nonempty {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
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
