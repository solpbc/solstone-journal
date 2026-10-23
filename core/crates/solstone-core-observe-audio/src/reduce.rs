// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::SAMPLE_RATE;

pub type SpeechInterval = (f64, f64);

#[derive(Clone, Debug, PartialEq)]
pub struct VadResult {
    pub duration_s: f64,
    pub speech_duration_s: f64,
    pub has_speech: bool,
    pub speech_segments: Vec<SpeechInterval>,
    pub noisy_rms: Option<f64>,
    pub noisy_s: f64,
    pub loud_windows: usize,
    pub speech_loud_windows: usize,
}

/// A contiguous interval copied into reduced audio, including any retained silence.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeechSegment {
    pub original_start_s: f64,
    pub original_end_s: f64,
    pub reduced_start_s: f64,
    pub reduced_end_s: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioReduction {
    /// Retained intervals in copy order, covering the entire reduced timeline.
    pub segments: Vec<SpeechSegment>,
    pub original_duration_s: f64,
    pub reduced_duration_s: f64,
}

impl AudioReduction {
    /// Restores a sample/start timestamp; a cut boundary belongs to the following interval.
    pub fn restore_timestamp(&self, reduced_time_s: f64) -> f64 {
        self.restore_boundary(reduced_time_s, false)
    }

    /// Restores an exclusive end timestamp; a cut boundary ends the preceding interval.
    pub fn restore_end_timestamp(&self, reduced_time_s: f64) -> f64 {
        self.restore_boundary(reduced_time_s, true)
    }

    fn restore_boundary(&self, reduced_time_s: f64, end: bool) -> f64 {
        let Some(first) = self.segments.first() else {
            return reduced_time_s;
        };
        if reduced_time_s <= first.reduced_start_s {
            return first.original_start_s;
        }
        for segment in &self.segments {
            if reduced_time_s < segment.reduced_end_s
                || (end && reduced_time_s == segment.reduced_end_s)
            {
                return segment.original_start_s + (reduced_time_s - segment.reduced_start_s);
            }
        }
        self.segments
            .last()
            .expect("first segment exists")
            .original_end_s
    }
}

const MIN_GAP_TO_REDUCE_S: f64 = 2.0;
const GAP_BUFFER_S: f64 = 1.0;

/// Removes the middle of silence gaps longer than two seconds.
pub fn reduce_audio(audio: &[f32], vad: &VadResult) -> Option<(Vec<f32>, AudioReduction)> {
    if vad.speech_segments.is_empty() {
        return None;
    }

    let first_start_s = vad.speech_segments[0].0;
    let last_end_s = vad.speech_segments.last()?.1;
    let trailing_gap_s = vad.duration_s - last_end_s;
    let has_reducible_interior_gap = vad
        .speech_segments
        .windows(2)
        .any(|segments| segments[1].0 - segments[0].1 > MIN_GAP_TO_REDUCE_S);
    if !has_reducible_interior_gap
        && first_start_s <= MIN_GAP_TO_REDUCE_S
        && trailing_gap_s <= MIN_GAP_TO_REDUCE_S
    {
        return None;
    }

    let mut reduced_audio = Vec::new();
    let mut segments = Vec::with_capacity(vad.speech_segments.len());

    for (index, &(segment_start_s, segment_end_s)) in vad.speech_segments.iter().enumerate() {
        if index == 0 && first_start_s > MIN_GAP_TO_REDUCE_S {
            append_samples(
                audio,
                sample_index(segment_start_s - GAP_BUFFER_S),
                sample_index(segment_start_s),
                &mut reduced_audio,
                &mut segments,
            );
        } else if index == 0 {
            append_samples(
                audio,
                0,
                sample_index(segment_start_s),
                &mut reduced_audio,
                &mut segments,
            );
        }

        append_samples(
            audio,
            sample_index(segment_start_s),
            sample_index(segment_end_s),
            &mut reduced_audio,
            &mut segments,
        );

        if let Some(&(next_start_s, _)) = vad.speech_segments.get(index + 1) {
            let gap_s = next_start_s - segment_end_s;
            if gap_s > MIN_GAP_TO_REDUCE_S {
                append_samples(
                    audio,
                    sample_index(segment_end_s),
                    sample_index(segment_end_s + GAP_BUFFER_S),
                    &mut reduced_audio,
                    &mut segments,
                );
                append_samples(
                    audio,
                    sample_index(next_start_s - GAP_BUFFER_S),
                    sample_index(next_start_s),
                    &mut reduced_audio,
                    &mut segments,
                );
            } else {
                append_samples(
                    audio,
                    sample_index(segment_end_s),
                    sample_index(next_start_s),
                    &mut reduced_audio,
                    &mut segments,
                );
            }
        }
    }

    let trailing_end = if trailing_gap_s > MIN_GAP_TO_REDUCE_S {
        sample_index(last_end_s + GAP_BUFFER_S)
    } else {
        audio.len()
    };
    append_samples(
        audio,
        sample_index(last_end_s),
        trailing_end,
        &mut reduced_audio,
        &mut segments,
    );

    let reduced_duration_s = reduced_audio.len() as f64 / f64::from(SAMPLE_RATE);
    Some((
        reduced_audio,
        AudioReduction {
            segments,
            original_duration_s: audio.len() as f64 / f64::from(SAMPLE_RATE),
            reduced_duration_s,
        },
    ))
}

fn sample_index(seconds: f64) -> usize {
    (seconds * f64::from(SAMPLE_RATE)) as usize
}

fn append_samples(
    audio: &[f32],
    start: usize,
    end: usize,
    reduced_audio: &mut Vec<f32>,
    segments: &mut Vec<SpeechSegment>,
) {
    let start = start.min(audio.len());
    let end = end.min(audio.len());
    let samples = audio.get(start..end).unwrap_or_default();
    if samples.is_empty() {
        return;
    }
    let rate = f64::from(SAMPLE_RATE);
    let reduced_start_s = reduced_audio.len() as f64 / rate;
    reduced_audio.extend_from_slice(samples);
    let original_start_s = start as f64 / rate;
    let original_end_s = end as f64 / rate;
    let reduced_end_s = reduced_audio.len() as f64 / rate;
    if let Some(last) = segments.last_mut()
        && last.original_end_s == original_start_s
    {
        last.original_end_s = original_end_s;
        last.reduced_end_s = reduced_end_s;
    } else {
        segments.push(SpeechSegment {
            original_start_s,
            original_end_s,
            reduced_start_s,
            reduced_end_s,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{VadResult, reduce_audio};

    fn vad(duration_s: f64, speech_segments: Vec<(f64, f64)>) -> VadResult {
        VadResult {
            duration_s,
            speech_duration_s: speech_segments.iter().map(|(start, end)| end - start).sum(),
            has_speech: !speech_segments.is_empty(),
            speech_segments,
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        }
    }

    #[test]
    fn leaves_no_gap_and_exactly_two_second_gap_unreduced() {
        let audio = vec![0.0; 6 * 16_000];
        assert!(reduce_audio(&audio, &vad(6.0, vec![(0.0, 1.0), (3.0, 4.0)])).is_none());
        assert!(reduce_audio(&audio, &vad(4.0, vec![(0.0, 1.0), (2.999, 4.0)])).is_none());
        assert!(reduce_audio(&audio, &vad(6.0, vec![])).is_none());
    }

    #[test]
    fn trims_long_gap_with_one_second_buffers() {
        let audio: Vec<f32> = (0..(7 * 16_000)).map(|sample| sample as f32).collect();
        let (reduced, mapping) =
            reduce_audio(&audio, &vad(7.0, vec![(0.0, 1.0), (5.0, 6.0)])).expect("reduction");
        assert_eq!(reduced.len(), 5 * 16_000);
        assert_eq!(mapping.segments[1].reduced_start_s, 2.0);
        assert_eq!(mapping.reduced_duration_s, 5.0);
    }

    #[test]
    fn restores_buffers_speech_and_cut_boundaries_without_extrapolating() {
        let audio = vec![0.0; 12 * 16_000];
        let (_, mapping) = reduce_audio(&audio, &vad(12.0, vec![(3.0, 4.0), (8.0, 9.0)])).unwrap();
        assert_eq!(mapping.restore_timestamp(0.5), 2.5);
        assert_eq!(mapping.restore_timestamp(1.5), 3.5);
        assert_eq!(mapping.restore_timestamp(2.5), 4.5);
        assert_eq!(mapping.restore_timestamp(3.0), 7.0);
        assert_eq!(mapping.restore_end_timestamp(3.0), 5.0);
        assert_eq!(mapping.restore_timestamp(3.5), 7.5);
        assert_eq!(mapping.restore_timestamp(4.5), 8.5);
        assert_eq!(mapping.restore_timestamp(5.5), 9.5);
        for time in [-1.0, 0.0] {
            assert_eq!(mapping.restore_timestamp(time), 2.0);
            assert_eq!(mapping.restore_end_timestamp(time), 2.0);
        }
        for time in [6.0, 7.0] {
            assert_eq!(mapping.restore_timestamp(time), 10.0);
            assert_eq!(mapping.restore_end_timestamp(time), 10.0);
        }
    }

    #[test]
    fn mapping_ends_at_the_last_sample_actually_copied() {
        let audio = vec![0.0; 9 * 16_000 + 7];
        let (_, mapping) = reduce_audio(&audio, &vad(11.0, vec![(3.0, 4.0), (8.0, 10.0)])).unwrap();
        let original_end = audio.len() as f64 / 16_000.0;
        assert_eq!(mapping.original_duration_s, original_end);
        assert_eq!(mapping.reduced_duration_s, 5.0 + 7.0 / 16_000.0);
        assert_eq!(
            mapping.restore_timestamp(mapping.reduced_duration_s),
            original_end
        );
        assert_eq!(
            mapping.restore_end_timestamp(mapping.reduced_duration_s),
            original_end
        );
    }

    #[test]
    fn copied_samples_restore_to_their_original_coordinates() {
        let rate = 16_000.0;
        let audio = (0..(14 * 16_000 + 31))
            .map(|sample| sample as f32)
            .collect::<Vec<_>>();
        // Nonintegral sample coordinates expose duration accumulation and copy rounding.
        let speech = vec![
            (48_007.25 / rate, 64_011.75 / rate),
            (128_013.5 / rate, 144_017.25 / rate),
            (160_019.75 / rate, 176_023.5 / rate),
        ];
        let (reduced, mapping) = reduce_audio(&audio, &vad(audio.len() as f64 / rate, speech))
            .expect("leading, interior and trailing silence are reduced");
        // The existing policy retains one second before/after each cut, and the entire
        // short gap between the last two speech ranges. These are original sample ids.
        let retained = (32_007..80_011).chain(112_013..192_023).collect::<Vec<_>>();
        assert_eq!(reduced.len(), retained.len());
        for (index, (&sample, &original)) in reduced.iter().zip(&retained).enumerate() {
            assert_eq!(sample, audio[original], "copied sample {index}");
            for fraction in [0.0, 0.5] {
                let restored = mapping.restore_timestamp((index as f64 + fraction) / rate);
                let expected = (original as f64 + fraction) / rate;
                assert!(
                    (restored - expected).abs() < 1e-12,
                    "sample {index}+{fraction}: {restored} != {expected}"
                );
            }
            let end = mapping.restore_end_timestamp((index + 1) as f64 / rate);
            let expected_end = (original + 1) as f64 / rate;
            assert!((end - expected_end).abs() < 1e-12, "sample {index} end");
        }
    }

    #[test]
    fn retained_gap_buffers_are_not_stretched_over_the_removed_middle() {
        let audio = (0..(7 * 16_000))
            .map(|sample| sample as f32)
            .collect::<Vec<_>>();
        let (reduced, mapping) =
            reduce_audio(&audio, &vad(7.0, vec![(0.0, 1.0), (5.0, 6.0)])).unwrap();
        for (reduced_time, original_sample) in [(1.5, 24_000), (2.5, 72_000)] {
            assert_eq!(
                reduced[(reduced_time * 16_000.0) as usize],
                audio[original_sample]
            );
            assert_eq!(
                mapping.restore_timestamp(reduced_time),
                original_sample as f64 / 16_000.0
            );
        }
    }
}
