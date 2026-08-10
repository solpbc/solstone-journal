// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::reduce::{SpeechInterval, VadResult};

pub const MIN_NONSPEECH_SEGMENT_SECONDS: f64 = 0.5;
pub const DEFAULT_LOUD_WINDOW_SECONDS: f64 = 1.0;
pub const DEFAULT_LOUD_RMS_THRESHOLD: f64 = 0.01;

/// Inverts ordered speech segments into leading, interior, and trailing gaps.
pub fn get_nonspeech_segments(
    speech: &[SpeechInterval],
    total_duration_s: f64,
) -> Vec<SpeechInterval> {
    let Some(&(first_start_s, _)) = speech.first() else {
        return Vec::new();
    };
    let mut nonspeech = Vec::new();
    if first_start_s > 0.0 {
        nonspeech.push((0.0, first_start_s));
    }
    for windows in speech.windows(2) {
        let gap_start_s = windows[0].1;
        let gap_end_s = windows[1].0;
        if gap_end_s > gap_start_s {
            nonspeech.push((gap_start_s, gap_end_s));
        }
    }
    if let Some(&(_, last_end_s)) = speech.last()
        && last_end_s < total_duration_s
    {
        nonspeech.push((last_end_s, total_duration_s));
    }
    nonspeech
}

/// Returns the arithmetic mean of each qualifying non-speech interval's RMS.
pub fn compute_nonspeech_rms(
    audio: &[f32],
    speech: &[SpeechInterval],
    sample_rate: u32,
    min_segment_s: f64,
) -> (Option<f64>, f64) {
    let duration_s = audio.len() as f64 / f64::from(sample_rate);
    let segments = get_nonspeech_segments(speech, duration_s);
    let mut rms_values = Vec::new();
    let mut duration_used_s = 0.0;
    for (start_s, end_s) in segments {
        if end_s - start_s < min_segment_s {
            continue;
        }
        let start = sample_index(start_s, sample_rate).min(audio.len());
        let end = sample_index(end_s, sample_rate).min(audio.len());
        let samples = audio.get(start..end).unwrap_or_default();
        let mean_square = samples
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>()
            / samples.len() as f64;
        rms_values.push(mean_square.sqrt());
        duration_used_s += end_s - start_s;
    }
    if rms_values.is_empty() {
        (None, 0.0)
    } else {
        (
            Some(rms_values.iter().sum::<f64>() / rms_values.len() as f64),
            duration_used_s,
        )
    }
}

/// Counts complete loud windows and the loud windows with strict speech overlap.
pub fn compute_loud_speech_windows(
    audio: &[f32],
    speech: &[SpeechInterval],
    sample_rate: u32,
    window_s: f64,
    rms_threshold: f64,
) -> (usize, usize) {
    let window_samples = sample_index(window_s, sample_rate);
    if window_samples == 0 || audio.len() < window_samples {
        return (0, 0);
    }
    let mut loud_windows = 0;
    let mut speech_loud_windows = 0;
    for index in 0..audio.len() / window_samples {
        let start = index * window_samples;
        let end = start + window_samples;
        let samples = &audio[start..end];
        let rms = (samples
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt();
        if rms <= rms_threshold {
            continue;
        }
        loud_windows += 1;
        let window_start_s = index as f64 * window_s;
        let window_end_s = window_start_s + window_s;
        if speech
            .iter()
            .any(|&(start_s, end_s)| window_start_s < end_s && window_end_s > start_s)
        {
            speech_loud_windows += 1;
        }
    }
    (loud_windows, speech_loud_windows)
}

impl VadResult {
    pub fn is_noisy(&self, threshold: f64) -> bool {
        self.noisy_rms.is_some_and(|rms| rms > threshold)
    }

    pub fn loud_speech_ratio(&self) -> Option<f64> {
        (self.loud_windows > 0).then(|| self.speech_loud_windows as f64 / self.loud_windows as f64)
    }
}

fn sample_index(seconds: f64, sample_rate: u32) -> usize {
    (seconds * f64::from(sample_rate)) as usize
}

#[cfg(test)]
mod tests {
    use super::{compute_loud_speech_windows, compute_nonspeech_rms, get_nonspeech_segments};
    use crate::VadResult;

    #[test]
    fn no_speech_has_no_nonspeech_segments() {
        assert!(get_nonspeech_segments(&[], 3.0).is_empty());
    }

    #[test]
    fn computes_unweighted_rms_of_qualifying_segments() {
        let audio = [0.0, 0.0, 1.0, 1.0, 0.0, 0.0];
        let (rms, duration) = compute_nonspeech_rms(&audio, &[(1.0, 2.0)], 2, 0.5);
        assert_eq!(rms, Some(0.0));
        assert_eq!(duration, 2.0);
        let (rms, duration) = compute_nonspeech_rms(&audio, &[(1.0, 2.0)], 2, 1.1);
        assert_eq!(rms, None);
        assert_eq!(duration, 0.0);
    }

    #[test]
    fn loud_windows_ignore_partial_tail_and_overlap_strictly() {
        let audio = [0.02, 0.02, 0.02, 0.02, 0.02];
        assert_eq!(
            compute_loud_speech_windows(&audio, &[(1.0, 2.0)], 2, 1.0, 0.01),
            (2, 1)
        );
    }

    #[test]
    fn vad_predicates_are_strict_and_handle_no_loud_windows() {
        let vad = VadResult {
            duration_s: 1.0,
            speech_duration_s: 0.0,
            has_speech: false,
            speech_segments: Vec::new(),
            noisy_rms: Some(0.01),
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        };
        assert!(!vad.is_noisy(0.01));
        assert_eq!(vad.loud_speech_ratio(), None);
    }
}
