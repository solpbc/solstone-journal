// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native-boundary coverage for `journal describe`.
//!
//! Pipeline semantics live beside the implementation in `pipeline_tests.rs`.
//! This harness intentionally keeps only contracts that require a real CLI,
//! native decode, detector child, or interprocess session behavior.

use std::fs;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;

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

fn read_jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read JSONL")
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid JSONL row"))
        .collect()
}

fn detector_requests(path: &Path) -> Vec<Value> {
    read_jsonl(path)
}

fn assert_stderr_deferred(output: &std::process::Output, expected_token: Option<&str>) {
    let expected = match expected_token {
        Some(token) if !token.is_empty() => format!("describe deferred: {token}"),
        _ => "describe deferred".to_owned(),
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.lines().any(|line| line.trim() == expected),
        "expected stderr line {expected:?}, got {stderr:?}"
    );
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

fn notification(listener: &UnixListener) -> Value {
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
    let actual: Value = serde_json::from_slice(&output.stdout).expect("JSON output");
    assert_eq!(actual["width"], serde_json::json!(case.width));
    assert_eq!(actual["height"], serde_json::json!(case.height));
    let expected_frames: Vec<Value> = case
        .frames
        .into_iter()
        .map(|frame| serde_json::json!({"frame_id": frame.frame_id, "timestamp": frame.timestamp}))
        .collect();
    assert_eq!(actual["frames"], serde_json::json!(expected_frames));
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
fn frames_only_owner_debug_and_verbose_flags_are_noops() {
    for flag in ["-v", "-d"] {
        let output = frames_only("mixed_vp8_screen.webm")
            .arg(flag)
            .output()
            .expect("run describe binary");
        assert!(
            output.status.success(),
            "{flag}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn malformed_invocation_is_a_usage_error() {
    let output = Command::new(BINARY).output().expect("run describe binary");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("usage: journal describe"));
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
        let expected_token = match mode {
            "blocking_retryable" => Some("binary_missing"),
            "unknown_code" => Some("future_code"),
            _ => unreachable!("blocked mode"),
        };
        assert_stderr_deferred(&output, expected_token);
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
    assert_stderr_deferred(&output, None);
    assert!(!video.with_extension("jsonl").exists());
    assert!(no_temp_files(&root));
    fs::remove_dir_all(root).expect("remove temporary root");
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
fn detection_failures_latch_after_one_attempt() {
    for mode in ["invalid_json", "exit_failure"] {
        let root = temporary_root(mode);
        let video = copied_video(&root, "mixed_vp8_screen.webm");
        let detector_log = root.join("detector.jsonl");
        let mut command = describe(&root, &video, "category_media");
        command
            .env("SOLSTONE_DESCRIBE_DETECT_BINARY", DETECT_STUB)
            .env("SOLSTONE_DESCRIBE_DETECT_STUB_MODE", mode)
            .env("SOLSTONE_DESCRIBE_DETECT_STUB_REQUESTS_PATH", &detector_log);
        let output = command.output().expect("describe");
        assert!(output.status.success(), "{mode}");
        assert_eq!(detector_requests(&detector_log).len(), 1, "{mode}");
        let rows = read_jsonl(&video.with_extension("jsonl"));
        assert_eq!(rows[0]["_solstone_processing"]["state"], "analyzed");
        assert!(rows[1..].iter().all(|row| row.get("detections").is_none()));
        fs::remove_dir_all(root).expect("remove root");
    }
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
    let deadline = Instant::now() + Duration::from_secs(120);
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
        "tier-one rows temp became nonempty while active"
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
        let expected_token = if is_refusal {
            Some("binary_missing")
        } else {
            None
        };
        assert_stderr_deferred(&output, expected_token);
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
        fs::remove_dir_all(root).expect("remove root");
    }
}
