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

#[derive(Clone, Debug, PartialEq)]
pub struct SpeechSegment {
    pub original_start_s: f64,
    pub original_end_s: f64,
    pub reduced_start_s: f64,
    pub reduced_end_s: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioReduction {
    pub segments: Vec<SpeechSegment>,
    pub original_duration_s: f64,
    pub reduced_duration_s: f64,
}

impl AudioReduction {
    /// Restores a reduced-audio timestamp to the reference audio timeline.
    pub fn restore_timestamp(&self, reduced_time_s: f64) -> f64 {
        let Some(first) = self.segments.first() else {
            return reduced_time_s;
        };
        if reduced_time_s < first.reduced_start_s {
            return first.original_start_s - (first.reduced_start_s - reduced_time_s);
        }

        for (index, segment) in self.segments.iter().enumerate() {
            if segment.reduced_start_s <= reduced_time_s && reduced_time_s <= segment.reduced_end_s
            {
                return segment.original_start_s + (reduced_time_s - segment.reduced_start_s);
            }

            if let Some(next) = self.segments.get(index + 1)
                && segment.reduced_end_s < reduced_time_s
                && reduced_time_s < next.reduced_start_s
            {
                let reduced_gap = next.reduced_start_s - segment.reduced_end_s;
                let original_gap = next.original_start_s - segment.original_end_s;
                let progress = (reduced_time_s - segment.reduced_end_s) / reduced_gap;
                return segment.original_end_s + progress * original_gap;
            }
        }

        let last = self.segments.last().expect("first segment exists");
        last.original_end_s + (reduced_time_s - last.reduced_end_s)
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

    let mut chunks = Vec::new();
    let mut segments = Vec::with_capacity(vad.speech_segments.len());
    let mut current_reduced_time_s = 0.0;

    for (index, &(segment_start_s, segment_end_s)) in vad.speech_segments.iter().enumerate() {
        if index == 0 && first_start_s > MIN_GAP_TO_REDUCE_S {
            chunks.push(slice_seconds(
                audio,
                segment_start_s - GAP_BUFFER_S,
                segment_start_s,
            ));
            current_reduced_time_s = GAP_BUFFER_S;
        } else if index == 0 {
            chunks.push(slice_seconds(audio, 0.0, segment_start_s));
            current_reduced_time_s = segment_start_s;
        }

        chunks.push(slice_seconds(audio, segment_start_s, segment_end_s));
        let segment_duration_s = segment_end_s - segment_start_s;
        segments.push(SpeechSegment {
            original_start_s: segment_start_s,
            original_end_s: segment_end_s,
            reduced_start_s: current_reduced_time_s,
            reduced_end_s: current_reduced_time_s + segment_duration_s,
        });
        current_reduced_time_s += segment_duration_s;

        if let Some(&(next_start_s, _)) = vad.speech_segments.get(index + 1) {
            let gap_s = next_start_s - segment_end_s;
            if gap_s > MIN_GAP_TO_REDUCE_S {
                chunks.push(slice_seconds(
                    audio,
                    segment_end_s,
                    segment_end_s + GAP_BUFFER_S,
                ));
                chunks.push(slice_seconds(
                    audio,
                    next_start_s - GAP_BUFFER_S,
                    next_start_s,
                ));
                current_reduced_time_s += 2.0 * GAP_BUFFER_S;
            } else {
                chunks.push(slice_seconds(audio, segment_end_s, next_start_s));
                current_reduced_time_s += gap_s;
            }
        }
    }

    if trailing_gap_s > MIN_GAP_TO_REDUCE_S {
        chunks.push(slice_seconds(audio, last_end_s, last_end_s + GAP_BUFFER_S));
    } else {
        chunks.push(slice_from_seconds(audio, last_end_s));
    }

    let reduced_audio: Vec<f32> = chunks.into_iter().flatten().copied().collect();
    let reduced_duration_s = reduced_audio.len() as f64 / f64::from(SAMPLE_RATE);
    Some((
        reduced_audio,
        AudioReduction {
            segments,
            original_duration_s: vad.duration_s,
            reduced_duration_s,
        },
    ))
}

fn sample_index(seconds: f64) -> usize {
    (seconds * f64::from(SAMPLE_RATE)) as usize
}

fn slice_seconds(audio: &[f32], start_s: f64, end_s: f64) -> &[f32] {
    let start = sample_index(start_s).min(audio.len());
    let end = sample_index(end_s).min(audio.len());
    audio.get(start..end).unwrap_or_default()
}

fn slice_from_seconds(audio: &[f32], start_s: f64) -> &[f32] {
    &audio[sample_index(start_s).min(audio.len())..]
}

#[cfg(test)]
mod tests {
    use super::{AudioReduction, SpeechSegment, VadResult, reduce_audio};

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
    }

    #[test]
    fn trims_long_gap_with_one_second_buffers() {
        let audio: Vec<f32> = (0..(7 * 16_000)).map(|sample| sample as f32).collect();
        let (reduced, mapping) =
            reduce_audio(&audio, &vad(7.0, vec![(0.0, 1.0), (5.0, 6.0)])).expect("reduction");
        assert_eq!(reduced.len(), 5 * 16_000);
        assert_eq!(mapping.segments[1].reduced_start_s, 3.0);
        assert_eq!(mapping.reduced_duration_s, 5.0);
    }

    #[test]
    fn restores_all_four_timestamp_regions() {
        let mapping = AudioReduction {
            segments: vec![
                SpeechSegment {
                    original_start_s: 3.0,
                    original_end_s: 4.0,
                    reduced_start_s: 1.0,
                    reduced_end_s: 2.0,
                },
                SpeechSegment {
                    original_start_s: 8.0,
                    original_end_s: 9.0,
                    reduced_start_s: 4.0,
                    reduced_end_s: 5.0,
                },
            ],
            original_duration_s: 10.0,
            reduced_duration_s: 6.0,
        };
        assert_eq!(mapping.restore_timestamp(0.5), 2.5);
        assert_eq!(mapping.restore_timestamp(1.5), 3.5);
        assert_eq!(mapping.restore_timestamp(3.0), 6.0);
        assert_eq!(mapping.restore_timestamp(5.5), 9.5);
    }
}
