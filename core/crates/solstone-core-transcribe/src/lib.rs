// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native transcription orchestration.

// Config extraction is implemented before the stage machine that consumes it.
#[allow(dead_code)]
mod config;
mod model_assets;
// The standalone CLI is introduced in a later step; retain the completed
// stage pieces without treating that staged integration as a lint failure.
#[allow(dead_code)]
mod processing;
#[allow(dead_code)]
mod stage;
#[allow(dead_code)]
mod terminal;
#[allow(dead_code)]
mod transcript;

pub use model_assets::{ModelAssetError, resolve_model_asset};

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
            | Self::TerminalRequest { .. } => 75,
            Self::TerminalWrite { .. } | Self::InputMetadata { .. } => 1,
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
        }
    }
}

impl std::error::Error for TranscribeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ModelAsset(error) => Some(error),
            Self::SpeakerTranscriptWrite(error) => Some(error),
            Self::OrphanNpzRemove { .. }
            | Self::TerminalPayload { .. }
            | Self::TerminalWrite { .. }
            | Self::InputMetadata { .. }
            | Self::RawInputRemove { .. }
            | Self::TerminalRequest { .. } => None,
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
