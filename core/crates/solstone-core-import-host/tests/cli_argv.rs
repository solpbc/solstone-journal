// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use chrono::{NaiveDateTime, TimeZone};
use serde_json::Value;
use solstone_core_callosum::CallosumSocketServer;
use solstone_core_import::cli_render::CliRun;
use solstone_core_import::{ImportError, ObservingSegment};
use solstone_core_import_host::audio::{
    AudioImportRecord, AudioImportRequest, AudioImportSeams, ProcessingWaitFn,
    ProcessingWaitOutcome, import_audio_with_seams, native_processing_wait,
};

use super::audio::probed;
use solstone_core_import_host::cli_argv::{
    CliOutcome, audio_import_cli_run, audio_import_runtime, run_cli_with,
};
use solstone_core_segment::SUPERVISOR_MESSAGE;

#[test]
fn argv_parses_before_supervisor_preflight_and_preserves_exit_contract() {
    let unknown = run(&["--nonsense"], |_| None, || false);
    assert_eq!(unknown.exit_code, 2);
    assert!(
        unknown
            .stderr
            .contains("unrecognized arguments: --nonsense")
    );
    assert!(unknown.stderr.contains("usage: solstone journal importer"));

    let spawned = run(
        &["file"],
        |name| (name == "SOL_SUPERVISOR_SPAWNED").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(spawned.exit_code, 75);
    assert!(spawned.stderr.is_empty());

    let unavailable = run(&["file"], |_| None, || false);
    assert_eq!(unavailable.exit_code, 1);
    assert_eq!(unavailable.stderr, format!("{SUPERVISOR_MESSAGE}\n"));
}

#[test]
fn positional_timestamp_reaches_the_generic_dispatch() {
    let temporary = tempfile::tempdir().unwrap();
    let result = run_at(
        temporary.path(),
        &["file", "20260311_120000"],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );

    assert!(!result.stderr.contains("usage: solstone journal importer"));
    assert!(
        result.stderr.contains("source path does not exist"),
        "stderr={}",
        result.stderr
    );
}

#[test]
fn value_options_accept_attached_and_separated_values() {
    for arguments in [
        &["--timestamp=20260311_120000", "file"][..],
        &["--timestamp", "20260311_120000", "file"][..],
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let result = run_at(
            temporary.path(),
            arguments,
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );

        assert!(!result.stderr.contains("usage: solstone journal importer"));
        assert!(!result.stderr.contains("media"));
        assert!(
            result.stderr.contains("source path does not exist"),
            "stderr={}",
            result.stderr
        );
    }
}

#[test]
fn an_unusable_journal_path_is_named() {
    let missing = tempfile::tempdir().unwrap().path().join("absent");
    for journal in [Path::new("."), missing.as_path()] {
        let result = run_at(
            journal,
            &["file"],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(result.exit_code, 1);
        assert!(
            result
                .stderr
                .contains(&format!("invalid journal root {}", journal.display())),
            "stderr={}",
            result.stderr
        );
    }
}

#[test]
fn auto_does_not_swallow_a_path_positional() {
    let temporary = tempfile::tempdir().unwrap();
    let result = run_at(
        temporary.path(),
        &["--auto", "/tmp/solstone-cycle2-does-not-exist.md"],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 1);
    assert!(
        result
            .stderr
            .contains("import source is missing: /tmp/solstone-cycle2-does-not-exist.md"),
        "stderr={}",
        result.stderr
    );
    assert!(
        !result
            .stderr
            .contains("the following arguments are required: media")
    );
}

#[test]
fn unknown_attached_option_is_rejected() {
    let result = run(&["--nonsense=x", "file"], |_| None, || false);

    assert_eq!(result.exit_code, 2);
    assert!(result.stderr.contains("usage: solstone journal importer"));
}

#[test]
fn generic_text_timestamp_writes_a_segment_from_the_stamp() {
    let journal = tempfile::tempdir().unwrap();
    let note = journal.path().join("note.md");
    fs::write(&note, "a short imported note").unwrap();
    let result = run_at(
        journal.path(),
        &[
            "--timestamp",
            "20260818_062652",
            note.to_str().expect("utf-8 note path"),
        ],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 0, "stderr={}", result.stderr);
    assert!(
        result
            .stdout
            .contains("Generic text import complete: segments=1"),
        "stdout={}",
        result.stdout
    );
    assert!(
        result.stdout.contains("no turn times found in this file")
            && result.stdout.contains(
                "https://support.solstone.app/#report=v1&app=import&state=transcript%20times%20not%20recognized"
            ),
        "an untimed file says so and where to ask for its layout; stdout={}",
        result.stdout
    );
    assert!(
        journal
            .path()
            .join("chronicle/20260818/import.text/062652_300/conversation_transcript.jsonl")
            .is_file()
    );
    let marker: serde_json::Value = serde_json::from_slice(
        &fs::read(
            journal
                .path()
                .join("chronicle/20260818/health/stream.updated"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(marker["generation"], 2);
    assert!(
        !journal
            .path()
            .join("chronicle/20260819/health/stream.updated")
            .exists()
    );
}

#[test]
fn generic_text_v1_transcript_takes_its_times_from_the_file_with_no_model() {
    let journal = tempfile::tempdir().unwrap();
    let note = journal.path().join("meeting.md");
    fs::write(
        &note,
        "# Planning sync\n\n## 00:00:00\n**Ana Lima:** Let's begin.\n\n## 00:07:30\n**Ben Okafor:** The file keeps  my words.\n",
    )
    .unwrap();
    let result = run_at(
        journal.path(),
        &[
            "--timestamp",
            "20260818_100000",
            note.to_str().expect("utf-8 note path"),
        ],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 0, "stderr={}", result.stderr);
    assert!(
        result
            .stdout
            .contains("Generic text import complete: segments=2"),
        "stdout={}",
        result.stdout
    );
    assert!(
        !result.stdout.contains("no turn times found"),
        "a recognized layout is not called untimed; stdout={}",
        result.stdout
    );
    let rows = |segment: &str| -> Vec<serde_json::Value> {
        fs::read_to_string(journal.path().join(format!(
            "chronicle/20260818/import.text/{segment}/conversation_transcript.jsonl"
        )))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
    };
    let first = rows("100000_300");
    let source_ms = solstone_core_journal_config::owner_zone(journal.path())
        .from_local_datetime(
            &NaiveDateTime::parse_from_str("20260818_100000", "%Y%m%d_%H%M%S").unwrap(),
        )
        .earliest()
        .unwrap()
        .timestamp_millis();
    assert_eq!(
        first[1..],
        [
            serde_json::json!({"start": "00:00:00", "source_ms":source_ms, "text": "# Planning sync", "source": "import"}),
            serde_json::json!({"start": "00:00:00", "source_ms":source_ms, "speaker": "Ana Lima", "text": "Let's begin.", "source": "import"}),
        ]
    );
    let second = rows("100500_300");
    assert_eq!(
        second[1..],
        [
            serde_json::json!({"start": "00:02:30", "source_ms":source_ms + 450_000, "speaker": "Ben Okafor", "text": "The file keeps  my words.", "source": "import"})
        ]
    );
}

#[test]
fn generic_text_marker_failure_is_nonzero_and_retains_created_content() {
    let journal = tempfile::tempdir().unwrap();
    let note = journal.path().join("note.md");
    fs::write(&note, "a short imported note").unwrap();
    let marker = journal
        .path()
        .join("chronicle/20260818/health/stream.updated");
    fs::create_dir_all(&marker).unwrap();

    let result = run_at(
        journal.path(),
        &[
            "--timestamp",
            "20260818_062652",
            note.to_str().expect("utf-8 note path"),
        ],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );

    assert_ne!(result.exit_code, 0);
    assert!(result.stdout.is_empty());
    assert!(result.stderr.contains("could not advance stream marker"));
    assert!(result.stderr.contains(&marker.display().to_string()));
    assert!(
        journal
            .path()
            .join("chronicle/20260818/import.text/062652_300/conversation_transcript.jsonl")
            .is_file()
    );
}

#[test]
fn missing_timestamp_guidance_is_not_success() {
    let journal = tempfile::tempdir().unwrap();
    let note = journal.path().join("note.md");
    fs::write(&note, "a short imported note").unwrap();
    let result = run_at(
        journal.path(),
        &[note.to_str().expect("utf-8 note path")],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 1, "stdout={}", result.stdout);
    assert!(
        result.stderr.contains("Could not detect timestamp")
            && result.stderr.contains("--timestamp"),
        "stderr={}",
        result.stderr
    );
    assert!(result.stdout.is_empty());
    assert!(!journal.path().join("chronicle").exists());
}

fn run<E, C>(args: &[&str], lookup_env: E, connectivity: C) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    run_at(Path::new("."), args, lookup_env, connectivity)
}

fn run_at<E, C>(journal: &Path, args: &[&str], lookup_env: E, connectivity: C) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    match run_cli_with(
        &args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
        journal,
        lookup_env,
        connectivity,
    ) {
        CliOutcome::Rendered(run) | CliOutcome::Imported { run, .. } => run,
        CliOutcome::Registry(_) => panic!("test invocation must not reach a registry body"),
    }
}

#[test]
fn production_audio_import_runtime_waits_on_a_bound_callosum_socket() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("journal");
    fs::create_dir_all(journal.join("health")).unwrap();
    let request = AudioImportRequest {
        source_instant: None,
        source_media: temporary.path().join("source.m4a"),
        journal_root: journal.clone(),
        day: "20260811".to_owned(),
        base_timestamp: NaiveDateTime::parse_from_str("2026-08-11T12:00:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap(),
        import_id: "runtime-io".to_owned(),
        stream: "import.audio".to_owned(),
        facet: None,
        setting: None,
        wait_for_processing: true,
        stall_timeout: Duration::from_millis(50),
        poll_interval: Duration::from_millis(5),
    };
    let runtime = audio_import_runtime().expect("audio import runtime");
    let outcome = runtime
        .block_on(async {
            let server = CallosumSocketServer::bind(journal.join("health/callosum.sock"))
                .await
                .expect("bind Callosum socket");
            let result = import_audio_with_seams(
                request,
                AudioImportSeams {
                    probe: |_: &Path| Ok(probed(1.0)),
                    slice: |_: &Path, output: &Path, _: f64, _: f64| {
                        fs::write(output, b"audio").map_err(|error| {
                            solstone_core_import_host::audio::AudioSliceError::InputUnreadable {
                                detail: error.to_string(),
                            }
                        })
                    },
                    emit_observing: |_: &ObservingSegment| {},
                    wait: native_processing_wait,
                },
            )
            .await;
            server.stop().await;
            result
        })
        .expect("native processing wait");
    assert!(outcome.created().processing.requested);
}

fn panic_wait(
    _: AudioImportRequest,
    _: PathBuf,
    _: AudioImportRecord,
) -> Pin<Box<dyn Future<Output = Result<ProcessingWaitOutcome, ImportError>> + Send>> {
    Box::pin(async { panic!("injected wait panic") })
}

fn error_wait(
    _: AudioImportRequest,
    _: PathBuf,
    _: AudioImportRecord,
) -> Pin<Box<dyn Future<Output = Result<ProcessingWaitOutcome, ImportError>> + Send>> {
    Box::pin(async {
        Err(ImportError::AudioProcessingWait {
            detail: "injected wait error".to_owned(),
        })
    })
}

fn failed_wait(
    _: AudioImportRequest,
    _: PathBuf,
    _: AudioImportRecord,
) -> Pin<Box<dyn Future<Output = Result<ProcessingWaitOutcome, ImportError>> + Send>> {
    Box::pin(async {
        Ok(ProcessingWaitOutcome {
            requested: true,
            failed_segments: vec!["120000_1".to_owned()],
            stalled_segments: Vec::new(),
        })
    })
}

fn stalled_wait(
    _: AudioImportRequest,
    _: PathBuf,
    _: AudioImportRecord,
) -> Pin<Box<dyn Future<Output = Result<ProcessingWaitOutcome, ImportError>> + Send>> {
    Box::pin(async {
        Ok(ProcessingWaitOutcome {
            requested: true,
            failed_segments: Vec::new(),
            stalled_segments: vec!["120000_1".to_owned()],
        })
    })
}

fn run_wait_cli(wait: ProcessingWaitFn) -> CliRun {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("journal");
    fs::create_dir_all(journal.join("health")).unwrap();
    let request = AudioImportRequest {
        source_instant: None,
        source_media: temporary.path().join("source.m4a"),
        journal_root: journal,
        day: "20260811".to_owned(),
        base_timestamp: NaiveDateTime::parse_from_str("2026-08-11T12:00:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap(),
        import_id: "wait-cli".to_owned(),
        stream: "import.audio".to_owned(),
        facet: None,
        setting: None,
        wait_for_processing: true,
        stall_timeout: Duration::from_millis(50),
        poll_interval: Duration::from_millis(5),
    };
    let runtime = audio_import_runtime().expect("audio import runtime");
    audio_import_cli_run(runtime.block_on(import_audio_with_seams(
        request,
        AudioImportSeams {
            probe: |_: &Path| Ok(probed(1.0)),
            slice: |_: &Path, output: &Path, _: f64, _: f64| {
                fs::write(output, b"audio").map_err(|error| {
                    solstone_core_import_host::audio::AudioSliceError::InputUnreadable {
                        detail: error.to_string(),
                    }
                })
            },
            emit_observing: |_: &ObservingSegment| {},
            wait,
        },
    )))
}

fn assert_wait_command_failure(run: &CliRun, cause: &str) {
    assert_ne!(
        run.exit_code, 0,
        "stdout={} stderr={}",
        run.stdout, run.stderr
    );
    assert!(
        !run.stdout.to_ascii_lowercase().contains("complete"),
        "stdout={}",
        run.stdout
    );
    assert!(run.stderr.contains(cause), "stderr={}", run.stderr);
}

#[test]
fn wait_panic_is_a_command_failure() {
    assert_wait_command_failure(&run_wait_cli(panic_wait), "injected wait panic");
}

#[test]
fn wait_error_is_a_command_failure() {
    assert_wait_command_failure(&run_wait_cli(error_wait), "injected wait error");
}

#[test]
fn wait_failed_segments_is_a_command_failure() {
    assert_wait_command_failure(&run_wait_cli(failed_wait), "120000_1");
}

#[test]
fn wait_stalled_segments_is_a_command_failure() {
    assert_wait_command_failure(&run_wait_cli(stalled_wait), "120000_1");
}

fn write_utc_zone(journal: &Path) {
    fs::create_dir_all(journal.join("config")).unwrap();
    fs::write(
        journal.join("config/journal.json"),
        br#"{"identity":{"timezone":"UTC"}}"#,
    )
    .unwrap();
}

#[test]
fn a_timestamp_with_its_offset_imports_the_second_occurrence_of_a_repeated_hour() {
    for (timestamp, instant) in [
        ("2025-11-02T01:30:00-07:00", "2025-11-02T08:30:00Z"),
        ("2025-11-02T01:30:00-06:00", "2025-11-02T07:30:00Z"),
        // The journal's own form is unchanged: a repeated hour takes the first occurrence.
        ("20251102_013000", "2025-11-02T07:30:00Z"),
    ] {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/journal.json"),
            br#"{"identity":{"timezone":"America/Denver"}}"#,
        )
        .unwrap();
        let note = journal.path().join("note.md");
        fs::write(&note, "a short imported note").unwrap();
        let (run, import_id) = admitted(
            journal.path(),
            &["--timestamp", timestamp, note.to_str().unwrap()],
        );
        assert_eq!(run.exit_code, 0, "{timestamp}: {}", run.stderr);
        assert_eq!(import_id, "20251102_013000", "{timestamp}");
        let transcript = fs::read_to_string(
            journal
                .path()
                .join("chronicle/20251102/import.text/013000_300/conversation_transcript.jsonl"),
        )
        .unwrap();
        let entry: Value = serde_json::from_str(transcript.lines().nth(1).unwrap()).unwrap();
        assert_eq!(
            entry["source_ms"],
            chrono::DateTime::parse_from_rfc3339(instant)
                .unwrap()
                .timestamp_millis(),
            "{timestamp}"
        );
    }
}

#[test]
fn a_timestamp_in_neither_form_names_both() {
    let journal = tempfile::tempdir().unwrap();
    let note = journal.path().join("note.md");
    fs::write(&note, "a short imported note").unwrap();
    let result = run_at(
        journal.path(),
        &["--timestamp", "2025-11-02T01:30:00", note.to_str().unwrap()],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 1);
    assert!(
        result.stderr.contains("YYYYMMDD_HHMMSS") && result.stderr.contains("-07:00"),
        "stderr={}",
        result.stderr
    );
    assert!(!journal.path().join("imports").exists());
}

fn admitted(journal: &Path, args: &[&str]) -> (CliRun, String) {
    match run_cli_with(
        &args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
        journal,
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    ) {
        CliOutcome::Imported { run, import_id } => (run, import_id),
        other => panic!("expected an admitted import, got {other:?}"),
    }
}

fn source_timestamp(journal: &Path, import_id: &str) -> String {
    let metadata: Value = serde_json::from_slice(
        &fs::read(journal.join("imports").join(import_id).join("import.json")).unwrap(),
    )
    .unwrap();
    metadata["source_timestamp"].as_str().unwrap().to_owned()
}

fn text_placements(journal: &Path) -> Vec<(String, String)> {
    let day = journal.join("chronicle/20260616/import.text");
    let mut found = Vec::new();
    if !day.exists() {
        return found;
    }
    for entry in fs::read_dir(&day).unwrap().flatten() {
        let transcript = entry.path().join("conversation_transcript.jsonl");
        if !transcript.is_file() {
            continue;
        }
        let header: Value = serde_json::from_str(
            fs::read_to_string(&transcript)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let key = entry.file_name().to_string_lossy().into_owned();
        let id = header["imported"]["id"].as_str().unwrap().to_owned();
        found.push((key, id));
    }
    found.sort();
    found
}

#[test]
fn two_texts_at_the_owner_day_boundary_keep_the_source_clock() {
    let journal = tempfile::tempdir().unwrap();
    write_utc_zone(journal.path());
    let first = journal.path().join("first.md");
    let second = journal.path().join("second.md");
    fs::write(&first, "first note").unwrap();
    fs::write(&second, "second note").unwrap();
    let stamp = "20260616_235959";

    let preview = run_cli_with(
        &[
            "--dry-run".to_owned(),
            "--timestamp".to_owned(),
            stamp.to_owned(),
            first.display().to_string(),
        ],
        journal.path(),
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert!(matches!(preview, CliOutcome::Rendered(_)));
    assert!(!journal.path().join("imports").exists());

    let (first_run, first_id) = admitted(
        journal.path(),
        &["--timestamp", stamp, first.to_str().unwrap()],
    );
    assert_eq!(first_run.exit_code, 0, "{}", first_run.stderr);
    let (second_run, second_id) = admitted(
        journal.path(),
        &["--timestamp", stamp, second.to_str().unwrap()],
    );
    assert_eq!(second_run.exit_code, 0, "{}", second_run.stderr);
    assert_eq!(first_id, stamp);
    assert_eq!(second_id, "20260617_000000");
    assert_eq!(source_timestamp(journal.path(), &first_id), stamp);
    assert_eq!(source_timestamp(journal.path(), &second_id), stamp);
    let placements = text_placements(journal.path());
    assert_eq!(placements.len(), 2, "{placements:?}");
    assert!(
        placements
            .iter()
            .any(|(key, id)| key.starts_with("235959") && id == stamp),
        "{placements:?}"
    );
    assert!(
        placements
            .iter()
            .any(|(key, id)| id == "20260617_000000" && !key.starts_with("000000")),
        "{placements:?}"
    );
    assert!(
        placements.iter().all(|(key, _)| !key.starts_with("000000")),
        "{placements:?}"
    );
    assert!(!journal.path().join("chronicle/20260617").exists());
}

#[test]
fn retry_of_a_shifted_text_import_keeps_the_selected_id_and_source_time() {
    let journal = tempfile::tempdir().unwrap();
    write_utc_zone(journal.path());
    let note = journal.path().join("note.md");
    fs::write(&note, "shifted note").unwrap();
    let occupant = journal.path().join("imports/20260616_235959");
    fs::create_dir_all(occupant.parent().unwrap()).unwrap();
    fs::write(&occupant, b"keep").unwrap();
    let marker = journal
        .path()
        .join("chronicle/20260616/health/stream.updated");
    fs::create_dir_all(&marker).unwrap();

    let (failed, failed_id) = admitted(
        journal.path(),
        &["--timestamp", "20260616_235959", note.to_str().unwrap()],
    );
    assert_ne!(failed.exit_code, 0);
    assert_eq!(failed_id, "20260617_000000");
    assert_eq!(
        source_timestamp(journal.path(), &failed_id),
        "20260616_235959"
    );
    assert_eq!(fs::read(&occupant).unwrap(), b"keep");
    fs::remove_dir(&marker).unwrap();

    let (retried, retried_id) = admitted(
        journal.path(),
        &["--timestamp", "20260616_235959", note.to_str().unwrap()],
    );
    assert_eq!(retried.exit_code, 0, "{}", retried.stderr);
    assert_eq!(retried_id, failed_id);
    assert_eq!(
        source_timestamp(journal.path(), &retried_id),
        "20260616_235959"
    );
    assert_eq!(fs::read(&occupant).unwrap(), b"keep");
    assert!(!journal.path().join("imports/20260617_000001").exists());
    assert!(!journal.path().join("chronicle/20260617").exists());
    let projection = solstone_core_import::project_import_result(journal.path(), &retried_id);
    assert_eq!(
        projection.status,
        solstone_core_import::ProjectionStatus::Success,
        "{projection:?}"
    );
}

fn instant_wait(
    _: AudioImportRequest,
    _: PathBuf,
    _: AudioImportRecord,
) -> Pin<Box<dyn Future<Output = Result<ProcessingWaitOutcome, ImportError>> + Send>> {
    Box::pin(async {
        Ok(ProcessingWaitOutcome {
            requested: false,
            failed_segments: Vec::new(),
            stalled_segments: Vec::new(),
        })
    })
}

fn place_audio(
    journal: &Path,
    source: &Path,
    import_id: &str,
    source_stamp: &str,
) -> Result<(), ImportError> {
    let runtime = audio_import_runtime().expect("audio import runtime");
    let request = AudioImportRequest {
        source_instant: None,
        source_media: source.to_path_buf(),
        journal_root: journal.to_path_buf(),
        day: source_stamp[..8].to_owned(),
        base_timestamp: NaiveDateTime::parse_from_str(source_stamp, "%Y%m%d_%H%M%S").unwrap(),
        import_id: import_id.to_owned(),
        stream: "import.audio".to_owned(),
        facet: None,
        setting: None,
        wait_for_processing: false,
        stall_timeout: Duration::from_millis(1),
        poll_interval: Duration::from_millis(1),
    };
    runtime.block_on(import_audio_with_seams(
        request,
        AudioImportSeams {
            probe: |_: &Path| Ok(probed(1.0)),
            slice: |_: &Path, output: &Path, _: f64, _: f64| {
                fs::write(output, b"audio").map_err(|error| {
                    solstone_core_import_host::audio::AudioSliceError::InputUnreadable {
                        detail: error.to_string(),
                    }
                })
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: instant_wait,
        },
    ))?;
    Ok(())
}

#[test]
fn two_audio_inputs_at_the_owner_day_boundary_keep_the_source_clock() {
    let journal = tempfile::tempdir().unwrap();
    write_utc_zone(journal.path());
    let first = journal.path().join("first.m4a");
    let second = journal.path().join("second.m4a");
    fs::write(&first, b"not-audio-a").unwrap();
    fs::write(&second, b"not-audio-b").unwrap();
    let stamp = "20260616_235959";

    let preview = run_cli_with(
        &[
            "--dry-run".to_owned(),
            "--timestamp".to_owned(),
            stamp.to_owned(),
            first.display().to_string(),
        ],
        journal.path(),
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert!(matches!(preview, CliOutcome::Rendered(_)));
    assert!(!journal.path().join("imports").exists());

    let (first_run, first_id) = admitted(
        journal.path(),
        &["--timestamp", stamp, first.to_str().unwrap()],
    );
    assert_ne!(
        first_run.exit_code, 0,
        "a non-media file fails after the record is claimed"
    );
    let (second_run, second_id) = admitted(
        journal.path(),
        &["--timestamp", stamp, second.to_str().unwrap()],
    );
    assert_ne!(second_run.exit_code, 0);
    assert_eq!(first_id, stamp);
    assert_eq!(second_id, "20260617_000000");
    assert_eq!(source_timestamp(journal.path(), &first_id), stamp);
    assert_eq!(source_timestamp(journal.path(), &second_id), stamp);

    place_audio(journal.path(), &second, &second_id, stamp)
        .expect("shifted id keeps the source day");
    let segment = journal
        .path()
        .join("chronicle/20260616/import.audio/235959_1");
    assert!(
        segment.join("imported_audio.m4a").is_file(),
        "{}",
        segment.display()
    );
    assert_eq!(
        fs::read(segment.join("imported_audio.m4a")).unwrap(),
        b"audio"
    );
    let overflow = place_audio(journal.path(), &first, &first_id, stamp);
    assert!(
        matches!(
            overflow,
            Err(ImportError::AudioSegmentDayOverflow { ref day, .. }) if day == "20260616"
        ),
        "{overflow:?}"
    );
    assert!(!journal.path().join("chronicle/20260617").exists());
}

#[test]
fn wait_success_still_reports_complete() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("journal");
    fs::create_dir_all(journal.join("health")).unwrap();
    let request = AudioImportRequest {
        source_instant: None,
        source_media: temporary.path().join("source.m4a"),
        journal_root: journal,
        day: "20260811".to_owned(),
        base_timestamp: NaiveDateTime::parse_from_str("2026-08-11T12:00:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap(),
        import_id: "wait-ok".to_owned(),
        stream: "import.audio".to_owned(),
        facet: None,
        setting: None,
        wait_for_processing: true,
        stall_timeout: Duration::from_millis(50),
        poll_interval: Duration::from_millis(5),
    };
    let runtime = audio_import_runtime().expect("audio import runtime");
    let run = audio_import_cli_run(runtime.block_on(import_audio_with_seams(
        request,
        AudioImportSeams {
            probe: |_: &Path| Ok(probed(1.0)),
            slice: |_: &Path, output: &Path, _: f64, _: f64| {
                fs::write(output, b"audio").unwrap();
                fs::write(
                    output.with_extension("jsonl"),
                    "{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\",\"handler\":\"transcribe\",\"input_size\":5}}\n",
                )
                .unwrap();
                Ok(())
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    )));
    assert_eq!(run.exit_code, 0, "stderr={}", run.stderr);
    assert!(
        run.stdout.contains("Generic audio import complete"),
        "stdout={}",
        run.stdout
    );
}

#[test]
fn partial_remux_without_failed_or_stalled_wait_still_succeeds() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("journal");
    fs::create_dir_all(journal.join("health")).unwrap();
    let request = AudioImportRequest {
        source_instant: None,
        source_media: temporary.path().join("source.m4a"),
        journal_root: journal,
        day: "20260811".to_owned(),
        base_timestamp: NaiveDateTime::parse_from_str("2026-08-11T12:00:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap(),
        import_id: "wait-partial".to_owned(),
        stream: "import.audio".to_owned(),
        facet: None,
        setting: None,
        wait_for_processing: true,
        stall_timeout: Duration::from_millis(50),
        poll_interval: Duration::from_millis(5),
    };
    let runtime = audio_import_runtime().expect("audio import runtime");
    let run = audio_import_cli_run(runtime.block_on(import_audio_with_seams(
        request,
        AudioImportSeams {
            probe: |_: &Path| Ok(probed(601.0)),
            slice: |_: &Path, output: &Path, start: f64, _: f64| {
                if start == 0.0 {
                    fs::write(output, b"audio").unwrap();
                    fs::write(
                        output.with_extension("jsonl"),
                        "{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\",\"handler\":\"transcribe\",\"input_size\":5}}\n",
                    )
                    .unwrap();
                    return Ok(());
                }
                Err(solstone_core_import_host::audio::AudioSliceError::Remux {
                    error: ffmpeg_next::Error::InvalidData,
                })
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    )));
    assert_eq!(run.exit_code, 0, "stderr={}", run.stderr);
    assert!(
        run.stdout.contains("Generic audio import complete"),
        "stdout={}",
        run.stdout
    );
    assert!(
        run.stderr.contains("5:00 to 10:00") && run.stderr.contains("10:00 to 10:01"),
        "the parts left out are named: stderr={}",
        run.stderr
    );
}

/// The repository's long Opus recording, copied under a name that carries its time.
#[cfg(feature = "full-tests")]
fn synced_opus_source(folder: &Path, name: &str) -> PathBuf {
    let source = folder.join(name);
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/long_audio_remux_corpus/long-remux.opus"),
        &source,
    )
    .unwrap();
    source
}

/// Move the granule positions of the Ogg pages between 420 s and 440 s back by 60 s, so the
/// second five-minute chunk goes backwards in time and cannot be copied.
#[cfg(feature = "full-tests")]
fn step_back_inside_second_chunk(path: &Path) {
    fn crc(bytes: &[u8]) -> u32 {
        bytes.iter().fold(0_u32, |crc, byte| {
            let mut value = crc ^ (u32::from(*byte) << 24);
            for _ in 0..8 {
                value = if value & 0x8000_0000 != 0 {
                    (value << 1) ^ 0x04C1_1DB7
                } else {
                    value << 1
                };
            }
            value
        })
    }
    let mut data = fs::read(path).unwrap();
    let mut position = 0;
    let mut changed = 0;
    while position < data.len() {
        assert_eq!(&data[position..position + 4], b"OggS");
        let segments = usize::from(data[position + 26]);
        let body: usize = data[position + 27..position + 27 + segments]
            .iter()
            .map(|lacing| usize::from(*lacing))
            .sum();
        let length = 27 + segments + body;
        let granule = i64::from_le_bytes(data[position + 6..position + 14].try_into().unwrap());
        if (420 * 48_000..440 * 48_000).contains(&granule) {
            let moved = granule - 60 * 48_000;
            data[position + 6..position + 14].copy_from_slice(&moved.to_le_bytes());
            data[position + 22..position + 26].fill(0);
            let checksum = crc(&data[position..position + length]);
            data[position + 22..position + 26].copy_from_slice(&checksum.to_le_bytes());
            changed += 1;
        }
        position += length;
    }
    assert!(changed > 0, "the fixture has pages in the second chunk");
    fs::write(path, data).unwrap();
}

#[cfg(feature = "full-tests")]
fn sync_audio_folder(journal: &Path, folder: &Path) -> CliRun {
    run_at(
        journal,
        &[
            "--sync",
            "audio",
            "--path",
            folder.to_str().unwrap(),
            "--save",
        ],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    )
}

#[cfg(feature = "full-tests")]
fn opening_packet(path: &Path) -> (i64, Vec<u8>) {
    ffmpeg_next::init().unwrap();
    let mut input = ffmpeg_next::format::input(path).unwrap();
    let audio = input
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .unwrap()
        .index();
    input
        .packets()
        .find(|(stream, _)| stream.index() == audio)
        .map(|(_, packet)| {
            (
                packet.pts().unwrap(),
                packet.data().unwrap_or_default().to_vec(),
            )
        })
        .unwrap()
}

#[cfg(feature = "full-tests")]
#[test]
fn an_import_with_a_part_left_out_is_not_marked_imported() {
    let journal = tempfile::tempdir().unwrap();
    write_utc_zone(journal.path());
    let folder = tempfile::tempdir().unwrap();
    let source = synced_opus_source(folder.path(), "20250717_120000.opus");
    step_back_inside_second_chunk(&source);

    let run = sync_audio_folder(journal.path(), folder.path());
    assert_eq!(
        run.exit_code, 0,
        "stdout={} stderr={}",
        run.stdout, run.stderr
    );
    let day = journal.path().join("chronicle/20250717/import.audio");
    assert!(day.join("120000_300/imported_audio.opus").is_file());
    assert!(
        !day.join("120500_300").exists(),
        "the second chunk was left out"
    );

    let hash = solstone_core_import::hash_source(&source).unwrap();
    let scan = solstone_core_import::find_manifest_by_hash(journal.path(), &hash).unwrap();
    assert!(
        scan.found.is_none(),
        "a partial import must stay importable: {:?}",
        scan.found.map(|found| found.path)
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn an_opus_import_keeps_its_opening_packet() {
    let journal = tempfile::tempdir().unwrap();
    write_utc_zone(journal.path());
    let folder = tempfile::tempdir().unwrap();
    let source = synced_opus_source(folder.path(), "20250717_130000.opus");
    let (source_pts, source_bytes) = opening_packet(&source);
    assert!(source_pts < 0, "the fixture opens before zero (pre-skip)");

    let run = sync_audio_folder(journal.path(), folder.path());
    assert_eq!(
        run.exit_code, 0,
        "stdout={} stderr={}",
        run.stdout, run.stderr
    );
    let chunk = journal
        .path()
        .join("chronicle/20250717/import.audio/130000_300/imported_audio.opus");
    assert_eq!(opening_packet(&chunk), (source_pts, source_bytes));
}

#[cfg(feature = "full-tests")]
mod audio_archive {
    use super::*;

    fn write_test_ogg_page(
        writer: &mut impl std::io::Write,
        header_type: u8,
        granule_pos: i64,
        serial: u32,
        page_seq: u32,
        payload: &[u8],
    ) {
        let mut segment_table = Vec::new();
        let mut rem = payload.len();
        while rem >= 255 {
            segment_table.push(255u8);
            rem -= 255;
        }
        segment_table.push(rem as u8);

        let mut header = vec![0u8; 27 + segment_table.len()];
        header[0..4].copy_from_slice(b"OggS");
        header[4] = 0;
        header[5] = header_type;
        header[6..14].copy_from_slice(&granule_pos.to_le_bytes());
        header[14..18].copy_from_slice(&serial.to_le_bytes());
        header[18..22].copy_from_slice(&page_seq.to_le_bytes());
        header[22..26].copy_from_slice(&0u32.to_le_bytes());
        header[26] = segment_table.len() as u8;
        header[27..27 + segment_table.len()].copy_from_slice(&segment_table);

        let mut full = header;
        full.extend_from_slice(payload);
        let checksum = full.iter().fold(0_u32, |crc, byte| {
            let mut value = crc ^ (u32::from(*byte) << 24);
            for _ in 0..8 {
                value = if value & 0x8000_0000 != 0 {
                    (value << 1) ^ 0x04C1_1DB7
                } else {
                    value << 1
                };
            }
            value
        });
        full[22..26].copy_from_slice(&checksum.to_le_bytes());
        writer.write_all(&full).unwrap();
    }

    fn write_test_opus_file(path: &Path, duration_ms: i64) {
        let mut file = fs::File::create(path).unwrap();
        let serial = 0x12345678;
        // Page 0: OpusHead (BOS)
        let head = b"OpusHead\x01\x01\x00\x00\x80\xbb\x00\x00\x00\x00\x00";
        write_test_ogg_page(&mut file, 0x02, 0, serial, 0, head);
        // Page 1: OpusTags
        let tags = b"OpusTags\x08\x00\x00\x00solstone\x00\x00\x00\x00";
        write_test_ogg_page(&mut file, 0x00, 0, serial, 1, tags);
        // Each packet has its own lacing terminator and advancing granule.
        let packet_count = (duration_ms / 20).max(1);
        for index in 0..packet_count {
            write_test_ogg_page(
                &mut file,
                if index + 1 == packet_count { 4 } else { 0 },
                (index + 1) * 960,
                serial,
                (index + 2) as u32,
                &[0xf8, 0xff, 0xfe],
            );
        }
    }

    #[test]
    fn audio_archive_folder_zone_required() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let run = run_at(
            journal.path(),
            &["--audio-archive", folder.path().to_str().unwrap()],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run.exit_code, 1);
        assert!(run.stderr.contains("--source-zone"));
    }

    #[test]
    fn audio_archive_manifest_offset_instant_does_not_need_the_flag() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
        )
        .unwrap();
        let run = run_at(
            journal.path(),
            &[
                "--audio-archive",
                manifest_file.to_str().unwrap(),
                "--dry-run",
            ],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run.exit_code, 0, "stderr={}", run.stderr);
        assert!(run.stdout.contains("audio archive preview:"));
    }

    #[test]
    fn audio_archive_agreeing_evidence_included() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        let name = "limitless_pendant_2026-08-11T12-00-00_to_2026-08-11T12-05-00.opus";
        write_test_opus_file(&folder.path().join(name), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
        &manifest_file,
        format!(
            r#"{{"members":[{{"path":"{name}","instant":"2026-08-11T12:00:00Z","source_zone":"UTC"}}]}}"#
        ),
    )
    .unwrap();
        let run = run_at(
            journal.path(),
            &[
                "--audio-archive",
                manifest_file.to_str().unwrap(),
                "--dry-run",
                "--json",
            ],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run.exit_code, 0, "stderr={}", run.stderr);
        let parsed: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(parsed["members"], 1);
    }

    #[test]
    fn audio_archive_selected_manifest_fields_can_conflict() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        let name = "limitless_pendant_2026-08-11T12-00-00_to_2026-08-11T12-05-00.opus";
        write_test_opus_file(&folder.path().join(name), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
        &manifest_file,
        format!(
            r#"{{"members":[{{"path":"{name}","instant":"2026-08-11T18:00:00Z","local_civil":"2026-08-11T12:00:00","source_zone":"UTC"}}]}}"#
        ),
    )
    .unwrap();
        let run = run_at(
            journal.path(),
            &[
                "--audio-archive",
                manifest_file.to_str().unwrap(),
                "--dry-run",
                "--json",
            ],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run.exit_code, 0, "stderr={}", run.stderr);
        let parsed: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(parsed["tiles"], 0);
        assert_eq!(parsed["left_out"]["conflicting_evidence"], 1);
    }

    #[test]
    fn audio_archive_dry_run_creates_no_run_directory() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
        )
        .unwrap();
        let run = run_at(
            journal.path(),
            &[
                "--audio-archive",
                manifest_file.to_str().unwrap(),
                "--dry-run",
            ],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run.exit_code, 0);
        assert!(!journal.path().join("imports").exists());
    }

    #[test]
    fn audio_archive_uses_the_admitted_plan_for_its_preview() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let args = vec![
            "--audio-archive".to_owned(),
            "/path/to/archive".to_owned(),
            "--source-zone".to_owned(),
            "UTC".to_owned(),
        ];
        assert!(
            solstone_core_import_host::cli_argv::preview_arguments(&args, journal.path())
                .unwrap()
                .is_none()
        );

        let mut dry_args = vec!["--dry-run".to_owned()];
        dry_args.extend(args.clone());
        assert!(
            solstone_core_import_host::cli_argv::preview_arguments(&dry_args, journal.path())
                .unwrap()
                .is_none()
        );

        let status_args = vec![
            "--audio-archive-status".to_owned(),
            "20260811_120000".to_owned(),
        ];
        assert!(
            solstone_core_import_host::cli_argv::preview_arguments(&status_args, journal.path())
                .unwrap()
                .is_none()
        );

        let members_args = vec![
            "--audio-archive-members".to_owned(),
            "20260811_120000".to_owned(),
        ];
        assert!(
            solstone_core_import_host::cli_argv::preview_arguments(&members_args, journal.path())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn audio_archive_save_does_not_print_the_size_again() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
        )
        .unwrap();
        let run = run_at(
            journal.path(),
            &["--audio-archive", manifest_file.to_str().unwrap()],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run.exit_code, 0, "stderr={}", run.stderr);
        assert!(!run.stdout.contains("audio archive preview:"));
        assert!(run.stdout.contains("audio archive new:"));
    }

    #[test]
    fn audio_archive_status_and_members_do_not_call_frontier_writers() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
        )
        .unwrap();
        let save_run = run_at(
            journal.path(),
            &["--audio-archive", manifest_file.to_str().unwrap(), "--json"],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(save_run.exit_code, 0);
        let parsed: serde_json::Value = serde_json::from_str(&save_run.stdout).unwrap();
        let run_id = parsed["run_id"].as_str().unwrap();

        let status_run = run_at(
            journal.path(),
            &["--audio-archive-status", run_id],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(status_run.exit_code, 0);
        assert!(status_run.stdout.contains("audio archive status:"));

        let members_run = run_at(
            journal.path(),
            &[
                "--audio-archive-members",
                run_id,
                "--offset",
                "0",
                "--limit",
                "10",
            ],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(members_run.exit_code, 0);
        assert!(members_run.stdout.contains("audio archive members:"));
        assert!(members_run.stdout.contains("disposition=included"));
    }

    #[test]
    fn audio_archive_explicit_resume_does_not_reopen_an_already_saved_source() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
        )
        .unwrap();
        let save_run = run_at(
            journal.path(),
            &["--audio-archive", manifest_file.to_str().unwrap(), "--json"],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(save_run.exit_code, 0);
        let parsed: serde_json::Value = serde_json::from_str(&save_run.stdout).unwrap();
        let run_id = parsed["run_id"].as_str().unwrap();

        // Now change file (2000ms duration instead of 1000ms, changing hash and length)
        write_test_opus_file(&folder.path().join("rec.opus"), 2000);
        let resume_run = run_at(
            journal.path(),
            &[
                "--audio-archive",
                manifest_file.to_str().unwrap(),
                "--resume",
                run_id,
            ],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(resume_run.exit_code, 0, "{}", resume_run.stderr);

        let entries: Vec<_> = fs::read_dir(journal.path().join("imports"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "no new run allocated on explicit resume mismatch"
        );
    }

    #[test]
    fn audio_archive_automatic_rerun_fills_only_unsaved_parts() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
        )
        .unwrap();

        let options = solstone_core_import_host::audio_archive::AudioArchiveDispatchOptions {
            path: manifest_file.clone(),
            source_zone: None,
            resume: None,
            expected_identity: None,
            expected_owner_zone: None,
            dry_run: false,
            json: true,
        };
        let run1 = solstone_core_import_host::audio_archive::run_audio_archive_with(
            options.clone(),
            journal.path(),
            None,
            Some(solstone_core_import::audio_archive::ArchiveFault::AfterRecordFirstSaved),
        );
        assert_eq!(run1.exit_code, 1);

        let run2 = solstone_core_import_host::audio_archive::run_audio_archive_with(
            options,
            journal.path(),
            None,
            None,
        );
        assert_eq!(run2.exit_code, 0, "stderr={}", run2.stderr);
        let parsed: serde_json::Value = serde_json::from_str(&run2.stdout).unwrap();
        assert_eq!(parsed["operation"], "resume");
        assert_eq!(parsed["tiles_saved"], 1);
        assert_eq!(parsed["parts_saved"], 1);
    }

    #[test]
    fn audio_archive_time_correction_keeps_identity_gets_settings_mismatch_then_new_run_saves() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
        )
        .unwrap();

        let options = solstone_core_import_host::audio_archive::AudioArchiveDispatchOptions {
            path: manifest_file.clone(),
            source_zone: None,
            resume: None,
            expected_identity: None,
            expected_owner_zone: None,
            dry_run: false,
            json: true,
        };
        let run1 = solstone_core_import_host::audio_archive::run_audio_archive_with(
            options.clone(),
            journal.path(),
            None,
            Some(solstone_core_import::audio_archive::ArchiveFault::AfterRecordFirstSaved),
        );
        assert_eq!(run1.exit_code, 1);

        // Correct the time: instant changes from 12:00:00 to 14:00:00.
        // Name, byte_len, sha256 are unchanged so identity is identical.
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T14:00:00Z"}]}"#,
        )
        .unwrap();

        let run2 = solstone_core_import_host::audio_archive::run_audio_archive_with(
            options,
            journal.path(),
            None,
            None,
        );
        assert_eq!(run2.exit_code, 0, "stderr={}", run2.stderr);
        let parsed: serde_json::Value = serde_json::from_str(&run2.stdout).unwrap();
        assert_eq!(parsed["operation"], "new");
        assert_eq!(parsed["tiles_saved"], 1);

        let entries: Vec<_> = fs::read_dir(journal.path().join("imports"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn audio_archive_save_complete_rerun_does_not_allocate() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
        )
        .unwrap();
        let run1 = run_at(
            journal.path(),
            &["--audio-archive", manifest_file.to_str().unwrap(), "--json"],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run1.exit_code, 0);

        let run2 = run_at(
            journal.path(),
            &["--audio-archive", manifest_file.to_str().unwrap(), "--json"],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run2.exit_code, 0);
        let parsed: serde_json::Value = serde_json::from_str(&run2.stdout).unwrap();
        assert_eq!(parsed["operation"], "unchanged");
        assert_eq!(parsed["tiles_saved"], 0);
        assert_eq!(parsed["parts_saved"], 0);

        let entries: Vec<_> = fs::read_dir(journal.path().join("imports"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn audio_archive_after_tile_bytes_staged_is_not_tiles_saved() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        write_test_opus_file(&folder.path().join("rec.opus"), 1000);
        let manifest_file = folder.path().join("manifest.json");
        fs::write(
            &manifest_file,
            r#"{"members":[{"path":"rec.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
        )
        .unwrap();

        let options = solstone_core_import_host::audio_archive::AudioArchiveDispatchOptions {
            path: manifest_file,
            source_zone: None,
            resume: None,
            expected_identity: None,
            expected_owner_zone: None,
            dry_run: false,
            json: true,
        };
        let run = solstone_core_import_host::audio_archive::run_audio_archive_with(
            options,
            journal.path(),
            None,
            Some(solstone_core_import::audio_archive::ArchiveFault::AfterTileBytesStaged),
        );
        assert_ne!(run.exit_code, 0);

        fn has_placement_json(dir: &Path) -> bool {
            if !dir.exists() {
                return false;
            }
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && entry.file_name() == "placement.json" {
                        return true;
                    }
                    if path.is_dir() && has_placement_json(&path) {
                        return true;
                    }
                }
            }
            false
        }
        assert!(
            !has_placement_json(&journal.path().join("chronicle")),
            "chronicle must have no placement.json after aborted tile staging"
        );
    }

    #[test]
    fn audio_archive_journal_importer_file_timestamp_still_parses_as_one_file_import() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let note = journal.path().join("note.md");
        fs::write(&note, "hello single file").unwrap();
        let run = run_at(
            journal.path(),
            &[note.to_str().unwrap(), "--timestamp", "20260811_120000"],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run.exit_code, 0, "stderr={}", run.stderr);
        assert!(run.stdout.contains("Generic text import complete"));
    }

    #[test]
    fn audio_archive_invalid_argv_combinations() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());

        // --offset without --audio-archive-members
        let run_offset = run_at(
            journal.path(),
            &["--offset", "10"],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run_offset.exit_code, 2);
        assert!(
            run_offset
                .stderr
                .contains("usage: solstone journal importer")
        );

        // --limit without --audio-archive-members
        let run_limit = run_at(
            journal.path(),
            &["--limit", "50"],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run_limit.exit_code, 2);
        assert!(
            run_limit
                .stderr
                .contains("usage: solstone journal importer")
        );

        // positional media together with --audio-archive
        let run_conflict = run_at(
            journal.path(),
            &["media.opus", "--audio-archive", "archive_dir"],
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert_eq!(run_conflict.exit_code, 2);
        assert!(
            run_conflict
                .stderr
                .contains("usage: solstone journal importer")
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod opus_full_tests {
        use super::*;

        #[test]
        fn full_test_subsecond_opus_member() {
            let journal = tempfile::tempdir().unwrap();
            write_utc_zone(journal.path());
            let folder = tempfile::tempdir().unwrap();
            let audio_path = folder.path().join("subsecond.opus");
            write_test_opus_file(&audio_path, 400);
            let manifest_path = folder.path().join("manifest.json");
            fs::write(
                &manifest_path,
                r#"{"members":[{"path":"subsecond.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
            )
            .unwrap();
            let run = run_at(
                journal.path(),
                &["--audio-archive", manifest_path.to_str().unwrap(), "--json"],
                |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
                || false,
            );
            assert_eq!(run.exit_code, 0, "stderr={}", run.stderr);
            let parsed: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
            assert_eq!(parsed["parts_saved"], 1);
        }

        #[test]
        fn full_test_short_opus_members_at_different_offsets() {
            let journal = tempfile::tempdir().unwrap();
            write_utc_zone(journal.path());
            let folder = tempfile::tempdir().unwrap();
            let a1 = folder.path().join("a1.opus");
            let a2 = folder.path().join("a2.opus");
            let a3 = folder.path().join("a3.opus");
            write_test_opus_file(&a1, 2000);
            write_test_opus_file(&a2, 4000);
            write_test_opus_file(&a3, 3000);
            let manifest_path = folder.path().join("manifest.json");
            fs::write(
                &manifest_path,
                r#"{"members":[
                {"path":"a1.opus","instant":"2026-08-11T12:00:00Z"},
                {"path":"a2.opus","instant":"2026-08-11T12:01:00Z"},
                {"path":"a3.opus","instant":"2026-08-11T12:03:00Z"}
            ]}"#,
            )
            .unwrap();
            let run = run_at(
                journal.path(),
                &["--audio-archive", manifest_path.to_str().unwrap(), "--json"],
                |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
                || false,
            );
            assert_eq!(run.exit_code, 0);
            let parsed: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
            assert_eq!(parsed["tiles_saved"], 1);
            assert_eq!(parsed["parts_saved"], 3);
        }

        #[test]
        fn full_test_opus_members_with_gap() {
            let journal = tempfile::tempdir().unwrap();
            write_utc_zone(journal.path());
            let folder = tempfile::tempdir().unwrap();
            let g1 = folder.path().join("g1.opus");
            let g2 = folder.path().join("g2.opus");
            write_test_opus_file(&g1, 5000);
            write_test_opus_file(&g2, 5000);
            let manifest_path = folder.path().join("manifest.json");
            fs::write(
                &manifest_path,
                r#"{"members":[
                {"path":"g1.opus","instant":"2026-08-11T12:00:00Z"},
                {"path":"g2.opus","instant":"2026-08-11T12:02:00Z"}
            ]}"#,
            )
            .unwrap();
            let run = run_at(
                journal.path(),
                &["--audio-archive", manifest_path.to_str().unwrap(), "--json"],
                |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
                || false,
            );
            assert_eq!(run.exit_code, 0);
            let parsed: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
            assert_eq!(parsed["tiles_saved"], 1);
            assert_eq!(parsed["parts_saved"], 2);
        }

        #[test]
        fn full_test_long_opus_member_across_midnight() {
            let journal = tempfile::tempdir().unwrap();
            write_utc_zone(journal.path());
            let folder = tempfile::tempdir().unwrap();
            let long_file = folder.path().join("long.opus");
            write_test_opus_file(&long_file, 360_000);
            let manifest_path = folder.path().join("manifest.json");
            fs::write(
                &manifest_path,
                r#"{"members":[{"path":"long.opus","instant":"2026-08-11T23:57:00Z"}]}"#,
            )
            .unwrap();
            let run = run_at(
                journal.path(),
                &["--audio-archive", manifest_path.to_str().unwrap(), "--json"],
                |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
                || false,
            );
            assert_eq!(run.exit_code, 0);
            let parsed: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
            assert!(parsed["tiles_saved"].as_u64().unwrap() >= 2);
        }

        #[test]
        fn full_test_opus_member_across_dst_fold() {
            let journal = tempfile::tempdir().unwrap();
            fs::create_dir_all(journal.path().join("config")).unwrap();
            fs::write(
                journal.path().join("config/journal.json"),
                br#"{"identity":{"timezone":"America/New_York"}}"#,
            )
            .unwrap();
            let folder = tempfile::tempdir().unwrap();
            let dst_file = folder.path().join("dst.opus");
            write_test_opus_file(&dst_file, 5000);
            let manifest_path = folder.path().join("manifest.json");
            fs::write(
                &manifest_path,
                r#"{"members":[{"path":"dst.opus","instant":"2026-11-01T05:30:00Z"}]}"#,
            )
            .unwrap();
            let run = run_at(
                journal.path(),
                &["--audio-archive", manifest_path.to_str().unwrap(), "--json"],
                |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
                || false,
            );
            assert_eq!(run.exit_code, 0);
        }

        #[test]
        fn full_test_fractional_non_packet_boundary() {
            let journal = tempfile::tempdir().unwrap();
            write_utc_zone(journal.path());
            let folder = tempfile::tempdir().unwrap();
            let frac_file = folder.path().join("frac.opus");
            write_test_opus_file(&frac_file, 10_015);
            let manifest_path = folder.path().join("manifest.json");
            fs::write(
                &manifest_path,
                r#"{"members":[{"path":"frac.opus","instant":"2026-08-11T12:00:00Z"}]}"#,
            )
            .unwrap();
            let run = run_at(
                journal.path(),
                &["--audio-archive", manifest_path.to_str().unwrap(), "--json"],
                |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
                || false,
            );
            assert_eq!(run.exit_code, 0);
        }
    }
}
