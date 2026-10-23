// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use solstone_core_speakers::{FeatureMatrix, compute_kaldi_filterbank};

use crate::{FIRERED_VAD_CMVN_SHA256, SAMPLE_RATE_HZ, VadError};

pub(crate) const FEATURE_BINS: usize = 80;
const PCM16_SCALE: f32 = 32768.0;
const CMVN_BYTES: &[u8] = include_bytes!("../../../models/assets/firered_vad_cmvn.ark");
const CMVN_HEADER: &[u8] = &[0, b'B', b'D', b'M', b' ', 4, 2, 0, 0, 0, 4, 81, 0, 0, 0];

#[derive(Debug)]
struct Cmvn {
    means: [f64; FEATURE_BINS],
    inverse_std: [f64; FEATURE_BINS],
}

impl Cmvn {
    fn decode(bytes: &[u8]) -> Result<Self, VadError> {
        let expected_length = CMVN_HEADER.len() + 2 * (FEATURE_BINS + 1) * size_of::<f64>();
        if bytes.len() != expected_length || !bytes.starts_with(CMVN_HEADER) {
            return Err(cmvn_error("expected one binary 2x81 Kaldi double matrix"));
        }
        let mut stats = [0.0_f64; 2 * (FEATURE_BINS + 1)];
        for (value, bytes) in stats
            .iter_mut()
            .zip(bytes[CMVN_HEADER.len()..].chunks_exact(size_of::<f64>()))
        {
            *value = f64::from_le_bytes(bytes.try_into().expect("eight bytes"));
        }
        if stats.iter().any(|value| !value.is_finite()) {
            return Err(cmvn_error("statistics contain a non-finite value"));
        }
        let count = stats[FEATURE_BINS];
        if count < 1.0 {
            return Err(cmvn_error("statistics count must be at least one"));
        }
        let mut means = [0.0; FEATURE_BINS];
        let mut inverse_std = [0.0; FEATURE_BINS];
        for bin in 0..FEATURE_BINS {
            let mean = stats[bin] / count;
            let variance = stats[FEATURE_BINS + 1 + bin] / count - mean * mean;
            if !variance.is_finite() {
                return Err(cmvn_error("derived variance is not finite"));
            }
            means[bin] = mean;
            inverse_std[bin] = 1.0 / variance.max(1e-20).sqrt();
        }
        Ok(Self { means, inverse_std })
    }

    fn normalize(&self, features: &FeatureMatrix) -> Result<FeatureMatrix, VadError> {
        let data = features
            .data()
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let bin = index % FEATURE_BINS;
                ((f64::from(*value) - self.means[bin]) * self.inverse_std[bin]) as f32
            })
            .collect();
        FeatureMatrix::from_row_major(features.frames(), FEATURE_BINS, data).map_err(|error| {
            VadError::Internal {
                detail: format!("FireRed feature shape is invalid: {error}"),
            }
        })
    }
}

fn cmvn_error(detail: &str) -> VadError {
    VadError::ModelInvalid {
        path: "embedded FireRed CMVN".to_string(),
        detail: detail.to_string(),
    }
}

fn check_cmvn_identity(bytes: &[u8]) -> Result<(), VadError> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != FIRERED_VAD_CMVN_SHA256 {
        return Err(VadError::ModelIdentityMismatch {
            path: "embedded FireRed CMVN".to_string(),
            expected: FIRERED_VAD_CMVN_SHA256,
            actual,
        });
    }
    Ok(())
}

fn quantize_pcm16(audio: &[f32]) -> Result<Vec<f32>, VadError> {
    audio
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            if !sample.is_finite() {
                return Err(VadError::AudioNonFinite {
                    path: "input audio".to_string(),
                    index,
                });
            }
            Ok((sample * PCM16_SCALE)
                .round_ties_even()
                .clamp(-32768.0, 32767.0)
                / PCM16_SCALE)
        })
        .collect()
}

pub(crate) fn prepare_features(audio: &[f32]) -> Result<FeatureMatrix, VadError> {
    check_cmvn_identity(CMVN_BYTES)?;
    let quantized = quantize_pcm16(audio)?;
    let raw = compute_kaldi_filterbank(&quantized, SAMPLE_RATE_HZ).map_err(|error| {
        VadError::Internal {
            detail: format!("FireRed filterbank extraction failed: {error}"),
        }
    })?;
    Cmvn::decode(CMVN_BYTES)?.normalize(&raw)
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    fn matrix_bytes(count: f64, sum: f64, squares: f64) -> Vec<u8> {
        let mut stats = [0.0_f64; 2 * (FEATURE_BINS + 1)];
        stats[..FEATURE_BINS].fill(sum);
        stats[FEATURE_BINS] = count;
        stats[FEATURE_BINS + 1..2 * FEATURE_BINS + 1].fill(squares);
        let mut bytes = CMVN_HEADER.to_vec();
        for value in stats {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn cmvn_decodes_double_statistics_and_normalizes_before_casting() {
        let cmvn = Cmvn::decode(&matrix_bytes(2.0, 6.0, 26.0)).expect("CMVN");
        let raw = FeatureMatrix::from_row_major(1, FEATURE_BINS, vec![5.0; FEATURE_BINS])
            .expect("features");
        let result = cmvn.normalize(&raw).expect("normalized");
        assert_eq!(cmvn.means, [3.0; FEATURE_BINS]);
        assert_eq!(cmvn.inverse_std, [0.5; FEATURE_BINS]);
        assert_eq!(result.data(), &[1.0; FEATURE_BINS]);
    }

    #[test]
    fn cmvn_floors_zero_variance() {
        let cmvn = Cmvn::decode(&matrix_bytes(1.0, 2.0, 4.0)).expect("CMVN");
        assert_eq!(cmvn.inverse_std, [1e10; FEATURE_BINS]);
    }

    #[test]
    fn cmvn_refuses_wrong_layout_truncation_and_trailing_bytes() {
        let valid = matrix_bytes(2.0, 6.0, 26.0);
        for header_index in [1, 2, 6, 11] {
            let mut bad = valid.clone();
            bad[header_index] ^= 1;
            assert!(Cmvn::decode(&bad).is_err());
        }
        assert!(Cmvn::decode(&valid[..valid.len() - 1]).is_err());
        let mut trailing = valid;
        trailing.push(0);
        assert!(Cmvn::decode(&trailing).is_err());
    }

    #[test]
    fn cmvn_refuses_nonfinite_statistics_and_invalid_counts() {
        for count in [0.0, -1.0, 0.5, f64::NAN, f64::INFINITY] {
            assert!(Cmvn::decode(&matrix_bytes(count, 6.0, 26.0)).is_err());
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Cmvn::decode(&matrix_bytes(2.0, value, 26.0)).is_err());
            assert!(Cmvn::decode(&matrix_bytes(2.0, 6.0, value)).is_err());
        }
        assert!(Cmvn::decode(&matrix_bytes(1.0, f64::MAX, 1.0)).is_err());
    }

    #[test]
    fn changed_finite_statistics_refuse_the_pinned_model_pairing() {
        let mut changed = CMVN_BYTES.to_vec();
        changed[CMVN_HEADER.len()] ^= 1;
        Cmvn::decode(&changed).expect("changed statistics remain structurally valid");
        assert_eq!(
            check_cmvn_identity(&changed).unwrap_err().reason(),
            "model-identity-mismatch"
        );
        check_cmvn_identity(CMVN_BYTES).expect("original pairing");
    }

    #[test]
    fn quantization_uses_ties_to_even_and_pcm16_saturation() {
        let pcm = [-40000.0, -32768.0, -2.5, -1.5, -0.5, 0.5, 1.5, 2.5, 32768.0];
        let audio = pcm.map(|sample| sample / PCM16_SCALE);
        let actual = quantize_pcm16(&audio).expect("quantized");
        let expected = [-32768.0, -32768.0, -2.0, -2.0, 0.0, 0.0, 2.0, 2.0, 32767.0]
            .map(|sample| sample / PCM16_SCALE);
        assert_eq!(actual, expected);
        assert!(quantize_pcm16(&[f32::NAN]).is_err());
    }

    #[test]
    fn snipped_frame_counts_include_each_complete_window_once() {
        for (samples, frames) in [(1, 0), (399, 0), (400, 1), (559, 1), (560, 2)] {
            let features = prepare_features(&vec![0.0; samples]).expect("features");
            assert_eq!(features.frames(), frames, "{samples} samples");
            assert_eq!(features.data().len(), frames * FEATURE_BINS);
        }
    }
}
