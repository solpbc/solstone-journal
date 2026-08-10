// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native transcription orchestration.

// Config extraction is implemented before the stage machine that consumes it.
#[allow(dead_code)]
mod config;
mod model_assets;

pub use model_assets::{ModelAssetError, resolve_model_asset};

/// Errors currently emitted by the native transcription contract.
#[derive(Debug)]
pub enum TranscribeError {
    /// A bundled model asset could not be resolved.
    ModelAsset(ModelAssetError),
}

impl TranscribeError {
    /// Process exit status for this error.
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::ModelAsset(_) => 78,
        }
    }
}

impl From<ModelAssetError> for TranscribeError {
    fn from(error: ModelAssetError) -> Self {
        Self::ModelAsset(error)
    }
}

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelAsset(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TranscribeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ModelAsset(error) => Some(error),
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
