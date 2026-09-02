// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native transcription orchestration.

// Config extraction is implemented before the stage machine that consumes it.
#[allow(dead_code)]
mod args;
mod audio;
#[allow(dead_code)]
mod backend;
#[allow(dead_code)]
mod config;
#[allow(dead_code)]
mod event;
mod model_assets;
mod speakers_installation;
mod vad_runtime;
// The standalone CLI is introduced in a later step; retain the completed
// stage pieces without treating that staged integration as a lint failure.
#[allow(dead_code)]
mod processing;
#[allow(dead_code)]
mod speakers;
#[allow(dead_code)]
mod stage;
#[allow(dead_code)]
mod terminal;
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub mod test_hooks;
#[allow(dead_code)]
mod transcript;

pub use args::{
    CliError, ParsedArgs, check_speakers_analyze_installation,
    check_speakers_analyze_installation_with, check_vad_runtime_with, parse_arguments,
    require_solstone, speakers_analyze_repair_text, vad_runtime_repair_for,
};
pub use model_assets::{
    ModelAssetError, PYANNOTE_SEGMENTATION_SHA256, SILERO_VAD_V6_SHA256, WESPEAKER_RESNET34_SHA256,
    resolve_model_asset,
};
pub use speakers::SpeakerAnalyzeError;
pub use speakers_installation::{
    SpeakersAnalyzeGeneration, SpeakersAnalyzeOwnerRole, SpeakersAnalyzeOwnerView,
    enter_speakers_analyze_generation, read_speakers_analyze_owner,
};
pub use vad_runtime::{
    VAD_RUNTIME_PROBE_TIMEOUT, VadRuntimeStatus, probe_from_executable, status_detail,
};

use std::path::{Path, PathBuf};

use solstone_core_speaker_id::writer::SpeakerTranscriptWriteError;
use solstone_core_spp_ratls::AttestationStateStore;

/// The standalone binary's completed result, including Python-compatible batch summary text.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CliRun {
    pub exit_code: i32,
    pub summary: Option<String>,
    pub stderr: Option<String>,
}

/// A CLI-renderable failure without moving pipeline behavior into the binary crate.
#[derive(Debug)]
pub enum CliRunError {
    Cli(CliError),
    Transcribe(TranscribeError),
    Runtime(String),
}

impl CliRunError {
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Cli(error) => error.exit_code(),
            Self::Transcribe(error) => error.exit_code(),
            Self::Runtime(_) => 1,
        }
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Cli(error) => error.message(),
            Self::Transcribe(_) => None,
            Self::Runtime(message) => Some(message),
        }
    }
}

impl std::fmt::Display for CliRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cli(error) => error
                .message()
                .map_or(Ok(()), |message| formatter.write_str(message)),
            Self::Transcribe(error) => error.fmt(formatter),
            Self::Runtime(message) => formatter.write_str(message),
        }
    }
}

/// Run the standalone transcription contract against an explicitly resolved journal root.
pub fn run_cli(
    arguments: impl IntoIterator<Item = String>,
    journal_path: &Path,
    on_day: &mut dyn FnMut(&Path),
) -> Result<CliRun, CliRunError> {
    let parsed = args::parse_arguments(arguments).map_err(CliRunError::Cli)?;
    args::require_solstone(journal_path).map_err(CliRunError::Cli)?;
    args::validate_selection(&parsed).map_err(CliRunError::Cli)?;
    let config = config::read_transcribe_config(journal_path)
        .map_err(|error| CliRunError::Runtime(format!("failed to read journal config: {error}")))?;
    let backend = backend::resolve_default_backend(
        parsed.backend.as_deref(),
        backend::local_stt_backend(),
        backend::read_available_bytes(),
        backend::platform_floor_bytes(),
        backend::confidential::confidential_channel_plausible(&config),
        config::confidential_audio_enabled(&config),
    )
    .map_err(CliRunError::Transcribe)?;
    let _generation =
        enter_speakers_analyze_generation(journal_path, SpeakersAnalyzeOwnerRole::Transcribe)
            .map_err(CliRunError::Cli)?;
    let attestation_state = AttestationStateStore::new();
    if parsed.all {
        return run_all(
            journal_path,
            parsed.redo,
            &backend.backend,
            &config,
            &attestation_state,
            on_day,
        );
    }
    let audio_path = args::resolve_single_audio_path(
        parsed
            .audio_path
            .as_deref()
            .expect("selection was validated"),
        journal_path,
    )
    .map_err(CliRunError::Cli)?;
    stage::process_one(
        &audio_path,
        journal_path,
        parsed.redo,
        Some(&backend.backend),
        &config,
        &attestation_state,
    )
    .map_err(CliRunError::Transcribe)?;
    Ok(CliRun::default())
}

fn run_all(
    journal_path: &Path,
    redo: bool,
    backend: &str,
    config: &solstone_core_journal_config::JournalConfigRead,
    attestation_state: &AttestationStateStore,
    on_day: &mut dyn FnMut(&Path),
) -> Result<CliRun, CliRunError> {
    run_all_with(
        discover_audio_files(journal_path, on_day),
        redo,
        |audio_path| {
            stage::process_one(
                audio_path,
                journal_path,
                redo,
                Some(backend),
                config,
                attestation_state,
            )
        },
    )
}

fn run_all_with<I, F>(audio_paths: I, redo: bool, mut process: F) -> Result<CliRun, CliRunError>
where
    I: IntoIterator<Item = PathBuf>,
    F: FnMut(&Path) -> Result<stage::ProcessOutcome, TranscribeError>,
{
    let (mut processed, mut skipped, mut failed, mut deferred) = (0_u64, 0_u64, 0_u64, 0_u64);
    let mut failed_lines = Vec::new();
    for audio_path in audio_paths {
        if args::should_skip_batch_processed(&audio_path, redo) {
            skipped += 1;
            continue;
        }
        match process(&audio_path) {
            Ok(stage::ProcessOutcome::Failed) => {
                failed += 1;
                failed_lines.push(format!("{}: transcription failed", audio_path.display()));
            }
            Ok(_) => processed += 1,
            Err(TranscribeError::SpeakerAnalysis(error)) => {
                failed += 1;
                failed_lines.push(format!("{}: {error}", audio_path.display()));
            }
            Err(error) if error.exit_code() == 69 => deferred += 1,
            Err(error) => return Err(CliRunError::Transcribe(error)),
        }
    }
    let mut summary = format!("{processed} processed, {skipped} skipped (already transcribed)");
    if deferred > 0 {
        summary.push_str(&format!(
            ", {deferred} deferred (provider not ready, will retry)"
        ));
    }
    if failed > 0 {
        summary.push_str(&format!(", {failed} failed"));
    }
    let stderr = (failed > 0).then(|| {
        let mut block = format!("transcription: {failed} failed\n");
        for line in &failed_lines {
            block.push_str(line);
            block.push('\n');
        }
        block
    });
    Ok(CliRun {
        exit_code: if failed > 0 { 1 } else { 0 },
        summary: Some(summary),
        stderr,
    })
}

/// Yield every transcribable audio file, one chronicle day at a time.
///
/// Collecting the whole journal before returning meant `--all` walked every day,
/// stream and segment on disk before touching a single file. On an owner journal
/// with hundreds of days that is minutes of directory walking during which the CLI
/// looks idle and no audio is transcribed -- measured at 673 s with zero files
/// processed on a 570-day journal. Streaming per day starts work immediately.
///
/// Days are walked newest-first, and `on_day` fires as each day's walk begins, so a
/// caller can report progress without waiting for the whole tree.
fn discover_audio_files<'a>(
    journal_path: &Path,
    on_day: &'a mut dyn FnMut(&Path),
) -> impl Iterator<Item = PathBuf> + 'a {
    let mut days = read_child_directories(&journal_path.join("chronicle"));
    days.sort_by(|left, right| right.cmp(left));
    days.into_iter().flat_map(move |day| {
        on_day(&day);
        let mut day_files = Vec::new();
        for stream in read_child_directories(&day) {
            for segment in read_child_directories(&stream) {
                let Ok(entries) = std::fs::read_dir(&segment) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let extension = path
                        .extension()
                        .and_then(|value| value.to_str())
                        .map(str::to_ascii_lowercase);
                    if path.is_file()
                        && matches!(
                            extension.as_deref(),
                            Some("flac" | "m4a" | "mp3" | "ogg" | "opus" | "wav")
                        )
                    {
                        day_files.push(path);
                    }
                }
            }
        }
        day_files.sort();
        day_files
    })
}

fn read_child_directories(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// Errors currently emitted by the native transcription contract.
#[derive(Debug)]
pub enum TranscribeError {
    /// A bundled model asset could not be resolved.
    ModelAsset(ModelAssetError),
    /// The transcript writer refused or could not publish its requested output.
    SpeakerTranscriptWrite(SpeakerTranscriptWriteError),
    /// A stale NPZ sidecar could not be removed before publication.
    OrphanNpzRemove { path: PathBuf, detail: String },
    /// The temporary zero-row embedding payload could not be prepared or removed.
    TerminalPayload {
        path: Option<PathBuf>,
        detail: String,
    },
    /// A non-writer terminal publication failure occurred.
    TerminalWrite { detail: String },
    /// Input metadata could not be captured before processing began.
    InputMetadata { path: PathBuf, detail: String },
    /// A terminal writer request could not be serialized.
    TerminalRequest { detail: String },
    /// A full transcript writer request could not be serialized.
    TranscriptRequest { detail: String },
    /// The temporary embedding payload for a full transcript could not be handled.
    TranscriptPayload {
        path: Option<PathBuf>,
        detail: String,
    },
    /// The VAD helper binary is unavailable from this installation.
    VadBinary { detail: String },
    /// Preparing or invoking the VAD helper failed temporarily.
    VadTemporary { detail: String },
    /// The VAD helper returned a typed non-success outcome.
    VadHelper {
        helper_exit_code: i32,
        reason: String,
        detail: String,
    },
    /// The VAD helper violated its JSON wire contract.
    VadResponse {
        helper_exit_code: Option<i32>,
        stderr: String,
        detail: String,
    },
    /// No configured, confidential, or resource-admissible STT backend exists.
    SttSurface {
        available_bytes: Option<u64>,
        floor_bytes: Option<u64>,
    },
    /// The supervised parakeet.cpp service is unavailable for retryable work.
    ParakeetCppDeferred { reason: String, detail: String },
    /// The parakeet.cpp service or client contract failed permanently.
    ParakeetCppFailure { reason: String, detail: String },
    /// The CoreML Parakeet helper is unavailable for retryable work.
    ParakeetCoremlDeferred { reason: String, detail: String },
    /// The CoreML Parakeet helper or its contract failed permanently.
    ParakeetCoremlFailure { reason: String, detail: String },
    /// Confidential transcription was refused or unavailable for retryable work.
    ConfidentialDeferred { reason: String, detail: String },
    /// The native speaker-analysis helper could not produce a usable result.
    SpeakerAnalysis(SpeakerAnalyzeError),
    /// Audio decoding failed before a terminal failure record was written.
    Decode { detail: String },
    /// The resolved backend is unavailable on this host platform.
    BackendNotImplemented { backend: String },
}

impl TranscribeError {
    /// Process exit status for this error.
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::ModelAsset(_) => 78,
            Self::SpeakerTranscriptWrite(error) => match error {
                SpeakerTranscriptWriteError::PayloadUnreadable { .. }
                | SpeakerTranscriptWriteError::PayloadInvalid { .. }
                | SpeakerTranscriptWriteError::PayloadNonFinite { .. } => 69,
                SpeakerTranscriptWriteError::OutputUnwritable { .. }
                | SpeakerTranscriptWriteError::NpzVerificationFailed { .. }
                | SpeakerTranscriptWriteError::Internal { .. } => 75,
                SpeakerTranscriptWriteError::MalformedRequest { .. }
                | SpeakerTranscriptWriteError::UnknownSchema { .. }
                | SpeakerTranscriptWriteError::MissingStatementId { .. }
                | SpeakerTranscriptWriteError::InvalidStatementId { .. }
                | SpeakerTranscriptWriteError::DuplicateStatementId { .. }
                | SpeakerTranscriptWriteError::InvalidStatement { .. }
                | SpeakerTranscriptWriteError::InvalidHeader { .. }
                | SpeakerTranscriptWriteError::InvalidOutputPath { .. }
                | SpeakerTranscriptWriteError::DestinationExists { .. } => 1,
            },
            Self::OrphanNpzRemove { .. }
            | Self::TerminalPayload { .. }
            | Self::TerminalRequest { .. }
            | Self::TranscriptRequest { .. }
            | Self::TranscriptPayload { .. }
            | Self::VadTemporary { .. } => 75,
            Self::VadBinary { .. } => 78,
            Self::VadHelper {
                helper_exit_code, ..
            } => match helper_exit_code {
                69 => 69,
                75 => 75,
                _ => 1,
            },
            Self::TerminalWrite { .. } | Self::InputMetadata { .. } | Self::VadResponse { .. } => 1,
            Self::SttSurface { .. } => 1,
            Self::ParakeetCppDeferred { .. } => 69,
            Self::ParakeetCppFailure { .. } => 1,
            Self::ParakeetCoremlDeferred { .. } => 69,
            Self::ParakeetCoremlFailure { .. } => 1,
            Self::ConfidentialDeferred { .. } => 69,
            Self::SpeakerAnalysis(_) => 1,
            Self::Decode { .. } | Self::BackendNotImplemented { .. } => 1,
        }
    }
}

impl From<ModelAssetError> for TranscribeError {
    fn from(error: ModelAssetError) -> Self {
        Self::ModelAsset(error)
    }
}

impl From<SpeakerTranscriptWriteError> for TranscribeError {
    fn from(error: SpeakerTranscriptWriteError) -> Self {
        Self::SpeakerTranscriptWrite(error)
    }
}

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelAsset(error) => error.fmt(formatter),
            Self::SpeakerTranscriptWrite(error) => error.fmt(formatter),
            Self::OrphanNpzRemove { path, detail } => {
                write!(
                    formatter,
                    "could not remove orphan NPZ {}: {detail}",
                    path.display()
                )
            }
            Self::TerminalPayload { path, detail } => match path {
                Some(path) => write!(
                    formatter,
                    "could not prepare or remove terminal payload {}: {detail}",
                    path.display()
                ),
                None => write!(formatter, "could not prepare terminal payload: {detail}"),
            },
            Self::TerminalWrite { detail } => formatter.write_str(detail),
            Self::InputMetadata { path, detail } => {
                write!(
                    formatter,
                    "could not inspect input {}: {detail}",
                    path.display()
                )
            }
            Self::TerminalRequest { detail } => {
                write!(
                    formatter,
                    "could not serialize terminal writer request: {detail}"
                )
            }
            Self::TranscriptRequest { detail } => {
                write!(
                    formatter,
                    "could not serialize transcript writer request: {detail}"
                )
            }
            Self::TranscriptPayload { path, detail } => match path {
                Some(path) => write!(
                    formatter,
                    "could not prepare or remove transcript payload {}: {detail}",
                    path.display()
                ),
                None => write!(formatter, "could not prepare transcript payload: {detail}"),
            },
            Self::VadBinary { detail } | Self::VadTemporary { detail } => {
                formatter.write_str(detail)
            }
            Self::VadResponse {
                helper_exit_code,
                stderr,
                detail,
            } => match (helper_exit_code, stderr.is_empty()) {
                (Some(code), true) => write!(
                    formatter,
                    "VAD helper contract error (exit {code}): {detail}"
                ),
                (Some(code), false) => write!(
                    formatter,
                    "VAD helper contract error (exit {code}): {detail}: {stderr}"
                ),
                (None, true) => write!(
                    formatter,
                    "VAD helper contract error (no exit code): {detail}"
                ),
                (None, false) => write!(
                    formatter,
                    "VAD helper contract error (no exit code): {detail}: {stderr}"
                ),
            },
            Self::VadHelper {
                helper_exit_code,
                reason,
                detail,
            } => write!(
                formatter,
                "VAD helper {reason} (exit {helper_exit_code}): {detail}"
            ),
            Self::SttSurface {
                available_bytes,
                floor_bytes,
            } => write!(
                formatter,
                "no viable STT backend (available memory: {available_bytes:?}, local floor: {floor_bytes:?})"
            ),
            Self::ParakeetCppDeferred { reason, detail }
            | Self::ParakeetCppFailure { reason, detail } => {
                write!(formatter, "parakeet.cpp {reason}: {detail}")
            }
            Self::ParakeetCoremlDeferred { reason, detail }
            | Self::ParakeetCoremlFailure { reason, detail } => {
                write!(formatter, "parakeet CoreML {reason}: {detail}")
            }
            Self::ConfidentialDeferred { reason, detail } => {
                write!(formatter, "confidential transcription {reason}: {detail}")
            }
            Self::SpeakerAnalysis(error) => error.fmt(formatter),
            Self::Decode { detail } => formatter.write_str(detail),
            Self::BackendNotImplemented { backend } => {
                write!(
                    formatter,
                    "STT backend is unavailable on this host platform: {backend}"
                )
            }
        }
    }
}

impl std::error::Error for TranscribeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ModelAsset(error) => Some(error),
            Self::SpeakerTranscriptWrite(error) => Some(error),
            Self::SpeakerAnalysis(error) => Some(error),
            Self::OrphanNpzRemove { .. }
            | Self::TerminalPayload { .. }
            | Self::TerminalWrite { .. }
            | Self::InputMetadata { .. }
            | Self::TerminalRequest { .. }
            | Self::TranscriptRequest { .. }
            | Self::TranscriptPayload { .. }
            | Self::VadBinary { .. }
            | Self::VadTemporary { .. }
            | Self::VadHelper { .. }
            | Self::VadResponse { .. }
            | Self::SttSurface { .. }
            | Self::ParakeetCppDeferred { .. }
            | Self::ParakeetCppFailure { .. }
            | Self::ParakeetCoremlDeferred { .. }
            | Self::ParakeetCoremlFailure { .. }
            | Self::ConfidentialDeferred { .. } => None,
            Self::Decode { .. } | Self::BackendNotImplemented { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{ModelAssetError, TranscribeError, discover_audio_files, run_all_with};
    use crate::speakers::SpeakerAnalyzeError;

    /// The discovery order is the contract: `--all` used to build one global sorted
    /// list. Streaming per day must produce exactly the same sequence, because a
    /// full path sorts by its day directory first.
    #[test]
    fn discovery_streams_per_day_in_the_same_order_a_global_sort_produced() {
        let temporary = tempfile::TempDir::new().unwrap();
        let root = temporary.path();
        let mut expected = Vec::new();
        for day in ["20260902", "20260831", "20260901"] {
            for stream in ["watch", "device_mobile"] {
                for segment in ["101500_300", "090000_301"] {
                    let dir = root.join("chronicle").join(day).join(stream).join(segment);
                    fs::create_dir_all(&dir).unwrap();
                    for name in ["audio.m4a", "notes.txt", "audio.wav"] {
                        fs::write(dir.join(name), b"x").unwrap();
                    }
                    expected.push(dir.join("audio.m4a"));
                    expected.push(dir.join("audio.wav"));
                }
            }
        }
        // Days are walked newest-first (`sweep --all` reports recent work first);
        // within a day the order is the plain ascending walk. A stable re-sort by
        // descending day over an ascending list produces exactly that.
        expected.sort();
        expected.sort_by_key(|path| {
            let day = path
                .components()
                .skip_while(|part| part.as_os_str() != "chronicle")
                .nth(1)
                .map(|part| part.as_os_str().to_owned())
                .unwrap_or_default();
            std::cmp::Reverse(day)
        });

        let streamed: Vec<_> = discover_audio_files(root, &mut |_| {}).collect();
        assert_eq!(streamed, expected);
        // `notes.txt` is not transcribable and must not appear.
        assert!(
            streamed
                .iter()
                .all(|path| path.extension().is_some_and(|extension| extension != "txt"))
        );
    }

    #[test]
    fn discovery_yields_nothing_when_the_chronicle_is_absent() {
        let temporary = tempfile::TempDir::new().unwrap();
        assert_eq!(
            discover_audio_files(temporary.path(), &mut |_| {}).count(),
            0
        );
    }

    fn write_audio(temporary: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
        let path = temporary.path().join(name);
        fs::write(&path, b"audio").unwrap();
        path
    }

    fn write_chronicle_file(
        journal: &Path,
        day: &str,
        stream: &str,
        segment: &str,
        filename: &str,
        contents: &[u8],
    ) -> PathBuf {
        let directory = journal
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(filename);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn model_asset_errors_are_configuration_failures() {
        let error = TranscribeError::from(ModelAssetError::AssetNotFound {
            asset: "silero_vad_v6.onnx".to_owned(),
            searched: Vec::new(),
        });

        assert_eq!(error.exit_code(), 78);
    }

    #[test]
    fn batch_absorbs_deferred_and_speaker_analysis_failures_then_exits_zero() {
        let temporary = tempfile::tempdir().unwrap();
        let deferred = temporary.path().join("deferred.wav");
        let speaker = temporary.path().join("speaker.wav");
        fs::write(&deferred, b"audio").unwrap();
        fs::write(&speaker, b"audio").unwrap();
        let result = run_all_with(vec![deferred.clone(), speaker.clone()], false, |path| {
            if path.file_stem().unwrap() == "deferred" {
                return Err(TranscribeError::ParakeetCppDeferred {
                    reason: "server_not_ready".to_owned(),
                    detail: "test".to_owned(),
                });
            }
            Err(TranscribeError::SpeakerAnalysis(SpeakerAnalyzeError::new(
                path,
                "invoke",
                "timeout",
                Some(75),
            )))
        })
        .unwrap();
        assert_eq!(result.exit_code, 1);
        assert_eq!(
            result.summary.as_deref(),
            Some(
                "0 processed, 0 skipped (already transcribed), 1 deferred (provider not ready, will retry), 1 failed"
            )
        );
        let stderr = result.stderr.expect("failed items must report a cause");
        assert!(stderr.contains("transcription: 1 failed"), "{stderr}");
        assert!(
            stderr.contains(&format!(
                "{}: speaker analysis failed: invoke/timeout",
                speaker.display()
            )),
            "{stderr}"
        );
        assert!(
            !stderr.contains(&deferred.display().to_string()),
            "{stderr}"
        );
    }

    #[test]
    fn batch_one_failed_exits_one_and_reports_the_path() {
        let temporary = tempfile::tempdir().unwrap();
        let failed = write_audio(&temporary, "failed.wav");
        let result = run_all_with(vec![failed.clone()], false, |_| {
            Ok(crate::stage::ProcessOutcome::Failed)
        })
        .unwrap();
        assert_eq!(result.exit_code, 1);
        assert_eq!(
            result.summary.as_deref(),
            Some("0 processed, 0 skipped (already transcribed), 1 failed")
        );
        let stderr = result.stderr.expect("failed items must report a cause");
        assert!(stderr.contains("transcription: 1 failed"), "{stderr}");
        assert!(
            stderr.contains(&format!("{}: transcription failed", failed.display())),
            "{stderr}"
        );
    }

    #[test]
    fn batch_multi_failed_exits_one_and_reports_both_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let first = write_audio(&temporary, "first.wav");
        let second = write_audio(&temporary, "second.wav");
        let result = run_all_with(vec![first.clone(), second.clone()], false, |_| {
            Ok(crate::stage::ProcessOutcome::Failed)
        })
        .unwrap();
        assert_eq!(result.exit_code, 1);
        assert_eq!(
            result.summary.as_deref(),
            Some("0 processed, 0 skipped (already transcribed), 2 failed")
        );
        let stderr = result.stderr.expect("failed items must report a cause");
        assert!(stderr.contains("transcription: 2 failed"), "{stderr}");
        assert!(
            stderr.contains(&format!("{}: transcription failed", first.display())),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!("{}: transcription failed", second.display())),
            "{stderr}"
        );
    }

    #[test]
    fn batch_all_success_exits_zero_without_stderr() {
        let temporary = tempfile::tempdir().unwrap();
        let transcribed = write_audio(&temporary, "ok.wav");
        let result = run_all_with(vec![transcribed], false, |_| {
            Ok(crate::stage::ProcessOutcome::Transcribed)
        })
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.summary.as_deref(),
            Some("1 processed, 0 skipped (already transcribed)")
        );
        assert_eq!(result.stderr, None);
    }

    #[test]
    fn batch_deferred_only_exits_zero_without_stderr() {
        let temporary = tempfile::tempdir().unwrap();
        let deferred = write_audio(&temporary, "deferred.wav");
        let result = run_all_with(vec![deferred], false, |_| {
            Err(TranscribeError::ParakeetCppDeferred {
                reason: "server_not_ready".to_owned(),
                detail: "test".to_owned(),
            })
        })
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.summary.as_deref(),
            Some(
                "0 processed, 0 skipped (already transcribed), 1 deferred (provider not ready, will retry)"
            )
        );
        assert_eq!(result.stderr, None);
    }

    #[test]
    fn batch_stops_at_the_first_non_absorbed_error() {
        let temporary = tempfile::tempdir().unwrap();
        let first = temporary.path().join("first.wav");
        let second = temporary.path().join("second.wav");
        fs::write(&first, b"audio").unwrap();
        fs::write(&second, b"audio").unwrap();
        let calls = Cell::new(0);
        let error = run_all_with(vec![first, second], false, |_| {
            calls.set(calls.get() + 1);
            Err(TranscribeError::BackendNotImplemented {
                backend: "confidential".to_owned(),
            })
        })
        .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_eq!(error.exit_code(), 1);
    }

    #[test]
    fn discover_fires_once_per_day_newest_first() {
        let temporary = tempfile::tempdir().unwrap();
        let journal = temporary.path();
        write_chronicle_file(
            journal,
            "20260101",
            "audio",
            "120000_1",
            "audio.wav",
            b"audio",
        );
        write_chronicle_file(journal, "20260315", "audio", "110000_1", "a.wav", b"audio");
        write_chronicle_file(journal, "20260315", "audio", "120000_1", "b.wav", b"audio");
        write_chronicle_file(journal, "20260315", "screen", "130000_1", "c.wav", b"audio");
        write_chronicle_file(
            journal,
            "20260201",
            "audio",
            "120000_1",
            "audio.wav",
            b"audio",
        );
        write_chronicle_file(
            journal,
            "20260401",
            "audio",
            "120000_1",
            "notes.txt",
            b"not audio",
        );

        let mut fired = Vec::new();
        // The walk is lazy: without consuming the iterator `on_day` never fires and
        // this test would pass while exercising nothing.
        discover_audio_files(journal, &mut |day| fired.push(day.to_path_buf())).for_each(drop);

        let expected = ["20260401", "20260315", "20260201", "20260101"]
            .map(|day| journal.join("chronicle").join(day));
        assert_eq!(fired, expected);
    }

    #[test]
    fn discover_orders_days_descending_and_files_ascending_within_day() {
        let temporary = tempfile::tempdir().unwrap();
        let journal = temporary.path();
        let january = write_chronicle_file(
            journal,
            "20260101",
            "audio",
            "120000_1",
            "audio.wav",
            b"audio",
        );
        let march_screen =
            write_chronicle_file(journal, "20260315", "screen", "130000_1", "c.wav", b"audio");
        let march_early =
            write_chronicle_file(journal, "20260315", "audio", "110000_1", "a.wav", b"audio");
        let february = write_chronicle_file(
            journal,
            "20260201",
            "audio",
            "120000_1",
            "audio.wav",
            b"audio",
        );
        let march_late =
            write_chronicle_file(journal, "20260315", "audio", "120000_1", "b.wav", b"audio");

        let files: Vec<_> = discover_audio_files(journal, &mut |_| {}).collect();

        let mut march = vec![march_early, march_late, march_screen];
        march.sort();
        let expected = [march, vec![february], vec![january]].concat();
        assert_eq!(files, expected);
    }
}
