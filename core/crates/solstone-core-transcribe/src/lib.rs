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
pub use speakers_installation::{SpeakersAnalyzeGeneration, enter_speakers_analyze_generation};
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
    let _generation = enter_speakers_analyze_generation(journal_path).map_err(CliRunError::Cli)?;
    let attestation_state = AttestationStateStore::new();
    if parsed.all {
        return run_all(
            journal_path,
            parsed.redo,
            &backend.backend,
            &config,
            &attestation_state,
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
) -> Result<CliRun, CliRunError> {
    run_all_with(discover_audio_files(journal_path), redo, |audio_path| {
        stage::process_one(
            audio_path,
            journal_path,
            redo,
            Some(backend),
            config,
            attestation_state,
        )
    })
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

fn discover_audio_files(journal_path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(days) = std::fs::read_dir(journal_path.join("chronicle")) else {
        return files;
    };
    for day in days
        .flatten()
        .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
    {
        let Ok(streams) = std::fs::read_dir(day.path()) else {
            continue;
        };
        for stream in streams
            .flatten()
            .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
        {
            let Ok(segments) = std::fs::read_dir(stream.path()) else {
                continue;
            };
            for segment in segments
                .flatten()
                .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
            {
                let Ok(entries) = std::fs::read_dir(segment.path()) else {
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
                        files.push(path);
                    }
                }
            }
        }
    }
    files.sort();
    files
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

    use super::{ModelAssetError, TranscribeError, run_all_with};
    use crate::speakers::SpeakerAnalyzeError;

    fn write_audio(temporary: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
        let path = temporary.path().join(name);
        fs::write(&path, b"audio").unwrap();
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
}
