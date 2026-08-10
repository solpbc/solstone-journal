// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use image::ImageReader;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "solstone-core-grab-differential-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary directory");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct Fixture {
    _temp: TempDir,
    journal: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new();
        let journal = temp.path.join("journal");
        let root = repository_root();
        fs::create_dir_all(journal.join("chronicle")).expect("create chronicle");

        for day in [
            "20260801", "20260802", "20260803", "20260804", "20260805", "20260806",
        ] {
            let segment = segment(&journal, day, "work", "120000_1");
            write_jsonl(
                &segment.join("screen.jsonl"),
                &["{\"frame_id\":1,\"timestamp\":0,\"analysis\":{\"primary\":\"seed\"}}"],
            );
        }

        let day = journal.join("chronicle/20260809");
        fs::create_dir_all(day.join("health")).expect("create health stream");
        fs::create_dir_all(day.join("empty")).expect("create empty stream");
        fs::create_dir_all(day.join("work/150000_1")).expect("create zero-screen segment");
        let other = segment(&journal, "20260809", "other", "120000_1");
        write_jsonl(
            &other.join("screen.jsonl"),
            &["{\"frame_id\":1,\"timestamp\":0,\"analysis\":{\"primary\":\"other\"}}"],
        );

        let main = segment(&journal, "20260809", "work", "120000_300");
        make_video(&main.join("renamed.mp4"));
        write_jsonl(
            &main.join("screen.jsonl"),
            &[
                "{\"raw\":\"renamed.mp4\"}",
                "{\"frame_id\":1,\"timestamp\":0,\"analysis\":{\"primary\":\"editor\"},\"box_2d\":[1,2,3,4]}",
                "{\"frame_id\":2,\"timestamp\":0.5,\"analysis\":{\"primary\":\"browser\"}}",
                "{\"frame_id\":5,\"timestamp\":1,\"analysis\":{\"primary\":\"terminal\"},\"error\":\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\nsecond line\"}",
                "{\"frame_id\":6,\"timestamp\":\"2\",\"analysis\":{\"primary\":\"mail\"}}",
                "{\"frame_id\":11,\"timestamp\":2.5,\"analysis\":{\"primary\":\"calendar\"}}",
            ],
        );
        fs::copy(main.join("renamed.mp4"), main.join("probe_screen.mp4"))
            .expect("copy probe video");
        write_jsonl(
            &main.join("probe_screen.jsonl"),
            &[
                "{\"header\":\"without raw\"}",
                "{\"frame_id\":7.0,\"timestamp\":1,\"analysis\":{\"primary\":\"probe\"}}",
            ],
        );
        write_jsonl(
            &main.join("purged_screen.jsonl"),
            &["{\"frame_id\":1,\"timestamp\":1,\"analysis\":{\"primary\":\"purged\"}}"],
        );
        write_jsonl(&main.join("legacy_screen.jsonl"), &["{\"old\":true}"]);
        write_jsonl(
            &main.join("header_only_screen.jsonl"),
            &["{\"raw\":\"gone.webm\"}"],
        );
        fs::copy(main.join("renamed.mp4"), main.join("captured_screen.mp4"))
            .expect("copy captured video");
        write_jsonl(
            &main.join("corrupt_screen.jsonl"),
            &[
                "{\"frame_id\":1,\"timestamp\":0,\"analysis\":{\"primary\":\"survivor\"}}",
                "not valid json",
                "{\"frame_id\":2,\"timestamp\":1,\"analysis\":{\"primary\":\"also-survives\"}}",
            ],
        );

        let midnight = segment(&journal, "20260809", "work", "235900_300");
        write_jsonl(
            &midnight.join("midnight_screen.jsonl"),
            &["{\"frame_id\":1,\"timestamp\":90,\"analysis\":{\"primary\":\"tomorrow\"}}"],
        );
        for minute in 0..21 {
            let name = format!("13{minute:02}00_1");
            let extra = segment(&journal, "20260809", "work", &name);
            write_jsonl(
                &extra.join("screen.jsonl"),
                &["{\"frame_id\":1,\"timestamp\":0,\"analysis\":{\"primary\":\"extra\"}}"],
            );
        }

        Self {
            _temp: temp,
            journal,
            root,
        }
    }

    fn python(&self, args: &[String]) -> Output {
        Command::new(self.root.join(".venv/bin/python3"))
            .args(["-m", "solstone.observe.grab"])
            .args(args)
            .current_dir(&self.root)
            .env("SOLSTONE_JOURNAL", &self.journal)
            .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
            .output()
            .expect("run Python grab reference")
    }

    fn native(&self, args: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .arg("grab")
            .args(args)
            .current_dir(&self.root)
            .env("SOLSTONE_JOURNAL", &self.journal)
            .output()
            .expect("run native grab")
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate should be nested below repository root")
        .to_path_buf()
}

fn segment(journal: &Path, day: &str, stream: &str, name: &str) -> PathBuf {
    let path = journal.join("chronicle").join(day).join(stream).join(name);
    fs::create_dir_all(&path).expect("create segment");
    path
}

fn write_jsonl(path: &Path, lines: &[&str]) {
    fs::write(path, format!("{}\n", lines.join("\n"))).expect("write JSONL");
}

fn make_video(path: &Path) {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=32x32:rate=1:duration=3",
            "-c:v",
            "mpeg4",
        ])
        .arg(path)
        .status()
        .expect("ffmpeg must be available for differential fixtures");
    assert!(status.success(), "ffmpeg fixture generation failed");
}

fn with_json(args: &[&str]) -> Vec<String> {
    args.iter()
        .copied()
        .chain(["--json"])
        .map(str::to_owned)
        .collect()
}

fn plain(args: &[&str]) -> Vec<String> {
    args.iter().map(|value| (*value).to_owned()).collect()
}

fn assert_success(output: &Output, implementation: &str, args: &[String]) {
    assert!(
        output.status.success(),
        "{implementation} failed for {args:?}:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn compare_success(fixture: &Fixture, args: &[&str]) {
    for invocation in [with_json(args), plain(args)] {
        let python = fixture.python(&invocation);
        let native = fixture.native(&invocation);
        assert_success(&python, "Python", &invocation);
        assert_success(&native, "native", &invocation);
        assert_eq!(
            native.stdout,
            python.stdout,
            "stdout differs for {invocation:?}\nPython stderr:\n{}\nNative stderr:\n{}",
            String::from_utf8_lossy(&python.stderr),
            String::from_utf8_lossy(&native.stderr),
        );
    }
}

fn compare_runtime_failure(fixture: &Fixture, args: &[&str]) {
    let invocation = plain(args);
    let python = fixture.python(&invocation);
    let native = fixture.native(&invocation);
    assert_eq!(python.status.code(), Some(1), "Python {invocation:?}");
    assert_eq!(native.status.code(), Some(1), "native {invocation:?}");
    assert_eq!(native.stdout, python.stdout, "stdout for {invocation:?}");
    assert_eq!(native.stderr, python.stderr, "stderr for {invocation:?}");
}

fn assert_usage_case(fixture: &Fixture, args: &[&str], expected_message: &str) {
    let invocation = plain(args);
    let python = fixture.python(&invocation);
    let native = fixture.native(&invocation);
    assert_eq!(python.status.code(), Some(2), "Python {invocation:?}");
    assert_eq!(native.status.code(), Some(2), "native {invocation:?}");
    for (implementation, output) in [("Python", python), ("native", native)] {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected_message),
            "{implementation} omitted {expected_message:?}: {stderr}"
        );
    }
}

fn compare_save(fixture: &Fixture, frame_ids: &str, output: &Path) {
    let output = output.to_str().expect("UTF-8 temporary path");
    let args = [
        "20260809",
        "work",
        "120000_300",
        "screen",
        frame_ids,
        "--out",
        output,
        "--force",
    ];
    let json = with_json(&args);
    let python = fixture.python(&json);
    assert_success(&python, "Python", &json);
    let python_paths = saved_paths(frame_ids, Path::new(output));
    let python_pngs: Vec<_> = python_paths
        .iter()
        .map(|path| fs::read(path).expect("read Python PNG"))
        .collect();
    let native = fixture.native(&json);
    assert_success(&native, "native", &json);
    assert_eq!(
        native.stdout, python.stdout,
        "save JSON differs for {frame_ids}"
    );
    for (path, expected) in python_paths.iter().zip(&python_pngs) {
        // The selected first frame carries box_2d. Both implementations disable
        // annotation for grab. PNG encoders choose their own lossless compression
        // and filtering, so exact decoded pixels are the end-to-end proof that
        // neither save path adds drawing; see.py's annotation path is out of scope.
        let expected = ImageReader::new(Cursor::new(expected))
            .with_guessed_format()
            .expect("detect Python PNG format")
            .decode()
            .expect("decode Python PNG")
            .into_rgb8()
            .into_raw();
        let actual = ImageReader::open(path)
            .expect("open native PNG")
            .decode()
            .expect("decode native PNG")
            .into_rgb8()
            .into_raw();
        assert_eq!(actual, expected, "{path:?}");
    }
    let human = plain(&args);
    let python = fixture.python(&human);
    let native = fixture.native(&human);
    assert_success(&python, "Python", &human);
    assert_success(&native, "native", &human);
    assert_eq!(
        native.stdout, python.stdout,
        "save human output differs for {frame_ids}"
    );
}

fn saved_paths(frame_ids: &str, output: &Path) -> Vec<PathBuf> {
    let ids: Vec<_> = frame_ids.split(',').collect();
    if ids.len() == 1 {
        return vec![output.to_path_buf()];
    }
    let stem = output.file_stem().expect("output stem").to_string_lossy();
    let extension = output
        .extension()
        .expect("output extension")
        .to_string_lossy();
    ids.iter()
        .map(|id| output.with_file_name(format!("{stem}_{id}.{extension}")))
        .collect()
}

#[test]
fn rich_fixture_matches_json_and_human_payloads() {
    let fixture = Fixture::new();
    for args in [
        &[][..],
        &["20260809"][..],
        &["20260809", "work"][..],
        &["20260809", "work", "120000_300"][..],
        &["20260809", "work", "120000_300", "screen"][..],
        &["20260809", "work", "120000_300", "screen", "11"][..],
        &["20260809", "work", "120000_300", "probe", "7"][..],
        &["20260809", "work", "235900_300", "midnight", "1"][..],
        &["20260809", "work", "120000_300", "purged"][..],
        &["20260809", "work", "120000_300", "legacy"][..],
        &["20260809", "work", "120000_300", "header_only"][..],
        &["20260809", "work", "120000_300", "corrupt"][..],
    ] {
        compare_success(&fixture, args);
    }
}

#[test]
fn save_levels_match_and_preserve_decoded_pixels() {
    let fixture = Fixture::new();
    compare_save(&fixture, "1", &fixture.journal.join("single.png"));
    compare_save(&fixture, "1,2", &fixture.journal.join("batch.png"));
}

#[test]
fn failures_and_usage_classes_match_the_reference() {
    let fixture = Fixture::new();
    for args in [
        &["99999999"][..],
        &["20260809", "missing"][..],
        &["20260809", "work", "missing_segment"][..],
        &["20260809", "work", "120000_300", "missing"][..],
        &["20260809", "work", "120000_300", "screen", "99"][..],
        &[
            "20260809",
            "work",
            "120000_300",
            "purged",
            "1",
            "--out",
            "/tmp/purged.png",
        ][..],
        &["20260809", "work", "120000_300", "legacy", "1"][..],
        &["20260809", "work", "120000_300", "captured"][..],
    ] {
        compare_runtime_failure(&fixture, args);
    }
    assert_usage_case(
        &fixture,
        &["a", "b", "c", "d", "e", "f", "--force"],
        "grab accepts at most 5 positional tokens: day stream segment screen frame-id",
    );
    assert_usage_case(&fixture, &["--force"], "--force requires --out");
    assert_usage_case(
        &fixture,
        &["20260809", "--out", "out.png"],
        "--out requires day stream segment screen and frame-id",
    );
    assert_usage_case(
        &fixture,
        &[
            "20260809",
            "work",
            "120000_300",
            "screen",
            "1",
            "--out",
            "out.gif",
        ],
        "--out must end in .png, .jpg, .jpeg, or .webp",
    );
    assert_usage_case(
        &fixture,
        &["20260809", "work", "120000_300", "screen", "1,2"],
        "multiple frame ids require --out",
    );
    for args in [
        &["20260809", "--json", "work"][..],
        &["20260809", "work", "120000_300", "screen", "0"][..],
        &["20260809", "work", "120000_300", "screen", "-1"][..],
        &["20260809", "work", "120000_300", "screen", "wat"][..],
        &["20260809", "work", "120000_300", "screen", "2,2"][..],
    ] {
        let invocation = plain(args);
        let python = fixture.python(&invocation);
        let native = fixture.native(&invocation);
        assert_eq!(native.status.code(), python.status.code(), "{invocation:?}");
        assert!(matches!(python.status.code(), Some(1) | Some(2)));
    }
}
