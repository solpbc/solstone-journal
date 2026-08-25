// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use image::ImageReader;
use serde_json::Value;

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
            "solstone-core-grab-cli-{}-{stamp}",
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
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new();
        let journal = temp.path.join("journal");
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
        }
    }

    fn native(&self, args: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .arg("grab")
            .args(args)
            .env("SOLSTONE_JOURNAL", &self.journal)
            .output()
            .expect("run native grab")
    }
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
        .expect("ffmpeg must be available for grab fixtures");
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

fn key(args: &[&str]) -> String {
    args.join(" ")
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("grab output is UTF-8")
}

fn assert_success(output: &Output, args: &[String]) {
    assert!(
        output.status.success(),
        "native failed for {args:?}:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn corpus() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/grab_cli_payloads.json"))
        .expect("frozen grab corpus parses")
}

const SUCCESS_CASES: &[&[&str]] = &[
    &[],
    &["20260809"],
    &["20260809", "work"],
    &["20260809", "work", "120000_300"],
    &["20260809", "work", "120000_300", "screen"],
    &["20260809", "work", "120000_300", "screen", "11"],
    &["20260809", "work", "120000_300", "probe", "7"],
    &["20260809", "work", "235900_300", "midnight", "1"],
    &["20260809", "work", "120000_300", "purged"],
    &["20260809", "work", "120000_300", "legacy"],
    &["20260809", "work", "120000_300", "header_only"],
    &["20260809", "work", "120000_300", "corrupt"],
];

const RUNTIME_FAILURES: &[&[&str]] = &[
    &["99999999"],
    &["20260809", "missing"],
    &["20260809", "work", "missing_segment"],
    &["20260809", "work", "120000_300", "missing"],
    &["20260809", "work", "120000_300", "screen", "99"],
    &[
        "20260809",
        "work",
        "120000_300",
        "purged",
        "1",
        "--out",
        "/tmp/purged.png",
    ],
    &["20260809", "work", "120000_300", "legacy", "1"],
    &["20260809", "work", "120000_300", "captured"],
];

const MIXED_CASES: &[(&[&str], i32)] = &[
    (&["20260809", "--json", "work"], 2),
    (&["20260809", "work", "120000_300", "screen", "0"], 1),
    (&["20260809", "work", "120000_300", "screen", "-1"], 1),
    (&["20260809", "work", "120000_300", "screen", "wat"], 1),
    (&["20260809", "work", "120000_300", "screen", "2,2"], 1),
];

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

fn decode_rgb8(path: &Path) -> Vec<u8> {
    ImageReader::open(path)
        .expect("open saved PNG")
        .decode()
        .expect("decode saved PNG")
        .into_rgb8()
        .into_raw()
}

fn hex_pixels(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn mask_journal(text: &str, journal: &Path) -> String {
    text.replace(&journal.display().to_string(), "{journal}")
}

fn case<'a>(corpus: &'a Value, section: &str, key: &str) -> &'a Value {
    corpus
        .get(section)
        .and_then(|section| section.get(key))
        .unwrap_or_else(|| panic!("missing frozen case {section}/{key:?}"))
}

#[test]
fn rich_fixture_matches_json_and_human_payloads() {
    let fixture = Fixture::new();
    let corpus = corpus();
    for args in SUCCESS_CASES {
        let expected = case(&corpus, "success", &key(args));
        let json_out = fixture.native(&with_json(args));
        let human_out = fixture.native(&plain(args));
        assert_success(&json_out, &with_json(args));
        assert_success(&human_out, &plain(args));
        assert_eq!(utf8(&json_out.stdout), expected["json"], "json {args:?}");
        assert_eq!(utf8(&human_out.stdout), expected["human"], "human {args:?}");
    }
}

#[test]
fn save_levels_preserve_decoded_pixels() {
    let fixture = Fixture::new();
    let corpus = corpus();
    for frame_ids in ["1", "1,2"] {
        let out = fixture.journal.join(if frame_ids == "1" {
            "single.png"
        } else {
            "batch.png"
        });
        let args = [
            "20260809",
            "work",
            "120000_300",
            "screen",
            frame_ids,
            "--out",
            out.to_str().expect("UTF-8 path"),
            "--force",
        ];
        let expected = case(&corpus, "save", frame_ids);
        let json_out = fixture.native(&with_json(&args));
        assert_success(&json_out, &with_json(&args));
        assert_eq!(
            mask_journal(&utf8(&json_out.stdout), &fixture.journal),
            expected["json"],
            "save JSON {frame_ids}"
        );
        let ids = frame_ids
            .split(',')
            .map(|id| id.parse::<i64>().expect("fixture frame ID"))
            .collect::<Vec<_>>();
        let expected_pixels = solstone_core_grab::test_hooks::decode_frames(
            &fixture
                .journal
                .join("chronicle/20260809/work/120000_300/renamed.mp4"),
            &ids,
        )
        .expect("decode fixture video")
        .into_iter()
        .map(|frame| {
            hex_pixels(
                &frame
                    .expect("fixture video contains requested frame")
                    .pixels,
            )
        })
        .collect::<Vec<_>>();
        let actual_pixels = saved_paths(frame_ids, &out)
            .iter()
            .map(|path| hex_pixels(&decode_rgb8(path)))
            .collect::<Vec<_>>();
        assert_eq!(actual_pixels, expected_pixels, "pixels {frame_ids}");
        let human_out = fixture.native(&plain(&args));
        assert_success(&human_out, &plain(&args));
        assert_eq!(
            mask_journal(&utf8(&human_out.stdout), &fixture.journal),
            expected["human"],
            "save human {frame_ids}"
        );
    }
}

#[test]
fn failures_and_usage_classes_match_native_contract() {
    let fixture = Fixture::new();
    let corpus = corpus();
    for args in RUNTIME_FAILURES {
        let output = fixture.native(&plain(args));
        let expected = case(&corpus, "failures", &key(args));
        assert_eq!(output.status.code(), Some(1), "runtime {args:?}");
        assert_eq!(utf8(&output.stdout), expected["stdout"], "stdout {args:?}");
        assert_eq!(utf8(&output.stderr), expected["stderr"], "stderr {args:?}");
    }

    for (args, message) in [
        (
            &["a", "b", "c", "d", "e", "f", "--force"][..],
            "grab accepts at most 5 positional tokens: day stream segment screen frame-id",
        ),
        (&["--force"][..], "--force requires --out"),
        (
            &["20260809", "--out", "out.png"][..],
            "--out requires day stream segment screen and frame-id",
        ),
        (
            &[
                "20260809",
                "work",
                "120000_300",
                "screen",
                "1",
                "--out",
                "out.gif",
            ][..],
            "--out must end in .png, .jpg, .jpeg, or .webp",
        ),
        (
            &["20260809", "work", "120000_300", "screen", "1,2"][..],
            "multiple frame ids require --out",
        ),
    ] {
        let output = fixture.native(&plain(args));
        assert_eq!(output.status.code(), Some(2), "usage {args:?}");
        let stderr = utf8(&output.stderr);
        assert!(
            stderr.contains(message),
            "usage omitted {message:?}: {stderr}"
        );
    }

    for (args, exit) in MIXED_CASES {
        let output = fixture.native(&plain(args));
        assert_eq!(output.status.code(), Some(*exit), "mixed {args:?}");
    }
}
