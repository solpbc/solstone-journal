// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use solstone_core_speakers::{FeatureMatrix, WESPEAKER_EMBEDDING_SIZE, WESPEAKER_MEL_BINS};

use crate::session::{ExpectedDim, expect_tensor, open_session};
use crate::{SpeakerExecutionProvider, SpeakerOnnxError};

const INPUT_NAME: &str = "feats";
const OUTPUT_NAME: &str = "embs";

#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerEmbedding {
    values: [f32; WESPEAKER_EMBEDDING_SIZE],
}

impl SpeakerEmbedding {
    pub fn values(&self) -> &[f32; WESPEAKER_EMBEDDING_SIZE] {
        &self.values
    }
}

#[derive(Debug)]
pub struct WespeakerEmbedder {
    session: Session,
    input_name: String,
    output_name: String,
}

impl WespeakerEmbedder {
    pub fn open(
        model_path: &Path,
        providers: &[SpeakerExecutionProvider],
    ) -> Result<Self, SpeakerOnnxError> {
        let session = open_session(model_path, providers)?;
        validate_session_io(&session)?;
        Ok(Self {
            session,
            input_name: INPUT_NAME.to_string(),
            output_name: OUTPUT_NAME.to_string(),
        })
    }

    pub fn embed(
        &mut self,
        features: &FeatureMatrix,
    ) -> Result<SpeakerEmbedding, SpeakerOnnxError> {
        if features.frames() == 0 || features.bins() != WESPEAKER_MEL_BINS {
            return Err(SpeakerOnnxError::InvalidFeatureMatrix {
                frames: features.frames(),
                bins: features.bins(),
            });
        }
        let input = Tensor::from_array((
            [1_usize, features.frames(), WESPEAKER_MEL_BINS],
            features.data().to_vec().into_boxed_slice(),
        ))?;
        let mut outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => input])?;
        let output =
            outputs
                .remove(&self.output_name)
                .ok_or_else(|| SpeakerOnnxError::MissingOutput {
                    name: self.output_name.clone(),
                })?;
        let (shape, values) = output.try_extract_tensor::<f32>()?;
        if shape[..] != [1, WESPEAKER_EMBEDDING_SIZE as i64] {
            return Err(SpeakerOnnxError::InvalidModelIo {
                detail: format!("output shape {shape} is not [1, {WESPEAKER_EMBEDDING_SIZE}]"),
            });
        }
        let mut embedding = [0.0; WESPEAKER_EMBEDDING_SIZE];
        embedding.copy_from_slice(values);
        Ok(SpeakerEmbedding { values: embedding })
    }
}

fn validate_session_io(session: &Session) -> Result<(), SpeakerOnnxError> {
    let inputs = session.inputs();
    let outputs = session.outputs();
    if inputs.len() != 1 {
        return Err(SpeakerOnnxError::InvalidModelIo {
            detail: format!("expected one input, got {}", inputs.len()),
        });
    }
    if outputs.len() != 1 {
        return Err(SpeakerOnnxError::InvalidModelIo {
            detail: format!("expected one output, got {}", outputs.len()),
        });
    }
    expect_tensor(
        "input",
        inputs[0].name(),
        inputs[0].dtype(),
        INPUT_NAME,
        &[
            ExpectedDim::Any,
            ExpectedDim::Any,
            ExpectedDim::Exact(WESPEAKER_MEL_BINS as i64),
        ],
    )?;
    expect_tensor(
        "output",
        outputs[0].name(),
        outputs[0].dtype(),
        OUTPUT_NAME,
        &[
            ExpectedDim::Any,
            ExpectedDim::Exact(WESPEAKER_EMBEDDING_SIZE as i64),
        ],
    )?;
    Ok(())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::test_support::{decode_waveform, fixture, repo_root};
    use solstone_core_speakers::{WESPEAKER_SAMPLE_RATE_HZ, compute_wespeaker_filterbank_cmn};

    #[test]
    fn committed_wespeaker_model_accepts_fixture_features_and_returns_256_floats() {
        let fixture = fixture();
        let audio = decode_waveform(&fixture);
        let features =
            compute_wespeaker_filterbank_cmn(&audio, WESPEAKER_SAMPLE_RATE_HZ).expect("features");
        let model_path = repo_root().join("core/models/assets/wespeaker-resnet34-256.onnx");
        let mut embedder = WespeakerEmbedder::open(&model_path, &[SpeakerExecutionProvider::Cpu])
            .expect("embedder");

        let embedding = embedder.embed(&features).expect("embedding");

        assert_eq!(embedding.values().len(), WESPEAKER_EMBEDDING_SIZE);
        assert!(embedding.values().iter().all(|value| value.is_finite()));
    }
}
