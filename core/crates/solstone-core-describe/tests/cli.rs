use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core-describe");

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
