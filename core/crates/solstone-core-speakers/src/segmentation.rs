// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use crate::speaker_turns::{SpeakerTurnEvidence, SpeakerTurnRange};
use crate::{FeatureMatrix, SpeakerFeatureError};

pub const PYANNOTE_SAMPLE_RATE_HZ: u32 = 16_000;
pub const PYANNOTE_WINDOW_S: u32 = 10;
pub const PYANNOTE_OVERLAP_STRIDE_S: u32 = 5;
pub const PYANNOTE_DIARIZE_STRIDE_S: u32 = 2;
pub const PYANNOTE_FRAMES_PER_WINDOW: usize = 589;
pub const PYANNOTE_CLASS_COUNT: usize = 7;
pub const PYANNOTE_OVERLAP_CLASSES: [usize; 3] = [4, 5, 6];
pub const PYANNOTE_SINGLE_SPEAKER_CLASSES: [usize; 3] = [1, 2, 3];
pub const SLOT_ACTIVE_MIN_SHARE: f64 = 0.10;
pub const SPEAKER_EVIDENCE_MULTI_MIN: f64 = 0.05;
pub const SPEAKER_EVIDENCE_SINGLE_MAX: f64 = 0.05;
pub const DIARIZE_MIN_OVERLAP: f64 = 0.05;
pub const MAX_SAME_WINDOW_TURN_GAP_S: f64 = 1.0;

#[derive(Debug, Clone, PartialEq)]
pub struct PyannoteSegmentationPassResult {
    pub overlap_fraction: f64,
    pub avg_log_probs: FeatureMatrix,
    pub window_stats: Vec<SpeakerWindowStats>,
    pub speaker_turns: SpeakerTurnEvidence,
    /// Seconds on the input timeline where any single window's own frame argmax is an overlap
    /// class. Built before cross-window averaging, clamped to the unpadded duration, sorted and
    /// merged where ranges overlap or touch.
    pub overlap_ranges: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeakerWindowStats {
    pub speech_frames: usize,
    pub active_slot_count: usize,
    pub overlap_frames: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerEvidence {
    NoSpeech,
    Single,
    Multi,
}

impl SpeakerEvidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoSpeech => "none",
            Self::Single => "single",
            Self::Multi => "multi",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeakerEvidenceDecision {
    pub speaker_evidence: SpeakerEvidence,
    pub multi_window_fraction: f64,
    pub mean_window_overlap_share: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeakerSegmentationError<E> {
    UnsupportedSampleRate {
        expected: u32,
        actual: u32,
    },
    InvalidStride {
        stride_s: u32,
    },
    NonFiniteAudioSample {
        index: usize,
    },
    ShapeOverflow {
        frames: usize,
        classes: usize,
    },
    WindowLogProbShapeMismatch {
        window_index: usize,
        expected_frames: usize,
        expected_classes: usize,
        actual_frames: usize,
        actual_classes: usize,
        actual_len: usize,
    },
    Inference {
        window_index: usize,
        source: E,
    },
}

impl<E: fmt::Display> fmt::Display for SpeakerSegmentationError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSampleRate { expected, actual } => write!(
                formatter,
                "pyannote segmentation requires sample rate {expected}, got {actual}"
            ),
            Self::InvalidStride { stride_s } => {
                write!(
                    formatter,
                    "pyannote segmentation stride must be positive, got {stride_s}"
                )
            }
            Self::NonFiniteAudioSample { index } => {
                write!(formatter, "audio sample at index {index} is not finite")
            }
            Self::ShapeOverflow { frames, classes } => write!(
                formatter,
                "pyannote segmentation matrix shape overflow: frames={frames} classes={classes}"
            ),
            Self::WindowLogProbShapeMismatch {
                window_index,
                expected_frames,
                expected_classes,
                actual_frames,
                actual_classes,
                actual_len,
            } => write!(
                formatter,
                "pyannote window {window_index} log-prob shape mismatch: expected frames={expected_frames} classes={expected_classes}, got frames={actual_frames} classes={actual_classes} len={actual_len}"
            ),
            Self::Inference {
                window_index,
                source,
            } => write!(
                formatter,
                "pyannote window {window_index} inference failed: {source}"
            ),
        }
    }
}

impl<E: Error + 'static> Error for SpeakerSegmentationError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Inference { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn run_pyannote_segmentation_pass<E, F>(
    audio: &[f32],
    sample_rate_hz: u32,
    stride_s: u32,
    mut infer_window: F,
) -> Result<PyannoteSegmentationPassResult, SpeakerSegmentationError<E>>
where
    F: FnMut(usize, &[f32]) -> Result<FeatureMatrix, E>,
{
    if sample_rate_hz != PYANNOTE_SAMPLE_RATE_HZ {
        return Err(SpeakerSegmentationError::UnsupportedSampleRate {
            expected: PYANNOTE_SAMPLE_RATE_HZ,
            actual: sample_rate_hz,
        });
    }
    if stride_s == 0 {
        return Err(SpeakerSegmentationError::InvalidStride { stride_s });
    }
    if let Some((index, _sample)) = audio
        .iter()
        .enumerate()
        .find(|(_index, sample)| !sample.is_finite())
    {
        return Err(SpeakerSegmentationError::NonFiniteAudioSample { index });
    }

    let window_samples = PYANNOTE_WINDOW_S as usize * sample_rate_hz as usize;
    let stride_samples = stride_s as usize * sample_rate_hz as usize;
    let audio_padded: Cow<'_, [f32]> = if audio.len() < window_samples {
        let mut padded = Vec::with_capacity(window_samples);
        padded.extend_from_slice(audio);
        padded.resize(window_samples, 0.0);
        Cow::Owned(padded)
    } else {
        Cow::Borrowed(audio)
    };
    let starts = window_starts(audio_padded.len(), window_samples, stride_samples);
    let samples_per_frame = window_samples as f64 / PYANNOTE_FRAMES_PER_WINDOW as f64;
    let num_frames = (audio_padded.len() as f64 / samples_per_frame).ceil() as usize;
    let len = num_frames.checked_mul(PYANNOTE_CLASS_COUNT).ok_or(
        SpeakerSegmentationError::ShapeOverflow {
            frames: num_frames,
            classes: PYANNOTE_CLASS_COUNT,
        },
    )?;
    let audio_duration_s = audio.len() as f64 / f64::from(sample_rate_hz);
    let mut turn_ranges: Vec<SpeakerTurnRange> = Vec::new();
    let mut overlap_ranges: Vec<(f64, f64)> = Vec::new();
    let frame_duration_s = samples_per_frame / f64::from(sample_rate_hz);
    let mut accum = vec![0.0_f64; len];
    let mut counts = vec![0_usize; num_frames];
    let mut window_stats = Vec::with_capacity(starts.len());

    for (window_index, start_sample) in starts.iter().copied().enumerate() {
        let chunk = &audio_padded[start_sample..start_sample + window_samples];
        let log_probs = infer_window(window_index, chunk).map_err(|source| {
            SpeakerSegmentationError::Inference {
                window_index,
                source,
            }
        })?;
        validate_window_log_probs(window_index, &log_probs)?;

        let frame_log_probs = crate::diarization::FrameLogProbs::from_row_major(
            log_probs.data(),
            PYANNOTE_CLASS_COUNT,
        )
        .expect("validated window log-probs data length is multiple of class count");
        let window_intervals = crate::diarization::find_intervals(frame_log_probs, window_samples);
        let window_offset_s = start_sample as f64 / f64::from(sample_rate_hz);
        for pair in window_intervals.windows(2) {
            let prev = &pair[0];
            let next = &pair[1];
            if prev.local_class != next.local_class {
                let gap_s = next.start_s - prev.end_s;
                if gap_s <= MAX_SAME_WINDOW_TURN_GAP_S {
                    let shifted_start_s = prev.end_s + window_offset_s;
                    let shifted_end_s = next.start_s + window_offset_s;
                    if shifted_end_s < 0.0 || shifted_start_s > audio_duration_s {
                        continue;
                    }
                    let clamped_start_s = shifted_start_s.max(0.0);
                    let clamped_end_s = shifted_end_s.min(audio_duration_s);
                    turn_ranges.push(SpeakerTurnRange {
                        start_s: clamped_start_s,
                        end_s: clamped_end_s,
                    });
                }
            }
        }

        for local_frame in 0..log_probs.frames() {
            let row = log_probs
                .row(local_frame)
                .expect("validated window frame index is in bounds");
            if PYANNOTE_OVERLAP_CLASSES.contains(&argmax(row)) {
                let start_s = window_offset_s + local_frame as f64 * frame_duration_s;
                let end_s = (start_s + frame_duration_s).min(audio_duration_s);
                if start_s < end_s {
                    overlap_ranges.push((start_s, end_s));
                }
            }
        }

        let frame_start = frame_start_for_sample(start_sample, samples_per_frame);
        let frame_end = frame_start + log_probs.frames();
        // A validated window has exactly the expected frame count; with starts at or before the
        // final full window, round_ties_even(x) <= ceil(x) keeps frame_end within num_frames.
        debug_assert!(frame_end <= num_frames);
        window_stats.push(
            compute_speaker_window_stats(&log_probs)
                .expect("validated pyannote log-probs have the expected class count"),
        );

        for local_frame in 0..log_probs.frames() {
            let global_frame = frame_start + local_frame;
            let source_start = local_frame * PYANNOTE_CLASS_COUNT;
            let target_start = global_frame * PYANNOTE_CLASS_COUNT;
            for class in 0..PYANNOTE_CLASS_COUNT {
                accum[target_start + class] += log_probs.data()[source_start + class] as f64;
            }
            counts[global_frame] += 1;
        }
    }

    turn_ranges.sort_by(|a, b| {
        a.start_s
            .total_cmp(&b.start_s)
            .then_with(|| a.end_s.total_cmp(&b.end_s))
    });
    turn_ranges.dedup_by(|b, a| {
        a.start_s.total_cmp(&b.start_s) == std::cmp::Ordering::Equal
            && a.end_s.total_cmp(&b.end_s) == std::cmp::Ordering::Equal
    });
    let speaker_turns = SpeakerTurnEvidence::new(audio_duration_s, turn_ranges);
    let overlap_ranges = merge_ranges(overlap_ranges);

    let avg_log_probs =
        average_accumulated_log_probs(&accum, &counts, num_frames, PYANNOTE_CLASS_COUNT)
            .expect("accumulator shape is constructed from checked dimensions");
    let overlap_fraction = compute_overlap_fraction(&avg_log_probs);
    Ok(PyannoteSegmentationPassResult {
        overlap_fraction,
        avg_log_probs,
        window_stats,
        speaker_turns,
        overlap_ranges,
    })
}

/// Sort and merge ranges that overlap or touch.
fn merge_ranges(mut ranges: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

pub fn compute_speaker_window_stats(
    log_probs: &FeatureMatrix,
) -> Result<SpeakerWindowStats, SpeakerFeatureError> {
    if log_probs.bins() != PYANNOTE_CLASS_COUNT {
        return Err(SpeakerFeatureError::ShapeMismatch {
            frames: log_probs.frames(),
            bins: PYANNOTE_CLASS_COUNT,
            len: log_probs.data().len(),
        });
    }

    let mut counts = [0_usize; PYANNOTE_CLASS_COUNT];
    for frame in 0..log_probs.frames() {
        let row = log_probs.row(frame).expect("frame index is in bounds");
        counts[argmax(row)] += 1;
    }
    let speech_frames = counts[1..].iter().sum();
    if speech_frames == 0 {
        return Ok(SpeakerWindowStats {
            speech_frames: 0,
            active_slot_count: 0,
            overlap_frames: 0,
        });
    }
    let active_slot_count = PYANNOTE_SINGLE_SPEAKER_CLASSES
        .iter()
        .filter(|class| (counts[**class] as f64 / speech_frames as f64) >= SLOT_ACTIVE_MIN_SHARE)
        .count();
    let overlap_frames = PYANNOTE_OVERLAP_CLASSES
        .iter()
        .map(|class| counts[*class])
        .sum();
    Ok(SpeakerWindowStats {
        speech_frames,
        active_slot_count,
        overlap_frames,
    })
}

pub fn decide_speaker_evidence(
    overlap_fraction: f64,
    window_stats: &[SpeakerWindowStats],
) -> SpeakerEvidenceDecision {
    let speech_windows: Vec<&SpeakerWindowStats> = window_stats
        .iter()
        .filter(|row| row.speech_frames > 0)
        .collect();
    if speech_windows.is_empty() {
        return SpeakerEvidenceDecision {
            speaker_evidence: SpeakerEvidence::NoSpeech,
            multi_window_fraction: 0.0,
            mean_window_overlap_share: 0.0,
        };
    }

    let multi_window_count = speech_windows
        .iter()
        .filter(|row| row.active_slot_count > 1)
        .count();
    let multi_window_fraction = multi_window_count as f64 / speech_windows.len() as f64;
    let mean_window_overlap_share = speech_windows
        .iter()
        .map(|row| row.overlap_frames as f64 / row.speech_frames as f64)
        .sum::<f64>()
        / speech_windows.len() as f64;

    let speaker_evidence = if multi_window_fraction >= SPEAKER_EVIDENCE_MULTI_MIN
        || overlap_fraction >= DIARIZE_MIN_OVERLAP
    {
        SpeakerEvidence::Multi
    } else if multi_window_fraction < SPEAKER_EVIDENCE_SINGLE_MAX
        && mean_window_overlap_share < DIARIZE_MIN_OVERLAP
    {
        SpeakerEvidence::Single
    } else {
        SpeakerEvidence::Multi
    };

    SpeakerEvidenceDecision {
        speaker_evidence,
        multi_window_fraction,
        mean_window_overlap_share,
    }
}

fn window_starts(len_padded: usize, window_samples: usize, stride_samples: usize) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut start = 0_usize;
    while start + window_samples <= len_padded {
        starts.push(start);
        start = match start.checked_add(stride_samples) {
            Some(next) => next,
            None => break,
        };
    }
    let final_start = len_padded.saturating_sub(window_samples);
    if starts.last().copied() != Some(final_start) {
        starts.push(final_start);
    }
    starts
}

fn frame_start_for_sample(start_sample: usize, samples_per_frame: f64) -> usize {
    (start_sample as f64 / samples_per_frame).round_ties_even() as usize
}

fn validate_window_log_probs<E>(
    window_index: usize,
    log_probs: &FeatureMatrix,
) -> Result<(), SpeakerSegmentationError<E>> {
    if log_probs.frames() != PYANNOTE_FRAMES_PER_WINDOW || log_probs.bins() != PYANNOTE_CLASS_COUNT
    {
        return Err(SpeakerSegmentationError::WindowLogProbShapeMismatch {
            window_index,
            expected_frames: PYANNOTE_FRAMES_PER_WINDOW,
            expected_classes: PYANNOTE_CLASS_COUNT,
            actual_frames: log_probs.frames(),
            actual_classes: log_probs.bins(),
            actual_len: log_probs.data().len(),
        });
    }
    Ok(())
}

fn average_accumulated_log_probs(
    accum: &[f64],
    counts: &[usize],
    frames: usize,
    classes: usize,
) -> Result<FeatureMatrix, SpeakerFeatureError> {
    let expected = frames
        .checked_mul(classes)
        .ok_or(SpeakerFeatureError::ShapeOverflow {
            frames,
            bins: classes,
        })?;
    if accum.len() != expected {
        return Err(SpeakerFeatureError::ShapeMismatch {
            frames,
            bins: classes,
            len: accum.len(),
        });
    }
    let mut data = Vec::with_capacity(expected);
    for frame in 0..frames {
        let count = counts.get(frame).copied().unwrap_or(0).max(1) as f64;
        for class in 0..classes {
            data.push((accum[frame * classes + class] / count) as f32);
        }
    }
    FeatureMatrix::from_row_major(frames, classes, data)
}

fn compute_overlap_fraction(avg_log_probs: &FeatureMatrix) -> f64 {
    let mut speech_count = 0_usize;
    let mut overlap_count = 0_usize;
    for frame in 0..avg_log_probs.frames() {
        let row = avg_log_probs.row(frame).expect("frame index is in bounds");
        let class = argmax(row);
        if class >= 1 {
            speech_count += 1;
            if PYANNOTE_OVERLAP_CLASSES.contains(&class) {
                overlap_count += 1;
            }
        }
    }
    if speech_count == 0 {
        0.0
    } else {
        overlap_count as f64 / speech_count as f64
    }
}

fn argmax(row: &[f32]) -> usize {
    let mut max_index = 0;
    let mut max_value = row[0];
    for (index, value) in row.iter().enumerate().skip(1) {
        if *value > max_value {
            max_index = index;
            max_value = *value;
        }
    }
    max_index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{matrix_comparison_error, stage_fixture};
    use serde_json::Value;
    use std::convert::Infallible;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct StubError;

    impl fmt::Display for StubError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("stub error")
        }
    }

    impl Error for StubError {}

    #[test]
    fn segmentation_constants_match_fixture_identity() {
        let fixture = stage_fixture();
        let constants = &fixture["identity"]["source_constants"];

        assert_eq!(
            PYANNOTE_SAMPLE_RATE_HZ as u64,
            constants["diarize"]["SAMPLE_RATE"]
                .as_u64()
                .expect("sample rate")
        );
        assert_eq!(
            PYANNOTE_WINDOW_S as u64,
            constants["overlap"]["WINDOW_S"]
                .as_u64()
                .expect("window seconds")
        );
        assert_eq!(
            PYANNOTE_OVERLAP_STRIDE_S as u64,
            constants["overlap"]["STRIDE_S"]
                .as_u64()
                .expect("overlap stride")
        );
        assert_eq!(
            PYANNOTE_DIARIZE_STRIDE_S as u64,
            constants["overlap"]["_DIARIZE_STRIDE_S"]
                .as_u64()
                .expect("diarize stride")
        );
        assert_eq!(
            PYANNOTE_FRAMES_PER_WINDOW as u64,
            constants["overlap"]["FRAMES_PER_WINDOW"]
                .as_u64()
                .expect("frames per window")
        );
        assert_eq!(
            usize_array(&constants["overlap"]["OVERLAP_CLASSES"]),
            PYANNOTE_OVERLAP_CLASSES
        );
        assert_eq!(
            usize_array(&constants["diarize"]["SINGLE_SPEAKER_CLASSES"]),
            PYANNOTE_SINGLE_SPEAKER_CLASSES
        );
        assert_eq!(
            SLOT_ACTIVE_MIN_SHARE,
            constants["encoder_config"]["SLOT_ACTIVE_MIN_SHARE"]
                .as_f64()
                .expect("slot active min share")
        );
        assert_eq!(
            SPEAKER_EVIDENCE_MULTI_MIN,
            constants["encoder_config"]["SPEAKER_EVIDENCE_MULTI_MIN"]
                .as_f64()
                .expect("speaker evidence multi min")
        );
        assert_eq!(
            SPEAKER_EVIDENCE_SINGLE_MAX,
            constants["encoder_config"]["SPEAKER_EVIDENCE_SINGLE_MAX"]
                .as_f64()
                .expect("speaker evidence single max")
        );
        assert_eq!(
            DIARIZE_MIN_OVERLAP,
            constants["encoder_config"]["DIARIZE_MIN_OVERLAP"]
                .as_f64()
                .expect("diarize min overlap")
        );
    }

    #[test]
    fn speaker_evidence_wire_strings_match_fixture_decisions() {
        let fixture = stage_fixture();
        let cases = &fixture["speaker_evidence"];

        assert_eq!(
            SpeakerEvidence::NoSpeech.as_str(),
            cases["none"]["decision"]["speaker_evidence"]
                .as_str()
                .expect("none decision")
        );
        assert_eq!(
            SpeakerEvidence::Single.as_str(),
            cases["single"]["decision"]["speaker_evidence"]
                .as_str()
                .expect("single decision")
        );
        assert_eq!(
            SpeakerEvidence::Multi.as_str(),
            cases["multi_by_active_slots"]["decision"]["speaker_evidence"]
                .as_str()
                .expect("multi decision")
        );
    }

    #[test]
    fn start_sequence_stride_aligned_buffer_length() {
        let audio = indexed_audio(20 * PYANNOTE_SAMPLE_RATE_HZ as usize);
        let mut starts = Vec::new();

        run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_OVERLAP_STRIDE_S,
            |_, window| {
                starts.push(window[0] as usize);
                Ok::<FeatureMatrix, Infallible>(class_matrix(&vec![0; PYANNOTE_FRAMES_PER_WINDOW]))
            },
        )
        .expect("segmentation pass");

        assert_eq!(starts, vec![0, 80_000, 160_000]);
    }

    #[test]
    fn start_sequence_non_stride_aligned_buffer_appends_final_window() {
        let audio = indexed_audio(13 * PYANNOTE_SAMPLE_RATE_HZ as usize);
        let mut starts = Vec::new();

        run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_OVERLAP_STRIDE_S,
            |_, window| {
                starts.push(window[0] as usize);
                Ok::<FeatureMatrix, Infallible>(class_matrix(&vec![0; PYANNOTE_FRAMES_PER_WINDOW]))
            },
        )
        .expect("segmentation pass");

        assert_eq!(starts, vec![0, 48_000]);
    }

    #[test]
    fn buffer_shorter_than_one_window_is_zero_padded() {
        let audio = vec![0.25; 3 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let mut inspected = false;

        run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_OVERLAP_STRIDE_S,
            |_, window| {
                inspected = true;
                assert_eq!(
                    window.len(),
                    PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize
                );
                assert!(window[..audio.len()].iter().all(|sample| *sample == 0.25));
                assert!(window[audio.len()..].iter().all(|sample| *sample == 0.0));
                Ok::<FeatureMatrix, Infallible>(class_matrix(&vec![0; PYANNOTE_FRAMES_PER_WINDOW]))
            },
        )
        .expect("segmentation pass");

        assert!(inspected);
    }

    #[test]
    fn frame_start_uses_round_ties_even_for_stride5_294_5_tie() {
        let window_samples = PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize;
        let samples_per_frame = window_samples as f64 / PYANNOTE_FRAMES_PER_WINDOW as f64;
        let start_sample = PYANNOTE_OVERLAP_STRIDE_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize;
        let ratio = start_sample as f64 / samples_per_frame;

        assert_eq!(start_sample, 80_000);
        assert_eq!(ratio, 294.5);
        // Python round() uses banker's rounding, so this must be ties-to-even.
        // Rust f64::round() is half-away-from-zero and would shift this window to 295.
        assert_eq!(ratio.round() as usize, 295);
        assert_eq!(frame_start_for_sample(start_sample, samples_per_frame), 294);
    }

    #[test]
    fn segmentation_accumulates_f64_floors_counts_then_narrows_to_f32() {
        let (actual, expected) = accumulated_mean_case();

        assert_eq!(actual, expected);

        let floored = average_accumulated_log_probs(&[5.0, 7.0], &[0], 1, 2).expect("average");
        assert_eq!(floored.data(), &[5.0, 7.0]);

        let expected_matrix = vec![expected];
        let actual_matrix = vec![actual];
        assert!(
            matrix_comparison_error("accumulated_mean", &actual_matrix, &expected_matrix, 0.0)
                .is_none()
        );
    }

    #[test]
    fn overlap_fraction_uses_averaged_f32_argmax_not_per_window_argmax() {
        let audio = vec![0.0; 14 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |window_index, _window| {
                let mut matrix = zero_matrix();
                match window_index {
                    0 => set_log_prob(&mut matrix, 236, 4, 10.0),
                    1 => set_log_prob(&mut matrix, 118, 1, 10.0),
                    2 => set_log_prob(&mut matrix, 0, 1, 10.0),
                    _ => {}
                }
                Ok::<FeatureMatrix, Infallible>(matrix)
            },
        )
        .expect("segmentation pass");
        let row = result.avg_log_probs.row(236).expect("frame 236");

        assert!(row[1] > row[4]);
        assert_eq!(result.overlap_fraction, 0.0);
    }

    #[test]
    fn no_speech_buffer_returns_zero_overlap_fraction() {
        let audio = vec![0.0; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_OVERLAP_STRIDE_S,
            |_, _window| {
                Ok::<FeatureMatrix, Infallible>(class_matrix(&vec![0; PYANNOTE_FRAMES_PER_WINDOW]))
            },
        )
        .expect("segmentation pass");

        assert_eq!(result.overlap_fraction, 0.0);
    }

    #[test]
    fn validated_window_frame_count_keeps_every_pass_window_within_num_frames() {
        let cases = [
            ("sub_window_tight", 137, PYANNOTE_OVERLAP_STRIDE_S),
            (
                "stride_aligned_overlap",
                20 * PYANNOTE_SAMPLE_RATE_HZ as usize,
                PYANNOTE_OVERLAP_STRIDE_S,
            ),
            (
                "non_stride_aligned_overlap",
                13 * PYANNOTE_SAMPLE_RATE_HZ as usize,
                PYANNOTE_OVERLAP_STRIDE_S,
            ),
            (
                "diarize_stride",
                14 * PYANNOTE_SAMPLE_RATE_HZ as usize,
                PYANNOTE_DIARIZE_STRIDE_S,
            ),
        ];
        let window_samples = PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize;
        let samples_per_frame = window_samples as f64 / PYANNOTE_FRAMES_PER_WINDOW as f64;
        let mut saw_tight_boundary = false;

        for (name, samples, stride_s) in cases {
            let audio = indexed_audio(samples);
            let len_padded = samples.max(window_samples);
            let num_frames = (len_padded as f64 / samples_per_frame).ceil() as usize;
            let mut inspected_windows = 0_usize;

            run_pyannote_segmentation_pass(
                &audio,
                PYANNOTE_SAMPLE_RATE_HZ,
                stride_s,
                |_, window| {
                    let start_sample = window[0] as usize;
                    let frame_start = frame_start_for_sample(start_sample, samples_per_frame);
                    let frame_end = frame_start + PYANNOTE_FRAMES_PER_WINDOW;
                    assert!(
                        frame_end <= num_frames,
                        "{name}: frame_end={frame_end} num_frames={num_frames}"
                    );
                    saw_tight_boundary |= frame_end == num_frames;
                    inspected_windows += 1;
                    Ok::<FeatureMatrix, Infallible>(class_matrix(&vec![
                        0;
                        PYANNOTE_FRAMES_PER_WINDOW
                    ]))
                },
            )
            .expect("segmentation pass");

            assert!(inspected_windows > 0, "{name}");
        }

        assert!(saw_tight_boundary);
    }

    #[test]
    fn window_stats_for_all_fixture_cases_match_committed_windows() {
        let fixture = stage_fixture();
        let evidence = &fixture["speaker_evidence"];
        let ambiguity_overlap_frames =
            (DIARIZE_MIN_OVERLAP * PYANNOTE_FRAMES_PER_WINDOW as f64).ceil() as usize;

        let cases = [
            ("none", vec![0; PYANNOTE_FRAMES_PER_WINDOW]),
            ("single", vec![1; PYANNOTE_FRAMES_PER_WINDOW]),
            (
                "multi_by_active_slots",
                [vec![1; PYANNOTE_FRAMES_PER_WINDOW / 2], {
                    vec![2; PYANNOTE_FRAMES_PER_WINDOW - PYANNOTE_FRAMES_PER_WINDOW / 2]
                }]
                .concat(),
            ),
            (
                "else_branch_overlap_ambiguity",
                [
                    vec![1; PYANNOTE_FRAMES_PER_WINDOW - ambiguity_overlap_frames],
                    vec![4; ambiguity_overlap_frames],
                ]
                .concat(),
            ),
        ];

        for (name, classes) in cases {
            let stats = compute_speaker_window_stats(&class_matrix(&classes)).expect("stats");
            let expected = &evidence[name]["windows"][0];

            assert_eq!(
                stats.speech_frames as u64,
                expected["speech_frames"].as_u64().expect("speech frames"),
                "{name}"
            );
            assert_eq!(
                stats.active_slot_count as u64,
                expected["active_slot_count"]
                    .as_u64()
                    .expect("active slot count"),
                "{name}"
            );
            assert_eq!(
                stats.overlap_frames as u64,
                expected["overlap_frames"].as_u64().expect("overlap frames"),
                "{name}"
            );
        }
    }

    #[test]
    fn zero_speech_window_yields_zero_statistics() {
        assert_eq!(
            compute_speaker_window_stats(&class_matrix(&vec![0; PYANNOTE_FRAMES_PER_WINDOW]))
                .expect("stats"),
            SpeakerWindowStats {
                speech_frames: 0,
                active_slot_count: 0,
                overlap_frames: 0,
            }
        );
    }

    #[test]
    fn decisions_for_all_fixture_cases_match_committed_values() {
        let fixture = stage_fixture();
        let tolerance = fixture["comparison"]["cluster_score_abs_tolerance"]
            .as_f64()
            .expect("cluster score tolerance");
        let evidence = &fixture["speaker_evidence"];

        for name in [
            "none",
            "single",
            "multi_by_active_slots",
            "else_branch_overlap_ambiguity",
        ] {
            let case = &evidence[name];
            let decision = decide_speaker_evidence(
                case["overlap_fraction"].as_f64().expect("overlap fraction"),
                &[fixture_window_stats(case)],
            );
            let expected = &case["decision"];

            assert_eq!(
                decision.speaker_evidence.as_str(),
                expected["speaker_evidence"].as_str().expect("evidence"),
                "{name}"
            );
            assert_within(
                decision.multi_window_fraction,
                expected["multi_window_fraction"]
                    .as_f64()
                    .expect("multi fraction"),
                tolerance,
                name,
            );
            assert_within(
                decision.mean_window_overlap_share,
                expected["mean_window_overlap_share"]
                    .as_f64()
                    .expect("mean overlap"),
                tolerance,
                name,
            );
        }
    }

    #[test]
    fn mixed_zero_speech_and_speech_windows_ignore_zero_speech_denominators() {
        let decision = decide_speaker_evidence(
            0.0,
            &[
                SpeakerWindowStats {
                    speech_frames: 0,
                    active_slot_count: 0,
                    overlap_frames: 0,
                },
                SpeakerWindowStats {
                    speech_frames: 100,
                    active_slot_count: 1,
                    overlap_frames: 4,
                },
                SpeakerWindowStats {
                    speech_frames: 0,
                    active_slot_count: 0,
                    overlap_frames: 0,
                },
            ],
        );

        assert_eq!(decision.speaker_evidence, SpeakerEvidence::Single);
        assert_eq!(decision.multi_window_fraction, 0.0);
        assert_eq!(decision.mean_window_overlap_share, 0.04);
    }

    #[test]
    fn malformed_window_logprob_shape_reports_specific_error_variant() {
        let audio = vec![0.0; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let error = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_OVERLAP_STRIDE_S,
            |_, _window| {
                Ok::<FeatureMatrix, StubError>(
                    FeatureMatrix::from_row_major(
                        PYANNOTE_FRAMES_PER_WINDOW - 1,
                        PYANNOTE_CLASS_COUNT,
                        vec![0.0; (PYANNOTE_FRAMES_PER_WINDOW - 1) * PYANNOTE_CLASS_COUNT],
                    )
                    .expect("stub matrix"),
                )
            },
        )
        .unwrap_err();

        assert_eq!(
            error,
            SpeakerSegmentationError::WindowLogProbShapeMismatch {
                window_index: 0,
                expected_frames: PYANNOTE_FRAMES_PER_WINDOW,
                expected_classes: PYANNOTE_CLASS_COUNT,
                actual_frames: PYANNOTE_FRAMES_PER_WINDOW - 1,
                actual_classes: PYANNOTE_CLASS_COUNT,
                actual_len: (PYANNOTE_FRAMES_PER_WINDOW - 1) * PYANNOTE_CLASS_COUNT,
            }
        );
    }

    #[test]
    fn segmentation_comparison_fails_when_value_exceeds_tolerance() {
        let (actual_value, expected_value) = accumulated_mean_case();
        let actual = vec![actual_value];
        let mut expected = vec![expected_value];
        let tolerance = 0.0_f32;
        expected[0] = f32::from_bits(expected[0].to_bits() + 1);

        assert!(matrix_comparison_error("segmentation", &actual, &expected, tolerance).is_some());
    }

    fn indexed_audio(samples: usize) -> Vec<f32> {
        (0..samples).map(|index| index as f32).collect()
    }

    fn accumulated_mean_case() -> (f32, f32) {
        let audio = vec![0.0; 14 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |window_index, _window| {
                let mut matrix = zero_matrix();
                match window_index {
                    0 => set_log_prob(&mut matrix, 236, 1, 100_000_000.0),
                    1 => set_log_prob(&mut matrix, 118, 1, 1.0),
                    2 => set_log_prob(&mut matrix, 0, 1, -100_000_000.0),
                    _ => {}
                }
                Ok::<FeatureMatrix, Infallible>(matrix)
            },
        )
        .expect("segmentation pass");
        let actual = result.avg_log_probs.row(236).expect("frame 236")[1];
        let expected = (1.0_f64 / 3.0) as f32;
        (actual, expected)
    }

    fn frame_s() -> f64 {
        PYANNOTE_WINDOW_S as f64 / PYANNOTE_FRAMES_PER_WINDOW as f64
    }

    fn assert_ranges(actual: &[(f64, f64)], expected: &[(f64, f64)]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!(
                (a.0 - e.0).abs() < 1e-9 && (a.1 - e.1).abs() < 1e-9,
                "{actual:?} vs {expected:?}"
            );
        }
    }

    #[test]
    fn overlap_ranges_come_from_each_window_before_averaging() {
        let audio = vec![0.0; 14 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |window_index, _window| {
                let mut matrix = zero_matrix();
                match window_index {
                    0 => set_log_prob(&mut matrix, 236, 4, 10.0),
                    1 => set_log_prob(&mut matrix, 118, 1, 10.0),
                    2 => set_log_prob(&mut matrix, 0, 1, 10.0),
                    _ => {}
                }
                Ok::<FeatureMatrix, Infallible>(matrix)
            },
        )
        .expect("segmentation pass");

        // The averaged frame is single-speaker, but window 0's own frame is overlap.
        assert_eq!(result.overlap_fraction, 0.0);
        assert_ranges(
            &result.overlap_ranges,
            &[(236.0 * frame_s(), 237.0 * frame_s())],
        );
    }

    #[test]
    fn averaged_overlap_alone_emits_no_overlap_range() {
        let audio = vec![0.0; 14 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |window_index, _window| {
                let mut matrix = zero_matrix();
                match window_index {
                    0 => {
                        set_log_prob(&mut matrix, 236, 4, 1.0);
                        set_log_prob(&mut matrix, 236, 1, 1.5);
                    }
                    1 => {
                        set_log_prob(&mut matrix, 118, 4, 1.0);
                        set_log_prob(&mut matrix, 118, 2, 1.5);
                    }
                    _ => {}
                }
                Ok::<FeatureMatrix, Infallible>(matrix)
            },
        )
        .expect("segmentation pass");

        // The averaged frame's argmax is the overlap class; no single window's argmax is.
        let row = result.avg_log_probs.row(236).expect("frame 236");
        assert!(row[4] > row[1] && row[4] > row[2] && row[4] > row[0]);
        assert!(result.overlap_fraction > 0.0);
        assert!(result.overlap_ranges.is_empty());
    }

    #[test]
    fn overlap_ranges_use_the_shifted_final_window_and_clamp_to_duration() {
        // 13 s pads to windows at 0 s, 2 s and a shifted final window at 3 s.
        let audio = vec![0.0; 13 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |window_index, _window| {
                let mut matrix = zero_matrix();
                if window_index == 2 {
                    set_log_prob(&mut matrix, 588, 5, 10.0);
                }
                Ok::<FeatureMatrix, Infallible>(matrix)
            },
        )
        .expect("segmentation pass");

        assert_ranges(&result.overlap_ranges, &[(3.0 + 588.0 * frame_s(), 13.0)]);
    }

    #[test]
    fn overlap_ranges_clamp_short_audio_and_merge_touching_frames() {
        let audio = vec![0.0; 3 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |_window_index, _window| {
                let mut matrix = zero_matrix();
                set_log_prob(&mut matrix, 100, 6, 10.0);
                set_log_prob(&mut matrix, 101, 4, 10.0);
                // Beyond the 3 s of real audio: padding, never reported.
                set_log_prob(&mut matrix, 400, 4, 10.0);
                Ok::<FeatureMatrix, Infallible>(matrix)
            },
        )
        .expect("segmentation pass");

        assert_ranges(
            &result.overlap_ranges,
            &[(100.0 * frame_s(), 102.0 * frame_s())],
        );
    }

    #[test]
    fn a_window_without_overlap_frames_adds_no_range() {
        let audio = vec![0.0; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_OVERLAP_STRIDE_S,
            |_, _window| {
                Ok::<FeatureMatrix, Infallible>(class_matrix(&vec![1; PYANNOTE_FRAMES_PER_WINDOW]))
            },
        )
        .expect("segmentation pass");

        assert!(result.overlap_ranges.is_empty());
    }

    fn zero_matrix() -> FeatureMatrix {
        FeatureMatrix::from_row_major(
            PYANNOTE_FRAMES_PER_WINDOW,
            PYANNOTE_CLASS_COUNT,
            vec![0.0; PYANNOTE_FRAMES_PER_WINDOW * PYANNOTE_CLASS_COUNT],
        )
        .expect("zero matrix")
    }

    fn class_matrix(classes: &[usize]) -> FeatureMatrix {
        let mut data = vec![-10.0; classes.len() * PYANNOTE_CLASS_COUNT];
        for (frame, class) in classes.iter().enumerate() {
            data[frame * PYANNOTE_CLASS_COUNT + *class] = 10.0;
        }
        FeatureMatrix::from_row_major(classes.len(), PYANNOTE_CLASS_COUNT, data)
            .expect("class matrix")
    }

    fn set_log_prob(matrix: &mut FeatureMatrix, frame: usize, class: usize, value: f32) {
        let mut data = matrix.data().to_vec();
        data[frame * matrix.bins() + class] = value;
        *matrix = FeatureMatrix::from_row_major(matrix.frames(), matrix.bins(), data)
            .expect("mutated matrix");
    }

    fn usize_array<const N: usize>(value: &Value) -> [usize; N] {
        let array = value.as_array().expect("array");
        assert_eq!(array.len(), N);
        std::array::from_fn(|index| array[index].as_u64().expect("usize value") as usize)
    }

    fn fixture_window_stats(case: &Value) -> SpeakerWindowStats {
        let window = &case["windows"][0];
        SpeakerWindowStats {
            speech_frames: window["speech_frames"].as_u64().expect("speech frames") as usize,
            active_slot_count: window["active_slot_count"]
                .as_u64()
                .expect("active slot count") as usize,
            overlap_frames: window["overlap_frames"].as_u64().expect("overlap frames") as usize,
        }
    }

    fn assert_within(actual: f64, expected: f64, tolerance: f64, label: &str) {
        let diff = (actual - expected).abs();
        assert!(
            diff <= tolerance,
            "{label}: actual={actual} expected={expected} tolerance={tolerance}"
        );
    }

    fn dominant_window_matrix(classes: &[u8]) -> FeatureMatrix {
        assert_eq!(classes.len(), PYANNOTE_FRAMES_PER_WINDOW);
        let mut data = Vec::with_capacity(classes.len() * PYANNOTE_CLASS_COUNT);
        for class in classes {
            let mut row = [-10.0_f32; PYANNOTE_CLASS_COUNT];
            row[usize::from(*class)] = 0.0;
            data.extend(row);
        }
        FeatureMatrix::from_row_major(classes.len(), PYANNOTE_CLASS_COUNT, data).unwrap()
    }

    #[test]
    fn turn_detection_one_window_gap_at_max_emits_range() {
        let samples_per_frame = (PYANNOTE_WINDOW_S as f64 * PYANNOTE_SAMPLE_RATE_HZ as f64)
            / PYANNOTE_FRAMES_PER_WINDOW as f64;
        let mut classes = vec![0_u8; PYANNOTE_FRAMES_PER_WINDOW];
        for c in classes[0..30].iter_mut() {
            *c = 1;
        }
        // 58 silence frames: gap is 58 * 10 / 589 <= 1.0
        for c in classes[88..118].iter_mut() {
            *c = 2;
        }
        let matrix = dominant_window_matrix(&classes);
        let audio = vec![0.0_f32; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |_, _| Ok::<FeatureMatrix, Infallible>(matrix.clone()),
        )
        .unwrap();

        assert_eq!(result.speaker_turns.ranges.len(), 1);
        let prev_end = (30.0 * samples_per_frame) / PYANNOTE_SAMPLE_RATE_HZ as f64;
        let next_start = (88.0 * samples_per_frame) / PYANNOTE_SAMPLE_RATE_HZ as f64;
        assert_eq!(result.speaker_turns.ranges[0].start_s, prev_end);
        assert_eq!(result.speaker_turns.ranges[0].end_s, next_start);
    }

    #[test]
    fn turn_detection_gap_zero_emits_zero_width_range() {
        let samples_per_frame = (PYANNOTE_WINDOW_S as f64 * PYANNOTE_SAMPLE_RATE_HZ as f64)
            / PYANNOTE_FRAMES_PER_WINDOW as f64;
        let mut classes = vec![0_u8; PYANNOTE_FRAMES_PER_WINDOW];
        for c in classes[0..30].iter_mut() {
            *c = 1;
        }
        for c in classes[30..60].iter_mut() {
            *c = 2;
        }
        let matrix = dominant_window_matrix(&classes);
        let audio = vec![0.0_f32; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |_, _| Ok::<FeatureMatrix, Infallible>(matrix.clone()),
        )
        .unwrap();

        assert_eq!(result.speaker_turns.ranges.len(), 1);
        let expected = (30.0 * samples_per_frame) / PYANNOTE_SAMPLE_RATE_HZ as f64;
        assert_eq!(result.speaker_turns.ranges[0].start_s, expected);
        assert_eq!(result.speaker_turns.ranges[0].end_s, expected);
    }

    #[test]
    fn turn_detection_same_class_runs_emit_no_range() {
        let mut classes = vec![0_u8; PYANNOTE_FRAMES_PER_WINDOW];
        for c in classes[0..30].iter_mut() {
            *c = 1;
        }
        for c in classes[40..70].iter_mut() {
            *c = 1;
        }
        let matrix = dominant_window_matrix(&classes);
        let audio = vec![0.0_f32; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |_, _| Ok::<FeatureMatrix, Infallible>(matrix.clone()),
        )
        .unwrap();

        assert!(result.speaker_turns.ranges.is_empty());
    }

    #[test]
    fn turn_detection_low_confidence_rows_emit_no_range() {
        let row1 = [0.59_f32, 0.60, 0.59, 0.59, 0.59, 0.59, 0.59];
        let row2 = [0.59_f32, 0.59, 0.60, 0.59, 0.59, 0.59, 0.59];
        let mut data = Vec::with_capacity(PYANNOTE_FRAMES_PER_WINDOW * PYANNOTE_CLASS_COUNT);
        for _ in 0..40 {
            data.extend(row1);
        }
        for _ in 0..40 {
            data.extend(row2);
        }
        while data.len() < PYANNOTE_FRAMES_PER_WINDOW * PYANNOTE_CLASS_COUNT {
            data.extend([0.0_f32; PYANNOTE_CLASS_COUNT]);
        }
        let matrix =
            FeatureMatrix::from_row_major(PYANNOTE_FRAMES_PER_WINDOW, PYANNOTE_CLASS_COUNT, data)
                .unwrap();
        let audio = vec![0.0_f32; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |_, _| Ok::<FeatureMatrix, Infallible>(matrix.clone()),
        )
        .unwrap();

        assert!(result.speaker_turns.ranges.is_empty());
    }

    #[test]
    fn turn_detection_twenty_nine_frame_runs_emit_no_range() {
        let mut classes = vec![0_u8; PYANNOTE_FRAMES_PER_WINDOW];
        for c in classes[0..29].iter_mut() {
            *c = 1;
        }
        // 1 silence frame
        for c in classes[30..59].iter_mut() {
            *c = 2;
        }
        let matrix = dominant_window_matrix(&classes);
        let audio = vec![0.0_f32; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |_, _| Ok::<FeatureMatrix, Infallible>(matrix.clone()),
        )
        .unwrap();

        assert!(result.speaker_turns.ranges.is_empty());
    }

    #[test]
    fn turn_detection_gap_greater_than_one_second_emits_no_range() {
        let mut classes = vec![0_u8; PYANNOTE_FRAMES_PER_WINDOW];
        for c in classes[0..30].iter_mut() {
            *c = 1;
        }
        // 59 silence frames: gap is 59 * 10 / 589 > 1.0
        for c in classes[89..119].iter_mut() {
            *c = 2;
        }
        let matrix = dominant_window_matrix(&classes);
        let audio = vec![0.0_f32; PYANNOTE_WINDOW_S as usize * PYANNOTE_SAMPLE_RATE_HZ as usize];

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |_, _| Ok::<FeatureMatrix, Infallible>(matrix.clone()),
        )
        .unwrap();

        assert!(result.speaker_turns.ranges.is_empty());
    }

    #[test]
    fn turn_detection_two_windows_no_cross_window_range() {
        // 12s audio gives two starts at 0 and 32_000 (with stride 2s)
        let audio = vec![0.0_f32; 12 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let window0_matrix = dominant_window_matrix(&vec![1_u8; PYANNOTE_FRAMES_PER_WINDOW]);
        let window1_matrix = dominant_window_matrix(&vec![2_u8; PYANNOTE_FRAMES_PER_WINDOW]);

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |window_index, _| {
                if window_index == 0 {
                    Ok::<FeatureMatrix, Infallible>(window0_matrix.clone())
                } else {
                    Ok::<FeatureMatrix, Infallible>(window1_matrix.clone())
                }
            },
        )
        .unwrap();

        assert!(result.speaker_turns.ranges.is_empty());
    }

    #[test]
    fn turn_detection_short_audio_drops_padded_change() {
        let samples_per_frame = (PYANNOTE_WINDOW_S as f64 * PYANNOTE_SAMPLE_RATE_HZ as f64)
            / PYANNOTE_FRAMES_PER_WINDOW as f64;
        // Audio duration is 2.0s
        let audio = vec![0.0_f32; 2 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let mut classes = vec![0_u8; PYANNOTE_FRAMES_PER_WINDOW];
        // Change 1: in-audio (frames 0..30 class 1, frame 30 silence, frames 31..61 class 2)
        for c in classes[0..30].iter_mut() {
            *c = 1;
        }
        for c in classes[31..61].iter_mut() {
            *c = 2;
        }
        // Change 2: starting at frame 200 (at ~3.4s, strictly after 2.0s)
        for c in classes[200..230].iter_mut() {
            *c = 1;
        }
        for c in classes[231..261].iter_mut() {
            *c = 2;
        }
        let matrix = dominant_window_matrix(&classes);

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |_, _| Ok::<FeatureMatrix, Infallible>(matrix.clone()),
        )
        .unwrap();

        assert_eq!(result.speaker_turns.audio_duration_s, 2.0);
        assert_eq!(result.speaker_turns.ranges.len(), 1);
        let expected_start = (30.0 * samples_per_frame) / PYANNOTE_SAMPLE_RATE_HZ as f64;
        let expected_end = (31.0 * samples_per_frame) / PYANNOTE_SAMPLE_RATE_HZ as f64;
        assert_eq!(result.speaker_turns.ranges[0].start_s, expected_start);
        assert_eq!(result.speaker_turns.ranges[0].end_s, expected_end);
    }

    #[test]
    fn turn_detection_shifted_window_range_offset() {
        let samples_per_frame = (PYANNOTE_WINDOW_S as f64 * PYANNOTE_SAMPLE_RATE_HZ as f64)
            / PYANNOTE_FRAMES_PER_WINDOW as f64;
        // 11s audio gives window starts at 0 and 16_000 (1.0s offset)
        let audio = vec![0.0_f32; 11 * PYANNOTE_SAMPLE_RATE_HZ as usize];
        let window0_matrix = dominant_window_matrix(&vec![0_u8; PYANNOTE_FRAMES_PER_WINDOW]);
        let mut classes1 = vec![0_u8; PYANNOTE_FRAMES_PER_WINDOW];
        for c in classes1[0..30].iter_mut() {
            *c = 1;
        }
        for c in classes1[31..61].iter_mut() {
            *c = 2;
        }
        let window1_matrix = dominant_window_matrix(&classes1);

        let result = run_pyannote_segmentation_pass(
            &audio,
            PYANNOTE_SAMPLE_RATE_HZ,
            PYANNOTE_DIARIZE_STRIDE_S,
            |window_index, _| {
                if window_index == 0 {
                    Ok::<FeatureMatrix, Infallible>(window0_matrix.clone())
                } else {
                    Ok::<FeatureMatrix, Infallible>(window1_matrix.clone())
                }
            },
        )
        .unwrap();

        assert_eq!(result.speaker_turns.audio_duration_s, 11.0);
        assert_eq!(result.speaker_turns.ranges.len(), 1);
        let local_start = (30.0 * samples_per_frame) / PYANNOTE_SAMPLE_RATE_HZ as f64;
        let local_end = (31.0 * samples_per_frame) / PYANNOTE_SAMPLE_RATE_HZ as f64;
        let range = result.speaker_turns.ranges[0];
        assert_eq!(range.start_s, local_start + 1.0);
        assert_eq!(range.end_s, local_end + 1.0);
        assert!(range.end_s <= 11.0);
        assert_ne!(range.start_s, local_start);
    }
}
