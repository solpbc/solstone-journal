// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native transcription orchestration.

// Config extraction is implemented before the stage machine that consumes it.
#[allow(dead_code)]
mod audio;
#[allow(dead_code)]
mod backend;
#[allow(dead_code)]
mod config;
mod model_assets;
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
#[allow(dead_code)]
mod transcript;

pub use model_assets::{ModelAssetError, resolve_model_asset};
pub use speakers::SpeakerAnalyzeError;

use std::path::PathBuf;

use solstone_core_speaker_id::writer::SpeakerTranscriptWriteError;

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
    /// Raw owner media could not be removed after terminal publication succeeded.
    RawInputRemove { path: PathBuf, detail: String },
    /// A terminal writer request could not be serialized.
    TerminalRequest { detail: String },
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
    VadResponse { detail: String },
    /// No configured, confidential, or resource-admissible STT backend exists.
    SttSurface {
        available_bytes: Option<u64>,
        floor_bytes: Option<u64>,
    },
    /// The supervised parakeet.cpp service is unavailable for retryable work.
    ParakeetCppDeferred { reason: String, detail: String },
    /// The parakeet.cpp service or client contract failed permanently.
    ParakeetCppFailure { reason: String, detail: String },
    /// Confidential transcription was refused or unavailable for retryable work.
    ConfidentialDeferred { reason: String, detail: String },
    /// The native speaker-analysis helper could not produce a usable result.
    SpeakerAnalysis(SpeakerAnalyzeError),
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
            | Self::RawInputRemove { .. }
            | Self::TerminalRequest { .. }
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
            Self::ConfidentialDeferred { .. } => 69,
            Self::SpeakerAnalysis(_) => 1,
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
            Self::RawInputRemove { path, detail } => {
                write!(
                    formatter,
                    "could not remove raw input {}: {detail}",
                    path.display()
                )
            }
            Self::TerminalRequest { detail } => {
                write!(
                    formatter,
                    "could not serialize terminal writer request: {detail}"
                )
            }
            Self::VadBinary { detail }
            | Self::VadTemporary { detail }
            | Self::VadResponse { detail } => formatter.write_str(detail),
            Self::VadHelper { reason, detail, .. } => {
                write!(formatter, "VAD helper {reason}: {detail}")
            }
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
            Self::ConfidentialDeferred { reason, detail } => {
                write!(formatter, "confidential transcription {reason}: {detail}")
            }
            Self::SpeakerAnalysis(error) => error.fmt(formatter),
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
            | Self::RawInputRemove { .. }
            | Self::TerminalRequest { .. }
            | Self::VadBinary { .. }
            | Self::VadTemporary { .. }
            | Self::VadHelper { .. }
            | Self::VadResponse { .. }
            | Self::SttSurface { .. }
            | Self::ParakeetCppDeferred { .. }
            | Self::ParakeetCppFailure { .. }
            | Self::ConfidentialDeferred { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelAssetError, TranscribeError};

    #[test]
    fn model_asset_errors_are_configuration_failures() {
        let error = TranscribeError::from(ModelAssetError::AssetNotFound {
            asset: "silero_vad_v6.onnx".to_owned(),
            searched: Vec::new(),
        });

        assert_eq!(error.exit_code(), 78);
    }
}
