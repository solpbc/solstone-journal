// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod filterbank;
mod segmentation;
mod statements;

use std::error::Error;
use std::fmt;

pub mod diarization;
pub mod discovery;

pub use filterbank::{
    WESPEAKER_EMBEDDING_SIZE, WESPEAKER_FFT_SIZE, WESPEAKER_FRAME_LENGTH_SAMPLES,
    WESPEAKER_FRAME_SHIFT_SAMPLES, WESPEAKER_MEL_BINS, WESPEAKER_SAMPLE_RATE_HZ,
    compute_wespeaker_filterbank_cmn, row_l2_normalize,
};
pub use segmentation::{
    DIARIZE_MIN_OVERLAP, PYANNOTE_CLASS_COUNT, PYANNOTE_DIARIZE_STRIDE_S,
    PYANNOTE_FRAMES_PER_WINDOW, PYANNOTE_OVERLAP_CLASSES, PYANNOTE_OVERLAP_STRIDE_S,
    PYANNOTE_SAMPLE_RATE_HZ, PYANNOTE_SINGLE_SPEAKER_CLASSES, PYANNOTE_WINDOW_S,
    PyannoteSegmentationPassResult, SLOT_ACTIVE_MIN_SHARE, SPEAKER_EVIDENCE_MULTI_MIN,
    SPEAKER_EVIDENCE_SINGLE_MAX, SpeakerEvidence, SpeakerEvidenceDecision,
    SpeakerSegmentationError, SpeakerWindowStats, compute_speaker_window_stats,
    decide_speaker_evidence, run_pyannote_segmentation_pass,
};
pub use statements::{
    AdmittedStatement, MIN_STATEMENT_DURATION_S, StatementAdmissionResult, StatementSpan,
    admit_statement_features,
};

#[derive(Debug, Clone, PartialEq)]
pub struct FeatureMatrix {
    frames: usize,
    bins: usize,
    data: Vec<f32>,
}

impl FeatureMatrix {
    pub fn from_row_major(
        frames: usize,
        bins: usize,
        data: Vec<f32>,
    ) -> Result<Self, SpeakerFeatureError> {
        let expected = frames
            .checked_mul(bins)
            .ok_or(SpeakerFeatureError::ShapeOverflow { frames, bins })?;
        if data.len() != expected {
            return Err(SpeakerFeatureError::ShapeMismatch {
                frames,
                bins,
                len: data.len(),
            });
        }
        Ok(Self { frames, bins, data })
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn bins(&self) -> usize {
        self.bins
    }

    pub fn data(&self) -> &[f32] {
        &self.data
    }

    pub fn row(&self, index: usize) -> Option<&[f32]> {
        if index >= self.frames {
            return None;
        }
        let start = index * self.bins;
        Some(&self.data[start..start + self.bins])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeakerFeatureError {
    UnsupportedSampleRate {
        expected: u32,
        actual: u32,
    },
    NonFiniteAudioSample {
        index: usize,
    },
    ShapeMismatch {
        frames: usize,
        bins: usize,
        len: usize,
    },
    ShapeOverflow {
        frames: usize,
        bins: usize,
    },
}

impl fmt::Display for SpeakerFeatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSampleRate { expected, actual } => write!(
                formatter,
                "unsupported sample rate: expected {expected}, got {actual}"
            ),
            Self::NonFiniteAudioSample { index } => {
                write!(formatter, "audio sample at index {index} is not finite")
            }
            Self::ShapeMismatch { frames, bins, len } => write!(
                formatter,
                "row-major matrix length mismatch: frames={frames} bins={bins} len={len}"
            ),
            Self::ShapeOverflow { frames, bins } => {
                write!(
                    formatter,
                    "row-major matrix shape overflow: frames={frames} bins={bins}"
                )
            }
        }
    }
}

impl Error for SpeakerFeatureError {}

#[cfg(test)]
pub(crate) mod test_support {
    use serde_json::Value;

    pub(crate) const FILTERBANK_FIXTURE: &str =
        include_str!("../../../fixtures/speaker_filterbank.json");
    pub(crate) const STAGE_FIXTURE: &str =
        include_str!("../../../fixtures/speaker_stage_boundaries.json");

    pub(crate) fn filterbank_fixture() -> Value {
        serde_json::from_str(FILTERBANK_FIXTURE).expect("filterbank fixture JSON")
    }

    pub(crate) fn stage_fixture() -> Value {
        serde_json::from_str(STAGE_FIXTURE).expect("stage fixture JSON")
    }

    pub(crate) fn decode_waveform(fixture: &Value) -> Vec<f32> {
        let encoded = fixture["waveform"]["samples_f32_le_base64"]
            .as_str()
            .expect("waveform base64");
        let bytes = decode_base64(encoded);
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("f32 bytes")))
            .collect()
    }

    pub(crate) fn fixture_matrix(fixture: &Value, name: &str) -> Vec<f32> {
        let rows = fixture["matrices"][name]["rows"]
            .as_array()
            .expect("matrix rows");
        rows.iter()
            .flat_map(|row| {
                row.as_str()
                    .expect("decimal row")
                    .split_whitespace()
                    .map(|value| value.parse::<f32>().expect("decimal value"))
            })
            .collect()
    }

    pub(crate) fn fixture_range(fixture: &Value, name: &str) -> std::ops::Range<usize> {
        let values = fixture["waveform"][name].as_array().expect("row range");
        let start = values[0].as_u64().expect("range start") as usize;
        let end = values[1].as_u64().expect("range end") as usize;
        start..end
    }

    pub(crate) fn assert_matrix_within(
        name: &str,
        actual: &[f32],
        expected: &[f32],
        tolerance: f32,
    ) {
        if let Some(error) = matrix_comparison_error(name, actual, expected, tolerance) {
            panic!("{error}");
        }
    }

    pub(crate) fn assert_region_within(
        name: &str,
        actual: &[f32],
        expected: &[f32],
        bins: usize,
        rows: std::ops::Range<usize>,
        tolerance: f32,
    ) {
        let start = rows.start * bins;
        let end = rows.end * bins;
        assert_matrix_within(name, &actual[start..end], &expected[start..end], tolerance);
    }

    pub(crate) fn matrix_comparison_error(
        name: &str,
        actual: &[f32],
        expected: &[f32],
        tolerance: f32,
    ) -> Option<String> {
        if actual.len() != expected.len() {
            return Some(format!(
                "{name} length mismatch: actual={} expected={}",
                actual.len(),
                expected.len()
            ));
        }
        let (max_abs, max_index) = max_abs_diff_with_index(actual, expected);
        if max_abs > tolerance {
            Some(format!(
                "{name} max_abs_diff={max_abs} at flat_index={max_index} tolerance={tolerance}"
            ))
        } else {
            None
        }
    }

    pub(crate) fn max_abs_diff(actual: &[f32], expected: &[f32]) -> f32 {
        max_abs_diff_with_index(actual, expected).0
    }

    fn max_abs_diff_with_index(actual: &[f32], expected: &[f32]) -> (f32, usize) {
        let mut max_abs = 0.0_f32;
        let mut max_index = 0;
        for (index, (left, right)) in actual.iter().zip(expected).enumerate() {
            let diff = (left - right).abs();
            if diff > max_abs {
                max_abs = diff;
                max_index = index;
            }
        }
        (max_abs, max_index)
    }

    pub(crate) fn decode_base64(input: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len() / 4 * 3);
        let mut quartet = [0_u8; 4];
        let mut len = 0;
        for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
            quartet[len] = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => 64,
                _ => panic!("invalid base64 byte: {byte}"),
            };
            len += 1;
            if len == 4 {
                out.push((quartet[0] << 2) | (quartet[1] >> 4));
                if quartet[2] != 64 {
                    out.push((quartet[1] << 4) | (quartet[2] >> 2));
                }
                if quartet[3] != 64 {
                    out.push((quartet[2] << 6) | quartet[3]);
                }
                len = 0;
            }
        }
        assert_eq!(len, 0);
        out
    }
}
