// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use ort::editor::{Graph, Model, ONNX_DOMAIN, Opset};
use ort::session::{OutputSelector, RunOptions, Session};
use ort::value::{Outlet, Shape, SymbolicDimensions, Tensor, TensorElementType, ValueType};
use solstone_core_speakers::{FeatureMatrix, WESPEAKER_EMBEDDING_SIZE, WESPEAKER_MEL_BINS};

use crate::onnx_init::Projection;
use crate::session::{ExpectedDim, expect_tensor, provider_dispatches};
use crate::{SpeakerExecutionProvider, SpeakerOnnxError};

const INPUT_NAME: &str = "feats";
const PREFIX_OUTPUT: &str = "onnx::ReduceMean_343";
const STRIDE: usize = 8;
const HALO: usize = 112;
const BANDS: usize = 10;
const FLAT: usize = 256 * BANDS;
const CORE_OUTPUTS: usize = 32;
/// The graph's exact f32 addend after the unbiased variance, `1e-7`.
const VARIANCE_EPS: f32 = f32::from_bits(0x33d6_bf95);

#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerEmbedding {
    values: [f32; WESPEAKER_EMBEDDING_SIZE],
}

impl SpeakerEmbedding {
    pub fn values(&self) -> &[f32; WESPEAKER_EMBEDDING_SIZE] {
        &self.values
    }
}

pub struct WespeakerEmbedder {
    session: Session,
    projection: Projection,
}

impl WespeakerEmbedder {
    pub fn open(
        model_path: &Path,
        providers: &[SpeakerExecutionProvider],
    ) -> Result<Self, SpeakerOnnxError> {
        if providers.is_empty() {
            return Err(SpeakerOnnxError::EmptyProviderPlan);
        }
        #[cfg(windows)]
        let admitted_model = crate::windows_runtime::bootstrap_windows_speaker_model(model_path)
            .map_err(|message| SpeakerOnnxError::Ort { message })?;
        #[cfg(windows)]
        let model_path = admitted_model.as_path();
        let projection = crate::onnx_init::read_projection(model_path)?;
        let dispatches = provider_dispatches(providers)?;
        let mut builder = Session::builder()?.with_execution_providers(dispatches)?;
        let mut edited = builder.edit_from_file(model_path)?;
        let mut graph = Graph::new()?;
        graph.set_outputs([Outlet::new(
            PREFIX_OUTPUT,
            ValueType::Tensor {
                ty: TensorElementType::Float32,
                shape: Shape::new([-1, 256, BANDS as i64, -1]),
                dimension_symbols: SymbolicDimensions::new([
                    "B".to_owned(),
                    String::new(),
                    String::new(),
                    "T".to_owned(),
                ]),
            },
        )])?;
        let mut model = Model::new([Opset::new(ONNX_DOMAIN, 14)?])?;
        model.add_graph(graph)?;
        edited.apply_model(&model)?;
        let session = edited.into_session()?;
        // The run selects the prefix by name. Rust's cached output list is not
        // that contract: it can keep reporting `embs` after the editor runs.
        validate_session_input(&session)?;
        Ok(Self {
            session,
            projection,
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
        let count = features.frames().div_ceil(STRIDE);
        if count < 2 {
            return Err(SpeakerOnnxError::InvalidFeatureMatrix {
                frames: features.frames(),
                bins: features.bins(),
            });
        }
        let mut sum = vec![0.0_f64; FLAT];
        let mut sumsq = vec![0.0_f64; FLAT];
        let mut next = 0_usize;
        while next < count {
            let end = (next + CORE_OUTPUTS).min(count);
            let took = self.accumulate_window(features, next, end, &mut sum, &mut sumsq)?;
            if took == 0 {
                return Err(SpeakerOnnxError::InvalidModelIo {
                    detail: format!("prefix window at output {next} covered no frames"),
                });
            }
            next += took;
        }
        if next != count {
            return Err(SpeakerOnnxError::InvalidModelIo {
                detail: format!("prefix covered {next} of {count} frames"),
            });
        }
        self.project(&sum, &sumsq, count)
    }

    fn accumulate_window(
        &mut self,
        features: &FeatureMatrix,
        start: usize,
        end: usize,
        sum: &mut [f64],
        sumsq: &mut [f64],
    ) -> Result<usize, SpeakerOnnxError> {
        let frames = features.frames();
        let ws = (start * STRIDE).saturating_sub(HALO);
        let we = ((end - 1) * STRIDE + HALO + 1).min(frames);
        if we <= ws {
            return Ok(0);
        }
        let bins = features.bins();
        let prefix = self.run_prefix(&features.data()[ws * bins..we * bins], we - ws)?;
        let local_base = ws / STRIDE;
        let mut took = 0_usize;
        for global in start..end {
            if !prefix_frame_is_inside(global, frames, ws, we) {
                break;
            }
            let local = global - local_base;
            if local >= prefix.tout {
                break;
            }
            for band in 0..FLAT {
                let value = f64::from(prefix.values[band * prefix.tout + local]);
                sum[band] += value;
                sumsq[band] += value * value;
            }
            took += 1;
        }
        Ok(took)
    }

    fn run_prefix(&mut self, samples: &[f32], frames: usize) -> Result<Prefix, SpeakerOnnxError> {
        let input = Tensor::from_array((
            [1_usize, frames, WESPEAKER_MEL_BINS],
            samples.to_vec().into_boxed_slice(),
        ))?;
        let options =
            RunOptions::new()?.with_outputs(OutputSelector::no_default().with(PREFIX_OUTPUT));
        let outputs = self
            .session
            .run_with_options(ort::inputs![INPUT_NAME => input], &options)?;
        let (shape, values) = outputs[0].try_extract_tensor::<f32>()?;
        if shape.len() != 4 || shape[0] != 1_i64 || shape[1] != 256 || shape[2] != BANDS as i64 {
            return Err(SpeakerOnnxError::InvalidModelIo {
                detail: format!("prefix shape {shape} is not [1, 256, {BANDS}, time]"),
            });
        }
        let tout = usize::try_from(shape[3]).map_err(|error| SpeakerOnnxError::InvalidModelIo {
            detail: error.to_string(),
        })?;
        if values.len() != FLAT * tout {
            return Err(SpeakerOnnxError::InvalidModelIo {
                detail: format!("prefix payload {} is not {FLAT} by {tout}", values.len()),
            });
        }
        Ok(Prefix {
            tout,
            values: values.to_vec(),
        })
    }

    fn project(
        &self,
        sum: &[f64],
        sumsq: &[f64],
        count: usize,
    ) -> Result<SpeakerEmbedding, SpeakerOnnxError> {
        let n = count as f64;
        let mut pooled = [0.0_f32; FLAT * 2];
        for index in 0..FLAT {
            let mean = sum[index] / n;
            let second = sumsq[index] / n;
            let variance = (second - mean * mean).max(0.0) * n / (n - 1.0);
            pooled[index] = mean as f32;
            pooled[FLAT + index] = (variance + f64::from(VARIANCE_EPS)).sqrt() as f32;
        }
        let mut values = [0.0_f32; WESPEAKER_EMBEDDING_SIZE];
        for (row, slot) in values.iter_mut().enumerate() {
            let mut acc = f64::from(self.projection.bias[row]);
            let weights = &self.projection.weight[row * pooled.len()..(row + 1) * pooled.len()];
            for (value, coeff) in pooled.iter().zip(weights) {
                acc += f64::from(*value) * f64::from(*coeff);
            }
            *slot = (acc - f64::from(self.projection.mean[row])) as f32;
        }
        Ok(SpeakerEmbedding { values })
    }
}

struct Prefix {
    tout: usize,
    values: Vec<f32>,
}

fn prefix_frame_is_inside(global: usize, frames: usize, ws: usize, we: usize) -> bool {
    let center = global * STRIDE;
    let rf_start = center as isize - HALO as isize;
    let rf_end = center + HALO + 1;
    let left_ok = if rf_start >= 0 {
        rf_start as usize >= ws
    } else {
        ws == 0
    };
    let right_ok = if rf_end <= frames {
        rf_end <= we
    } else {
        we == frames
    };
    left_ok && right_ok
}

fn validate_session_input(session: &Session) -> Result<(), SpeakerOnnxError> {
    let inputs = session.inputs();
    if inputs.len() != 1 {
        return Err(SpeakerOnnxError::InvalidModelIo {
            detail: format!("expected one input, got {}", inputs.len()),
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

    #[test]
    fn bounded_windows_match_the_original_embedding() {
        let model_path = repo_root().join("core/models/assets/wespeaker-resnet34-256.onnx");
        let mut bounded =
            WespeakerEmbedder::open(&model_path, &[SpeakerExecutionProvider::Cpu]).expect("open");
        let mut original =
            crate::session::open_session(&model_path, &[SpeakerExecutionProvider::Cpu])
                .expect("original session");
        let fixture = fixture();
        let audio = decode_waveform(&fixture);
        let features =
            compute_wespeaker_filterbank_cmn(&audio, WESPEAKER_SAMPLE_RATE_HZ).expect("features");
        let fixture_delta = embedding_delta(&mut bounded, &mut original, &features);
        let window_features = synthetic_features(605);
        let window_delta = embedding_delta(&mut bounded, &mut original, &window_features);
        assert!(
            fixture_delta.max <= 1.0e-6 && window_delta.max <= 1.0e-6,
            "fixture frames {} max abs {} at {}; windows frames {} max abs {} at {}",
            features.frames(),
            fixture_delta.max,
            fixture_delta.at,
            window_features.frames(),
            window_delta.max,
            window_delta.at
        );
    }

    struct EmbeddingDelta {
        max: f32,
        at: usize,
    }

    fn embedding_delta(
        bounded: &mut WespeakerEmbedder,
        original: &mut Session,
        features: &FeatureMatrix,
    ) -> EmbeddingDelta {
        let got = bounded.embed(features).expect("bounded embedding");
        let expect = original_embedding(original, features);
        assert!(
            got.values()
                .iter()
                .chain(expect.iter())
                .all(|value| value.is_finite())
        );
        let mut max = 0.0_f32;
        let mut at = 0_usize;
        for (index, (left, right)) in got.values().iter().zip(expect.iter()).enumerate() {
            let delta = (left - right).abs();
            if delta > max {
                max = delta;
                at = index;
            }
        }
        EmbeddingDelta { max, at }
    }

    fn original_embedding(
        session: &mut Session,
        features: &FeatureMatrix,
    ) -> [f32; WESPEAKER_EMBEDDING_SIZE] {
        const OUTPUT_NAME: &str = "embs";
        let input = Tensor::from_array((
            [1_usize, features.frames(), WESPEAKER_MEL_BINS],
            features.data().to_vec().into_boxed_slice(),
        ))
        .expect("reference input");
        let mut outputs = session
            .run(ort::inputs![INPUT_NAME => input])
            .expect("reference run");
        let output = outputs.remove(OUTPUT_NAME).expect("embs");
        let (shape, values) = output.try_extract_tensor::<f32>().expect("extract");
        assert_eq!(shape[..], [1, WESPEAKER_EMBEDDING_SIZE as i64]);
        let mut embedding = [0.0_f32; WESPEAKER_EMBEDDING_SIZE];
        embedding.copy_from_slice(values);
        embedding
    }

    fn synthetic_features(frames: usize) -> FeatureMatrix {
        let mut data = Vec::with_capacity(frames * WESPEAKER_MEL_BINS);
        for frame in 0..frames {
            for bin in 0..WESPEAKER_MEL_BINS {
                let value = ((frame * 17 + bin * 3) % 100) as f32 / 50.0 - 1.0;
                data.push(value);
            }
        }
        FeatureMatrix::from_row_major(frames, WESPEAKER_MEL_BINS, data).expect("synthetic")
    }
}
