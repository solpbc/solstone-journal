// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::f32::consts::PI;

use crate::{FeatureMatrix, SpeakerFeatureError};

pub const WESPEAKER_SAMPLE_RATE_HZ: u32 = 16_000;
pub const WESPEAKER_MEL_BINS: usize = 80;
pub const WESPEAKER_FRAME_LENGTH_SAMPLES: usize = 400;
pub const WESPEAKER_FRAME_SHIFT_SAMPLES: usize = 160;
pub const WESPEAKER_FFT_SIZE: usize = 512;
pub const WESPEAKER_EMBEDDING_SIZE: usize = 256;

const LOW_FREQ_HZ: f32 = 20.0;
const HIGH_FREQ_HZ: f32 = WESPEAKER_SAMPLE_RATE_HZ as f32 / 2.0;
const PREEMPH_COEFF: f32 = 0.97;
const AUDIO_SCALE: f32 = 32768.0;
const ROW_NORM_EPSILON: f32 = 1e-9;

pub fn compute_wespeaker_filterbank_cmn(
    audio: &[f32],
    sample_rate_hz: u32,
) -> Result<FeatureMatrix, SpeakerFeatureError> {
    if sample_rate_hz != WESPEAKER_SAMPLE_RATE_HZ {
        return Err(SpeakerFeatureError::UnsupportedSampleRate {
            expected: WESPEAKER_SAMPLE_RATE_HZ,
            actual: sample_rate_hz,
        });
    }
    if let Some((index, _sample)) = audio
        .iter()
        .enumerate()
        .find(|(_index, sample)| !sample.is_finite())
    {
        return Err(SpeakerFeatureError::NonFiniteAudioSample { index });
    }

    let frames = num_snipped_frames(audio.len());
    let mut data = vec![0.0; frames * WESPEAKER_MEL_BINS];
    if frames == 0 {
        return FeatureMatrix::from_row_major(0, WESPEAKER_MEL_BINS, data);
    }

    let window = povey_window();
    let mel_bank = MelBank::production();
    for frame in 0..frames {
        let mut padded = padded_frame(audio, frame, &window);
        let power = power_spectrum_512(&mut padded);
        let output_start = frame * WESPEAKER_MEL_BINS;
        mel_bank.compute_log_energies(
            &power,
            &mut data[output_start..output_start + WESPEAKER_MEL_BINS],
        );
    }
    subtract_column_mean(frames, WESPEAKER_MEL_BINS, &mut data);
    FeatureMatrix::from_row_major(frames, WESPEAKER_MEL_BINS, data)
}

pub fn row_l2_normalize(features: &FeatureMatrix) -> FeatureMatrix {
    let mut data = features.data().to_vec();
    for row in data.chunks_mut(features.bins()) {
        let norm = row.iter().map(|value| value * value).sum::<f32>().sqrt();
        let denom = if norm > ROW_NORM_EPSILON { norm } else { 1.0 };
        for value in row {
            *value /= denom;
        }
    }
    FeatureMatrix::from_row_major(features.frames(), features.bins(), data)
        .expect("row-l2 normalization preserves feature shape")
}

fn num_snipped_frames(samples: usize) -> usize {
    if samples < WESPEAKER_FRAME_LENGTH_SAMPLES {
        0
    } else {
        1 + (samples - WESPEAKER_FRAME_LENGTH_SAMPLES) / WESPEAKER_FRAME_SHIFT_SAMPLES
    }
}

fn povey_window() -> [f32; WESPEAKER_FRAME_LENGTH_SAMPLES] {
    let mut window = [0.0; WESPEAKER_FRAME_LENGTH_SAMPLES];
    let scale = 2.0_f64 * std::f64::consts::PI / (WESPEAKER_FRAME_LENGTH_SAMPLES as f64 - 1.0);
    for (index, value) in window.iter_mut().enumerate() {
        *value = (0.5_f64 - 0.5_f64 * (scale * index as f64).cos()).powf(0.85) as f32;
    }
    window
}

fn padded_frame(
    audio: &[f32],
    frame: usize,
    window: &[f32; WESPEAKER_FRAME_LENGTH_SAMPLES],
) -> [f32; WESPEAKER_FFT_SIZE] {
    let mut padded = [0.0; WESPEAKER_FFT_SIZE];
    let start = frame * WESPEAKER_FRAME_SHIFT_SAMPLES;
    for index in 0..WESPEAKER_FRAME_LENGTH_SAMPLES {
        padded[index] = audio[start + index] * AUDIO_SCALE;
    }
    remove_dc_offset(&mut padded[..WESPEAKER_FRAME_LENGTH_SAMPLES]);
    preemphasize(&mut padded[..WESPEAKER_FRAME_LENGTH_SAMPLES]);
    apply_window(&mut padded[..WESPEAKER_FRAME_LENGTH_SAMPLES], window);
    padded
}

fn remove_dc_offset(frame: &mut [f32]) {
    let mean = frame.iter().sum::<f32>() / frame.len() as f32;
    for sample in frame {
        *sample -= mean;
    }
}

fn preemphasize(frame: &mut [f32]) {
    for index in (1..frame.len()).rev() {
        frame[index] -= PREEMPH_COEFF * frame[index - 1];
    }
    frame[0] -= PREEMPH_COEFF * frame[0];
}

fn apply_window(frame: &mut [f32], window: &[f32; WESPEAKER_FRAME_LENGTH_SAMPLES]) {
    for (sample, weight) in frame.iter_mut().zip(window) {
        *sample *= *weight;
    }
}

fn power_spectrum_512(input: &mut [f32; WESPEAKER_FFT_SIZE]) -> [f32; WESPEAKER_FFT_SIZE / 2 + 1] {
    let mut real = *input;
    let mut imag = [0.0; WESPEAKER_FFT_SIZE];
    fft_512(&mut real, &mut imag);
    let mut power = [0.0; WESPEAKER_FFT_SIZE / 2 + 1];
    for index in 0..power.len() {
        power[index] = real[index] * real[index] + imag[index] * imag[index];
    }
    power
}

fn fft_512(real: &mut [f32; WESPEAKER_FFT_SIZE], imag: &mut [f32; WESPEAKER_FFT_SIZE]) {
    let mut j = 0;
    for i in 1..WESPEAKER_FFT_SIZE {
        let mut bit = WESPEAKER_FFT_SIZE >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }

    let mut len = 2;
    while len <= WESPEAKER_FFT_SIZE {
        let angle = -2.0 * PI / len as f32;
        let wlen_real = angle.cos();
        let wlen_imag = angle.sin();
        for start in (0..WESPEAKER_FFT_SIZE).step_by(len) {
            let mut w_real = 1.0;
            let mut w_imag = 0.0;
            for offset in 0..(len / 2) {
                let even = start + offset;
                let odd = even + len / 2;
                let odd_real = real[odd] * w_real - imag[odd] * w_imag;
                let odd_imag = real[odd] * w_imag + imag[odd] * w_real;
                real[odd] = real[even] - odd_real;
                imag[odd] = imag[even] - odd_imag;
                real[even] += odd_real;
                imag[even] += odd_imag;
                let next_real = w_real * wlen_real - w_imag * wlen_imag;
                w_imag = w_real * wlen_imag + w_imag * wlen_real;
                w_real = next_real;
            }
        }
        len *= 2;
    }
}

fn subtract_column_mean(frames: usize, bins: usize, data: &mut [f32]) {
    for bin in 0..bins {
        let mut sum = 0.0;
        for frame in 0..frames {
            sum += data[frame * bins + bin];
        }
        let mean = sum / frames as f32;
        for frame in 0..frames {
            data[frame * bins + bin] -= mean;
        }
    }
}

#[derive(Debug, Clone)]
struct MelBin {
    offset: usize,
    weights: Vec<f32>,
}

#[derive(Debug, Clone)]
struct MelBank {
    bins: Vec<MelBin>,
}

impl MelBank {
    fn production() -> Self {
        let mel_low = mel_scale(LOW_FREQ_HZ);
        let mel_high = mel_scale(HIGH_FREQ_HZ);
        let delta = (mel_high - mel_low) / (WESPEAKER_MEL_BINS as f32 + 1.0);
        let fft_bin_width = WESPEAKER_SAMPLE_RATE_HZ as f32 / WESPEAKER_FFT_SIZE as f32;
        let mut bins = Vec::with_capacity(WESPEAKER_MEL_BINS);
        for bin in 0..WESPEAKER_MEL_BINS {
            let left = mel_low + bin as f32 * delta;
            let center = mel_low + (bin as f32 + 1.0) * delta;
            let right = mel_low + (bin as f32 + 2.0) * delta;
            let mut dense = [0.0; WESPEAKER_FFT_SIZE / 2];
            let mut first = None;
            let mut last = 0;
            for (fft_bin, weight) in dense.iter_mut().enumerate() {
                let freq = fft_bin_width * fft_bin as f32;
                let mel = mel_scale(freq);
                if mel > left && mel < right {
                    *weight = if mel <= center {
                        (mel - left) / (center - left)
                    } else {
                        (right - mel) / (right - center)
                    };
                    first.get_or_insert(fft_bin);
                    last = fft_bin;
                }
            }
            let first = first.expect("production mel bin should have weights");
            bins.push(MelBin {
                offset: first,
                weights: dense[first..=last].to_vec(),
            });
        }
        MelBank { bins }
    }

    fn compute_log_energies(&self, power: &[f32; WESPEAKER_FFT_SIZE / 2 + 1], output: &mut [f32]) {
        for (bin, out) in self.bins.iter().zip(output.iter_mut()) {
            let mut energy = 0.0;
            for (index, weight) in bin.weights.iter().enumerate() {
                energy += weight * power[bin.offset + index];
            }
            *out = energy.max(f32::EPSILON).ln();
        }
    }

    #[cfg(test)]
    fn weight_columns(&self) -> usize {
        WESPEAKER_FFT_SIZE / 2
    }
}

fn mel_scale(freq: f32) -> f32 {
    1127.0 * (1.0 + freq / 700.0).ln()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        assert_matrix_within, assert_region_within, decode_waveform, filterbank_fixture,
        fixture_matrix, fixture_range, matrix_comparison_error, max_abs_diff,
    };

    const TOLERANCE: f32 = 1e-2;

    #[test]
    fn nyquist_power_bin_is_computed_but_not_consumed_by_mel_bank() {
        let bank = MelBank::production();
        assert_eq!(bank.weight_columns(), WESPEAKER_FFT_SIZE / 2);

        let mut power = [0.0; WESPEAKER_FFT_SIZE / 2 + 1];
        let mut baseline = [0.0; WESPEAKER_MEL_BINS];
        bank.compute_log_energies(&power, &mut baseline);

        power[WESPEAKER_FFT_SIZE / 2] = 1.0e12;
        let mut perturbed = [0.0; WESPEAKER_MEL_BINS];
        bank.compute_log_energies(&power, &mut perturbed);

        assert_eq!(baseline, perturbed);
    }

    #[test]
    fn povey_window_spans_400_samples_and_padded_tail_stays_zero() {
        let window = povey_window();
        assert_eq!(window.len(), WESPEAKER_FRAME_LENGTH_SAMPLES);

        let audio = vec![0.25; WESPEAKER_FRAME_LENGTH_SAMPLES];
        let padded = padded_frame(&audio, 0, &window);
        assert!(
            padded[WESPEAKER_FRAME_LENGTH_SAMPLES..]
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }

    #[test]
    fn filterbank_cmn_and_row_l2_match_fixture_regions_separately() {
        let fixture = filterbank_fixture();
        let audio = decode_waveform(&fixture);
        let cmn =
            compute_wespeaker_filterbank_cmn(&audio, WESPEAKER_SAMPLE_RATE_HZ).expect("features");
        let row_l2 = row_l2_normalize(&cmn);
        let expected_cmn = fixture_matrix(&fixture, "filterbank_cmn");
        let expected_row_l2 = fixture_matrix(&fixture, "row_l2_normalized");
        let near_rows = fixture_range(&fixture, "near_silent_rows");
        let broad_rows = fixture_range(&fixture, "broadband_rows");
        eprintln!(
            "filterbank_cmn_max_abs_diff={}",
            max_abs_diff(cmn.data(), &expected_cmn)
        );

        assert_matrix_within("filterbank_cmn", cmn.data(), &expected_cmn, TOLERANCE);
        assert_matrix_within(
            "row_l2_normalized",
            row_l2.data(),
            &expected_row_l2,
            TOLERANCE,
        );
        assert_region_within(
            "filterbank_cmn near_silent",
            cmn.data(),
            &expected_cmn,
            cmn.bins(),
            near_rows.clone(),
            TOLERANCE,
        );
        assert_region_within(
            "filterbank_cmn broadband",
            cmn.data(),
            &expected_cmn,
            cmn.bins(),
            broad_rows.clone(),
            TOLERANCE,
        );
        assert_region_within(
            "row_l2_normalized near_silent",
            row_l2.data(),
            &expected_row_l2,
            row_l2.bins(),
            near_rows,
            TOLERANCE,
        );
        assert_region_within(
            "row_l2_normalized broadband",
            row_l2.data(),
            &expected_row_l2,
            row_l2.bins(),
            broad_rows,
            TOLERANCE,
        );
    }

    #[test]
    fn fixture_comparison_fails_when_value_exceeds_tolerance() {
        let fixture = filterbank_fixture();
        let audio = decode_waveform(&fixture);
        let cmn =
            compute_wespeaker_filterbank_cmn(&audio, WESPEAKER_SAMPLE_RATE_HZ).expect("features");
        let mut expected = fixture_matrix(&fixture, "filterbank_cmn");
        expected[0] = cmn.data()[0] + TOLERANCE * 2.0;

        let result = matrix_comparison_error("filterbank_cmn", cmn.data(), &expected, TOLERANCE);
        assert!(result.is_some());
    }
}
