// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};
use solstone_core_processing_record::media::expected_handler;
use solstone_core_processing_record::predicate::{TerminalProofOutcome, evaluate_terminal_proof};
use solstone_core_processing_record::vocab::{
    HANDLER_DESCRIBE, HANDLER_TRANSCRIBE, MAX_FIRST_ROW_BYTES,
};
use tempfile::TempDir;

use crate::classify::{Outcome, classify, plan};
use crate::{AtomicWriter, Writer, commit_eligible, exit_code, run};

const FIXTURE: &str = include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");
const INSTANT: &str = "2026-02-03T04:05:06Z";

struct Fixture {
    temp: TempDir,
    day: String,
    screen: PathBuf,
    audio: PathBuf,
    chunk: PathBuf,
    marked: PathBuf,
    torn: PathBuf,
    invalid: PathBuf,
    leading: PathBuf,
    recorded: PathBuf,
    symlink_target: PathBuf,
    directory_candidate: PathBuf,
}

impl Fixture {
    fn journal(&self) -> &Path {
        self.temp.path()
    }
}

fn instant() -> DateTime<Utc> {
    INSTANT.parse().expect("test instant is valid")
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn segment(journal: &Path, day: &str, stream: Option<&str>, key: &str) -> PathBuf {
    let mut path = journal.join("chronicle").join(day);
    if let Some(stream) = stream {
        path.push(stream);
    }
    path.push(key);
    fs::create_dir_all(&path).expect("create segment");
    path
}

fn json_line(value: Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&value).expect("serialize json");
    bytes.push(b'\n');
    bytes
}

fn write_sidecar(
    segment: &Path,
    name: &str,
    extension: &str,
    header: Value,
    size: usize,
) -> PathBuf {
    fs::write(
        segment.join(format!("{name}.{extension}")),
        vec![b'x'; size],
    )
    .expect("write media");
    let sidecar = segment.join(format!("{name}.jsonl"));
    fs::write(&sidecar, json_line(header)).expect("write sidecar");
    sidecar
}

fn setup_fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("temporary journal");
    let journal = temp.path();
    let day = "20990101".to_owned();

    let default_segment = segment(journal, &day, None, "080000_300");
    write_sidecar(
        &default_segment,
        "default_screen",
        "mp4",
        json!({"raw":"default_screen.mp4"}),
        10,
    );

    let screen_segment = segment(journal, &day, Some("alpha"), "090000_300");
    let screen = write_sidecar(
        &screen_segment,
        "screen",
        "mp4",
        json!({"raw":"screen.mp4", "café":"naïve", "keep": 7}),
        11,
    );

    let audio_segment = segment(journal, &day, Some("alpha"), "090100_300");
    let audio = write_sidecar(
        &audio_segment,
        "audio",
        "flac",
        json!({"raw":"audio.flac"}),
        23,
    );

    let recorded_segment = segment(journal, &day, Some("alpha"), "090200_300");
    let recorded = write_sidecar(
        &recorded_segment,
        "recorded_screen",
        "mp4",
        json!({"_solstone_processing":{"schema":"wrong", "state":"analyzed"}}),
        3,
    );

    let chunk_segment = segment(journal, &day, Some("alpha"), "090300_300");
    let chunk = write_sidecar(&chunk_segment, "chunk_screen", "mp4", json!({"raw":"x"}), 4);
    let mut chunk_bytes = fs::read(&chunk).expect("read chunk");
    chunk_bytes.extend_from_slice(b"\n{\"other\": 1}\n{\"timestamp\": 2}\n");
    fs::write(&chunk, chunk_bytes).expect("write chunk rows");

    let marked_segment = segment(journal, &day, Some("alpha"), "090400_300");
    let marked = write_sidecar(
        &marked_segment,
        "marked_audio",
        "flac",
        json!({"raw":"x"}),
        5,
    );
    fs::write(marked_segment.join(".analyzing_audio"), b"{}").expect("write marker");

    let absent_segment = segment(journal, &day, Some("alpha"), "090500_300");
    fs::write(
        absent_segment.join("missing_screen.jsonl"),
        json_line(json!({"raw":"x"})),
    )
    .expect("write missing sibling");

    let duplicate_segment = segment(journal, &day, Some("alpha"), "090600_300");
    fs::write(
        duplicate_segment.join("two_audio.jsonl"),
        json_line(json!({"raw":"x"})),
    )
    .expect("write duplicate sidecar");
    fs::write(duplicate_segment.join("two_audio.flac"), b"a").expect("write first duplicate");
    fs::write(duplicate_segment.join("two_audio.wav"), b"b").expect("write second duplicate");

    let import_segment = segment(journal, &day, Some("alpha"), "090700_300");
    write_sidecar(
        &import_segment,
        "imported_audio",
        "flac",
        json!({"raw":"x"}),
        2,
    );
    fs::write(
        import_segment.join("stream.json"),
        b"{\"stream\":\"import.audio\"}",
    )
    .expect("write stream marker");

    let meta_segment = segment(journal, &day, Some("alpha"), "090800_300");
    fs::write(
        meta_segment.join("meta.jsonl"),
        json_line(json!({"meta":true})),
    )
    .expect("write meta");
    fs::write(
        meta_segment.join("Screen.jsonl"),
        json_line(json!({"meta":true})),
    )
    .expect("write case shape");

    let upper_segment = segment(journal, &day, Some("alpha"), "090900_300");
    write_sidecar(
        &upper_segment,
        "clip_screen",
        "MP4",
        json!({"raw":"clip"}),
        9,
    );

    let directory_segment = segment(journal, &day, Some("alpha"), "091000_300");
    let directory_candidate = directory_segment.join("screen.jsonl");
    fs::create_dir(&directory_candidate).expect("create directory candidate");

    let link_segment = segment(journal, &day, Some("alpha"), "091100_300");
    let symlink_target = journal.join("symlink-source.jsonl");
    fs::write(&symlink_target, b"not a candidate\n").expect("write symlink target");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&symlink_target, link_segment.join("audio.jsonl"))
        .expect("create candidate symlink");

    let array_segment = segment(journal, &day, Some("alpha"), "091200_300");
    write_sidecar(&array_segment, "array_screen", "mp4", json!([]), 1);

    let leading_segment = segment(journal, &day, Some("alpha"), "091300_300");
    let leading = write_sidecar(
        &leading_segment,
        "leading_audio",
        "flac",
        json!({"raw":"x"}),
        8,
    );
    let mut leading_bytes = b"\n \t\n".to_vec();
    leading_bytes.extend_from_slice(&fs::read(&leading).expect("read leading"));
    fs::write(&leading, leading_bytes).expect("write leading blanks");

    let invalid_segment = segment(journal, &day, Some("alpha"), "091400_300");
    fs::write(invalid_segment.join("invalid_audio.flac"), b"x").expect("write invalid media");
    let invalid = invalid_segment.join("invalid_audio.jsonl");
    fs::write(&invalid, b"{\xff}\n").expect("write invalid utf8");

    let torn_segment = segment(journal, &day, Some("alpha"), "091500_300");
    let torn = write_sidecar(&torn_segment, "torn_screen", "mp4", json!({"raw":"x"}), 6);
    fs::write(&torn, b"{\"raw\":\"x\"}\n{\"timestamp\":1}\ntorn\n").expect("write torn rows");

    let oversized_segment = segment(journal, &day, Some("alpha"), "091600_300");
    let oversized = oversized_segment.join("large_screen.jsonl");
    fs::write(oversized_segment.join("large_screen.mp4"), b"x").expect("write oversized media");
    let large = "x".repeat(MAX_FIRST_ROW_BYTES - 32);
    fs::write(&oversized, json_line(json!({"raw":large}))).expect("write oversized header");

    let noise_segment = segment(journal, &day, Some("alpha"), "091700_300");
    fs::write(noise_segment.join("noise.flac"), b"x").expect("write ignored media");
    fs::write(
        noise_segment.join("stream.json"),
        b"{\"stream\":\"ignored\"}",
    )
    .expect("write ignored stream");
    fs::write(noise_segment.join("device.json"), b"{}").expect("write ignored device");

    let current_day = instant().with_timezone(&Local).format("%Y%m%d").to_string();
    let current_segment = segment(journal, &current_day, None, "100000_300");
    write_sidecar(
        &current_segment,
        "today_screen",
        "mp4",
        json!({"raw":"x"}),
        4,
    );

    Fixture {
        temp,
        day,
        screen,
        audio,
        chunk,
        marked,
        torn,
        invalid,
        leading,
        recorded,
        symlink_target,
        directory_candidate,
    }
}

fn invoke(fixture: &Fixture, values: &[&str], writer: &dyn Writer) -> (i32, String, String) {
    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    let exit = run(
        &args(values),
        fixture.journal(),
        instant(),
        writer,
        &mut stdout,
        &mut stderr,
    );
    (
        exit,
        String::from_utf8(stdout.into_inner()).expect("stdout utf8"),
        String::from_utf8(stderr.into_inner()).expect("stderr utf8"),
    )
}

fn expected_counts(mode: &str, total: u64, values: &[(&str, u64)]) -> String {
    let counts = BTreeMap::from_iter(values.iter().copied());
    let order = [
        "stamp_empty",
        "skip_has_record",
        "skip_chunk_bearing",
        "skip_marker",
        "skip_ineligible",
        "skip_unreadable",
        "skip_oversize",
        "write_failed",
    ];
    let calculated_total: u64 = order
        .iter()
        .map(|name| counts.get(name).copied().unwrap_or(0))
        .sum();
    assert_eq!(calculated_total, total, "literal fixture total");
    let mut output = format!("{mode}\n");
    for name in order {
        output.push_str(&format!(
            "{name}: {}\n",
            counts.get(name).copied().unwrap_or(0)
        ));
    }
    output.push_str(&format!("total: {total}\n"));
    output
}

fn fixture_help() -> String {
    let block = FIXTURE
        .split_once("=== backfill-processing-records --help\n")
        .expect("fixture has help block")
        .1;
    block
        .split_once("\n=== misuse exit codes")
        .expect("fixture help block ends before misuse block")
        .0
        .to_owned()
}

#[test]
fn help_and_argument_refusals_are_fixture_faithful() {
    let fixture = setup_fixture();
    let writer = AtomicWriter;
    let (exit, stdout, stderr) = invoke(&fixture, &["--help"], &writer);
    assert_eq!(exit, 0);
    assert_eq!(stdout, fixture_help());
    assert_eq!(stderr, "");

    let (exit, stdout, stderr) = invoke(&fixture, &["-h"], &writer);
    assert_eq!(exit, 0);
    assert_eq!(stdout, fixture_help());
    assert_eq!(stderr, "");

    let (exit, stdout, stderr) = invoke(&fixture, &["--nonsense"], &writer);
    assert_eq!(exit, 2);
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        concat!(
            "usage: journal backfill-processing-records [-h] [--day DAY] [--commit |\n",
            "                                           --dry-run] [-v] [-d]\n",
            "journal backfill-processing-records: error: unrecognized arguments: --nonsense\n",
        )
    );

    let (exit, stdout, stderr) = invoke(&fixture, &["--commit", "--dry-run"], &writer);
    assert_eq!(exit, 2);
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        concat!(
            "usage: journal backfill-processing-records [-h] [--day DAY] [--commit |\n",
            "                                           --dry-run] [-v] [-d]\n",
            "journal backfill-processing-records: error: argument --dry-run: not allowed with argument --commit\n",
        )
    );

    let (exit, stdout, stderr) = invoke(&fixture, &["--day", "not-a-day"], &writer);
    assert_eq!(exit, 1);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "expected day in YYYYMMDD format\n");
}

#[test]
fn fixture_exercises_all_eight_outcomes_and_flags_are_inert() {
    let fixture = setup_fixture();
    let writer = AtomicWriter;
    let expected = expected_counts(
        "DRY RUN (no changes written)",
        20,
        &[
            ("stamp_empty", 5),
            ("skip_has_record", 1),
            ("skip_chunk_bearing", 1),
            ("skip_marker", 1),
            ("skip_ineligible", 8),
            ("skip_unreadable", 3),
            ("skip_oversize", 1),
            ("write_failed", 0),
        ],
    );
    let (exit, stdout, stderr) = invoke(&fixture, &[], &writer);
    assert_eq!(exit, 0);
    assert_eq!(stdout, expected);
    assert_eq!(stderr, "");

    let (exit, stdout, stderr) = invoke(&fixture, &["-v", "--verbose", "-d", "--debug"], &writer);
    assert_eq!(exit, 0);
    assert_eq!(stdout, expected);
    assert_eq!(stderr, "");
}

#[test]
fn commit_stamps_ordered_records_preserves_other_bytes_and_is_idempotent() {
    let fixture = setup_fixture();
    let writer = AtomicWriter;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    let screen_mode = fs::metadata(&fixture.screen)
        .expect("screen metadata")
        .permissions()
        .mode();
    #[cfg(unix)]
    let audio_mode = fs::metadata(&fixture.audio)
        .expect("audio metadata")
        .permissions()
        .mode();
    let (exit, stdout, stderr) = invoke(&fixture, &["--commit"], &writer);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout,
        expected_counts(
            "COMMITTED",
            20,
            &[
                ("stamp_empty", 5),
                ("skip_has_record", 1),
                ("skip_chunk_bearing", 1),
                ("skip_marker", 1),
                ("skip_ineligible", 8),
                ("skip_unreadable", 3),
                ("skip_oversize", 1),
                ("write_failed", 0),
            ],
        )
    );
    assert_eq!(stderr, "");

    for (path, handler, reason, size) in [
        (&fixture.screen, HANDLER_DESCRIBE, "no_decodable_frames", 11),
        (&fixture.audio, HANDLER_TRANSCRIBE, "no_decodable_audio", 23),
    ] {
        let text = fs::read_to_string(path).expect("stamped sidecar utf8");
        let first = text.lines().next().expect("header");
        let value: Value = serde_json::from_str(first).expect("header json");
        let record = value["_solstone_processing"].clone();
        let keys: Vec<_> = record.as_object().expect("record object").keys().collect();
        assert_eq!(
            keys,
            [
                "schema",
                "state",
                "reason_code",
                "handler",
                "attempted_at",
                "input_size",
                "source"
            ]
        );
        assert_eq!(record["handler"], handler);
        assert_eq!(record["reason_code"], reason);
        assert_eq!(record["input_size"], size);
        assert_eq!(record["attempted_at"], INSTANT);
        assert_eq!(record["source"], "backfill");
        let media_extension = if handler == HANDLER_DESCRIBE {
            "mp4"
        } else {
            "flac"
        };
        assert_eq!(
            evaluate_terminal_proof(
                Some(&record),
                expected_handler(media_extension).expect("expected handler"),
                size,
            ),
            TerminalProofOutcome::Held
        );
    }
    let screen_header: Value = serde_json::from_str(
        fs::read_to_string(&fixture.screen)
            .expect("screen text")
            .lines()
            .next()
            .expect("screen header"),
    )
    .expect("screen json");
    let keys: Vec<_> = screen_header
        .as_object()
        .expect("screen map")
        .keys()
        .collect();
    assert_eq!(keys, ["raw", "café", "keep", "_solstone_processing"]);
    assert_eq!(screen_header["café"], "naïve");
    assert_eq!(screen_header["keep"], 7);
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&fixture.screen)
            .expect("screen mode")
            .permissions()
            .mode(),
        screen_mode
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&fixture.audio)
            .expect("audio mode")
            .permissions()
            .mode(),
        audio_mode
    );

    for path in [&fixture.chunk, &fixture.marked, &fixture.torn] {
        let header: Value = serde_json::from_str(
            fs::read_to_string(path)
                .expect("known utf8 fixture")
                .lines()
                .next()
                .expect("header"),
        )
        .expect("header json");
        assert_eq!(
            evaluate_terminal_proof(header.get("_solstone_processing"), HANDLER_DESCRIBE, 0,),
            TerminalProofOutcome::RecordAbsent
        );
    }
    let recorded_header: Value = serde_json::from_str(
        fs::read_to_string(&fixture.recorded)
            .expect("recorded text")
            .lines()
            .next()
            .expect("recorded header"),
    )
    .expect("recorded json");
    assert_eq!(
        evaluate_terminal_proof(
            recorded_header.get("_solstone_processing"),
            HANDLER_DESCRIBE,
            3,
        ),
        TerminalProofOutcome::SchemaUnrecognized
    );
    assert_eq!(
        fs::read(&fixture.torn).expect("torn bytes"),
        b"{\"raw\":\"x\"}\n{\"timestamp\":1}\ntorn\n"
    );
    assert_eq!(
        fs::read(&fixture.invalid).expect("invalid bytes"),
        b"{\xff}\n"
    );
    assert!(fixture.directory_candidate.is_dir());
    assert_eq!(
        fs::read(&fixture.symlink_target).expect("symlink target"),
        b"not a candidate\n"
    );
    let leading_bytes = fs::read(&fixture.leading).expect("leading bytes");
    assert!(leading_bytes.starts_with(b"\n \t\n"));
    assert!(leading_bytes.ends_with(b"\n"));

    let (exit, stdout, stderr) = invoke(&fixture, &["--commit", "--day", &fixture.day], &writer);
    assert_eq!(exit, 0);
    assert!(stdout.contains("skip_has_record: 6\n"));
    assert_eq!(stderr, "");
    assert_no_temporary_files(fixture.journal().join("chronicle"));
}

fn assert_no_temporary_files(directory: PathBuf) {
    for entry in fs::read_dir(directory).expect("directory entries") {
        let entry = entry.expect("entry");
        let path = entry.path();
        assert!(!entry.file_name().to_string_lossy().starts_with(".tmp_"));
        if path.is_dir() {
            assert_no_temporary_files(path);
        }
    }
}

struct FailingWriter;

impl Writer for FailingWriter {
    fn replace(&self, _path: &Path, _contents: &[u8]) -> Result<(), String> {
        Err("injected failure".to_owned())
    }
}

#[test]
fn writer_failure_and_re_read_guard_are_independent() {
    let fixture = setup_fixture();
    let dry = invoke(&fixture, &[], &AtomicWriter);
    let (exit, stdout, stderr) = invoke(&fixture, &["--commit"], &FailingWriter);
    assert_eq!(exit, 3);
    assert_eq!(stderr.matches("Could not stamp ").count(), 5);
    assert!(stderr.contains("injected failure"));
    assert_eq!(
        stdout,
        expected_counts(
            "COMMITTED",
            20,
            &[
                ("stamp_empty", 0),
                ("skip_has_record", 1),
                ("skip_chunk_bearing", 1),
                ("skip_marker", 1),
                ("skip_ineligible", 8),
                ("skip_unreadable", 3),
                ("skip_oversize", 1),
                ("write_failed", 5),
            ],
        )
    );
    assert_eq!(
        dry.1,
        expected_counts(
            "DRY RUN (no changes written)",
            20,
            &[
                ("stamp_empty", 5),
                ("skip_has_record", 1),
                ("skip_chunk_bearing", 1),
                ("skip_marker", 1),
                ("skip_ineligible", 8),
                ("skip_unreadable", 3),
                ("skip_oversize", 1),
                ("write_failed", 0),
            ],
        )
    );

    let mut sink = Cursor::new(Vec::new());
    let mut report =
        plan(fixture.journal(), Some(&fixture.day), instant(), &mut sink).expect("fixture plan");
    assert_eq!(report.counts.write_failed, 0);
    fs::write(&fixture.screen, b"mutated after planning\n").expect("mutate real file");
    commit_eligible(fixture.journal(), &mut report, &AtomicWriter, &mut sink);
    assert_eq!(report.counts.write_failed, 1);
    assert_eq!(exit_code(true, &report), 3);
    assert_eq!(
        fs::read(&fixture.screen).expect("mutation survives"),
        b"mutated after planning\n"
    );
}

#[test]
fn missing_day_and_single_day_default_stream_are_distinguishable() {
    let fixture = setup_fixture();
    let writer = AtomicWriter;
    let (exit, stdout, stderr) = invoke(&fixture, &["--day", "20991231"], &writer);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout,
        expected_counts("DRY RUN (no changes written)", 0, &[])
    );
    assert_eq!(stderr, "Day 20991231 was not found in the journal\n");

    let current_day = instant().with_timezone(&Local).format("%Y%m%d").to_string();
    let (exit, stdout, stderr) = invoke(&fixture, &["--day", &current_day], &writer);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout,
        expected_counts("DRY RUN (no changes written)", 1, &[("skip_ineligible", 1)],)
    );
    assert_eq!(stderr, "");

    let (exit, stdout, stderr) = invoke(&fixture, &["--day", &fixture.day], &writer);
    assert_eq!(exit, 0);
    assert!(stdout.contains("stamp_empty: 5\n"));
    assert_eq!(stderr, "");
}

#[test]
fn missing_sibling_after_listing_is_unreadable() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let segment = temp.path().join("segment");
    fs::create_dir(&segment).expect("create segment");
    let sidecar = write_sidecar(&segment, "audio", "flac", json!({"raw":"x"}), 1);
    let entries = solstone_core_journal_io::list_dir_entries(&segment).expect("entries");
    fs::remove_file(segment.join("audio.flac")).expect("remove sibling");
    let candidate = entries
        .iter()
        .find(|entry| entry.path == sidecar)
        .expect("candidate");
    assert!(matches!(
        classify(
            "20990101",
            "20000101",
            &segment,
            "alpha",
            candidate,
            &entries,
            instant()
        ),
        Err(Outcome::SkipUnreadable)
    ));
}

#[test]
fn regular_file_in_stream_position_is_reported_without_aborting_other_days() {
    let fixture = setup_fixture();
    let broken_day = "20990102";
    let path = fixture
        .journal()
        .join("chronicle")
        .join(broken_day)
        .join("alpha");
    fs::create_dir_all(path.parent().expect("chronicle day")).expect("create day");
    fs::write(&path, b"not a directory").expect("write stream-shaped file");

    let (exit, stdout, stderr) = invoke(&fixture, &[], &AtomicWriter);
    assert_eq!(exit, 0);
    assert!(stdout.contains("stamp_empty: 5\n"));
    assert!(stderr.contains(&format!(
        "Could not list stream directory {}: not a directory",
        path.display()
    )));
}

struct CountingWriter {
    inner: AtomicWriter,
    calls: std::sync::atomic::AtomicU64,
}

impl Writer for CountingWriter {
    fn replace(&self, path: &Path, contents: &[u8]) -> Result<(), String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.replace(path, contents)
    }
}

fn eligible_sidecar(journal: &Path, stream: Option<&str>, key: &str, stem: &str) -> PathBuf {
    let path = segment(journal, "20990101", stream, key);
    write_sidecar(
        &path,
        stem,
        "mp4",
        json!({"raw": format!("{stem}.mp4")}),
        10,
    )
}

#[test]
fn named_default_blocks_commit_for_the_whole_plan() {
    let temp = tempfile::tempdir().expect("temporary journal");
    let journal = temp.path();
    let direct = eligible_sidecar(journal, None, "080000_300", "direct_screen");
    let named_segment = segment(journal, "20990101", Some("_default"), "090000_300");
    let named = write_sidecar(
        &named_segment,
        "named_screen",
        "mp4",
        json!({"raw": "named_screen.mp4"}),
        10,
    );
    let before_direct = fs::read(&direct).expect("direct sidecar");
    let before_named = fs::read(&named).expect("named sidecar");
    let writer = CountingWriter {
        inner: AtomicWriter,
        calls: std::sync::atomic::AtomicU64::new(0),
    };
    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    let exit = run(
        &args(&["--commit", "--day", "20990101"]),
        journal,
        instant(),
        &writer,
        &mut stdout,
        &mut stderr,
    );
    let stdout = String::from_utf8(stdout.into_inner()).expect("stdout utf8");
    let stderr = String::from_utf8(stderr.into_inner()).expect("stderr utf8");
    assert_eq!(exit, 1, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr
            .contains("named stream directory \"_default\" cannot be spelled as a record identity"),
        "{stderr}"
    );
    assert!(!stdout.contains("COMMITTED"), "{stdout}");
    assert!(!stdout.contains("DRY RUN"), "{stdout}");
    assert_eq!(writer.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(fs::read(&direct).expect("direct unchanged"), before_direct);
    assert_eq!(fs::read(&named).expect("named unchanged"), before_named);
}

#[test]
fn stream_json_does_not_launder_named_default_into_eligibility() {
    let temp = tempfile::tempdir().expect("temporary journal");
    let journal = temp.path();
    eligible_sidecar(journal, None, "080000_300", "direct_screen");
    let named_segment = segment(journal, "20990101", Some("_default"), "090000_300");
    write_sidecar(
        &named_segment,
        "named_screen",
        "mp4",
        json!({"raw": "named_screen.mp4"}),
        10,
    );
    fs::write(named_segment.join("stream.json"), b"{\"stream\":\"work\"}").expect("stream marker");
    let writer = CountingWriter {
        inner: AtomicWriter,
        calls: std::sync::atomic::AtomicU64::new(0),
    };
    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    let exit = run(
        &args(&["--commit", "--day", "20990101"]),
        journal,
        instant(),
        &writer,
        &mut stdout,
        &mut stderr,
    );
    let stderr = String::from_utf8(stderr.into_inner()).expect("stderr utf8");
    assert_eq!(exit, 1, "{stderr}");
    assert!(
        stderr
            .contains("named stream directory \"_default\" cannot be spelled as a record identity"),
        "{stderr}"
    );
    assert_eq!(writer.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn direct_only_commit_still_stamps() {
    let temp = tempfile::tempdir().expect("temporary journal");
    let journal = temp.path();
    let direct = eligible_sidecar(journal, None, "080000_300", "direct_screen");
    let before = fs::read(&direct).expect("direct sidecar");
    let writer = CountingWriter {
        inner: AtomicWriter,
        calls: std::sync::atomic::AtomicU64::new(0),
    };
    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    let exit = run(
        &args(&["--commit", "--day", "20990101"]),
        journal,
        instant(),
        &writer,
        &mut stdout,
        &mut stderr,
    );
    let stdout = String::from_utf8(stdout.into_inner()).expect("stdout utf8");
    let stderr = String::from_utf8(stderr.into_inner()).expect("stderr utf8");
    assert_eq!(exit, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("COMMITTED"), "{stdout}");
    assert_eq!(stderr, "");
    assert_eq!(writer.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let after = fs::read(&direct).expect("stamped sidecar");
    assert_ne!(after, before);
}

fn stream_generation(journal: &Path, day: &str) -> Option<u64> {
    match read_health_marker(journal, day, HealthMarkerKind::Stream).expect("read stream marker") {
        HealthMarkerState::Versioned { marker, .. } => Some(marker.generation),
        HealthMarkerState::Absent => None,
        state => panic!("unexpected stream marker state: {state:?}"),
    }
}

#[test]
fn commit_dirties_exact_rewritten_days_after_each_atomic_replacement() {
    let temp = tempfile::tempdir().expect("temporary journal");
    let journal = temp.path();
    let first_day = "20990101";
    let second_day = "20990102";
    let untouched_day = "20990103";

    for (day, key, stem) in [
        (first_day, "080000_300", "first_screen"),
        (first_day, "080100_300", "second_screen"),
        (second_day, "090000_300", "third_screen"),
    ] {
        let segment = segment(journal, day, Some("alpha"), key);
        write_sidecar(
            &segment,
            stem,
            "mp4",
            json!({"raw": format!("{stem}.mp4")}),
            10,
        );
    }
    fs::create_dir_all(journal.join("chronicle").join(untouched_day))
        .expect("create untouched day");

    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    let exit = run(
        &args(&["--commit"]),
        journal,
        instant(),
        &AtomicWriter,
        &mut stdout,
        &mut stderr,
    );

    assert_eq!(exit, 0, "{}", String::from_utf8_lossy(stderr.get_ref()));
    assert_eq!(stream_generation(journal, first_day), Some(2));
    assert_eq!(stream_generation(journal, second_day), Some(1));
    assert_eq!(stream_generation(journal, untouched_day), None);
}

#[test]
fn marker_failure_is_terminal_after_retaining_the_rewritten_sidecar() {
    let temp = tempfile::tempdir().expect("temporary journal");
    let journal = temp.path();
    let day = "20990101";
    let segment = segment(journal, day, Some("alpha"), "080000_300");
    let sidecar = write_sidecar(&segment, "screen", "mp4", json!({"raw": "screen.mp4"}), 10);
    fs::create_dir_all(
        journal
            .join("chronicle")
            .join(day)
            .join("health")
            .join("stream.updated"),
    )
    .expect("block stream marker with directory");

    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    let exit = run(
        &args(&["--commit", "--day", day]),
        journal,
        instant(),
        &AtomicWriter,
        &mut stdout,
        &mut stderr,
    );
    let stdout = String::from_utf8(stdout.into_inner()).expect("stdout utf8");
    let stderr = String::from_utf8(stderr.into_inner()).expect("stderr utf8");

    assert_eq!(exit, 3, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("stamp_empty: 0\n"), "{stdout}");
    assert!(stdout.contains("write_failed: 1\n"), "{stdout}");
    assert!(
        stderr.contains(&format!(
            "Stamped {}, but could not mark day {day} updated:",
            sidecar.display()
        )),
        "{stderr}"
    );
    let first_row = fs::read_to_string(&sidecar)
        .expect("rewritten sidecar retained")
        .lines()
        .next()
        .expect("header row")
        .to_owned();
    let header: Value = serde_json::from_str(&first_row).expect("header json");
    assert!(header.get("_solstone_processing").is_some());
}
