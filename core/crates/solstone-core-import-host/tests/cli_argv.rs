// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use chrono::NaiveDateTime;
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
    let result = run(
        &["file", "20260311_120000"],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );

    assert!(!result.stderr.contains("usage: solstone journal importer"));
}

#[test]
fn value_options_accept_attached_and_separated_values() {
    for arguments in [
        &["--timestamp=20260311_120000", "file"][..],
        &["--timestamp", "20260311_120000", "file"][..],
    ] {
        let result = run(
            arguments,
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );

        assert!(!result.stderr.contains("usage: solstone journal importer"));
        assert!(!result.stderr.contains("media"));
    }
}

#[test]
fn auto_does_not_swallow_a_path_positional() {
    let result = run(
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
    assert_eq!(
        first[1..],
        [
            serde_json::json!({"start": "00:00:00", "text": "# Planning sync", "source": "import"}),
            serde_json::json!({"start": "00:00:00", "speaker": "Ana Lima", "text": "Let's begin.", "source": "import"}),
        ]
    );
    let second = rows("100500_300");
    assert_eq!(
        second[1..],
        [
            serde_json::json!({"start": "00:02:30", "speaker": "Ben Okafor", "text": "The file keeps  my words.", "source": "import"})
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
        result.stderr.contains("detected timestamp") && result.stderr.contains("or --auto"),
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
}
