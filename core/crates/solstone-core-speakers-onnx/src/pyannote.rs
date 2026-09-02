// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use solstone_core_speakers::{FeatureMatrix, PYANNOTE_CLASS_COUNT, PYANNOTE_FRAMES_PER_WINDOW};

use crate::session::{ExpectedDim, expect_tensor, open_session};
use crate::{SpeakerExecutionProvider, SpeakerOnnxError, validate_pyannote_audio_window};

const INPUT_NAME: &str = "input_values";
const OUTPUT_NAME: &str = "logits";

#[derive(Debug)]
pub struct PyannoteSegmenter {
    session: Session,
    input_name: String,
    output_name: String,
}

impl PyannoteSegmenter {
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

    pub fn infer_window(
        &mut self,
        audio_window: &[f32],
    ) -> Result<FeatureMatrix, SpeakerOnnxError> {
        validate_pyannote_audio_window(audio_window)?;
        let input = Tensor::from_array((
            [1_usize, 1_usize, audio_window.len()],
            audio_window.to_vec().into_boxed_slice(),
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
        if shape[..]
            != [
                1,
                PYANNOTE_FRAMES_PER_WINDOW as i64,
                PYANNOTE_CLASS_COUNT as i64,
            ]
        {
            return Err(SpeakerOnnxError::InvalidModelIo {
                detail: format!(
                    "output shape {shape} is not [1, {PYANNOTE_FRAMES_PER_WINDOW}, {PYANNOTE_CLASS_COUNT}]"
                ),
            });
        }
        FeatureMatrix::from_row_major(
            PYANNOTE_FRAMES_PER_WINDOW,
            PYANNOTE_CLASS_COUNT,
            values.to_vec(),
        )
        .map_err(|error| SpeakerOnnxError::InvalidModelIo {
            detail: error.to_string(),
        })
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
        &[ExpectedDim::Any, ExpectedDim::Any, ExpectedDim::Any],
    )?;
    expect_tensor(
        "output",
        outputs[0].name(),
        outputs[0].dtype(),
        OUTPUT_NAME,
        &[
            ExpectedDim::Any,
            ExpectedDim::Any,
            ExpectedDim::Exact(PYANNOTE_CLASS_COUNT as i64),
        ],
    )?;
    Ok(())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::test_support::repo_root;
    use solstone_core_speakers::{PYANNOTE_SAMPLE_RATE_HZ, PYANNOTE_WINDOW_S};

    #[test]
    fn committed_pyannote_model_accepts_zero_window_and_returns_589x7_finite_values() {
        let model_path = repo_root().join("core/models/assets/pyannote-segmentation-3.0.onnx");
        let mut segmenter = PyannoteSegmenter::open(&model_path, &[SpeakerExecutionProvider::Cpu])
            .expect("segmenter");
        let audio = vec![0.0; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];

        let log_probs = segmenter.infer_window(&audio).expect("log probs");

        assert_eq!(log_probs.frames(), PYANNOTE_FRAMES_PER_WINDOW);
        assert_eq!(log_probs.bins(), PYANNOTE_CLASS_COUNT);
        assert!(log_probs.data().iter().all(|value| value.is_finite()));
    }
}
