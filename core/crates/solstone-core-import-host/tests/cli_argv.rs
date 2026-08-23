// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use chrono::NaiveDateTime;
use solstone_core_callosum::CallosumSocketServer;
use solstone_core_import::cli_render::CliRun;
use solstone_core_import::{ImportError, ObservingSegment};
use solstone_core_import_host::audio::{
    AudioImportRecord, AudioImportRequest, AudioImportSeams, ProcessingWaitFn,
    ProcessingWaitOutcome, import_audio_with_seams, native_processing_wait,
};
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
    assert!(unknown.stderr.contains("usage: journal importer"));

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

    assert!(!result.stderr.contains("usage: journal importer"));
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

        assert!(!result.stderr.contains("usage: journal importer"));
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
    assert!(result.stderr.contains("usage: journal importer"));
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
        journal
            .path()
            .join("chronicle/20260818/import.text/062652_5/conversation_transcript.jsonl")
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
        CliOutcome::Rendered(run) => run,
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
                    duration_probe: |_: &Path| Ok(1.0),
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
            duration_probe: |_: &Path| Ok(1.0),
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
            duration_probe: |_: &Path| Ok(1.0),
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
            duration_probe: |_: &Path| Ok(601.0),
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
