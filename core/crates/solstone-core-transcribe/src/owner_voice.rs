// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Voice-only transcription: keep the owner's speech, leave out stretches where only other
//! people speak.
//!
//! With `transcribe.owner_voice_only` on, a recording processed for the first time is scored
//! in two-second windows against the owner's saved voice. Windows that match, and windows at
//! overlapping speech or a speaker change, are kept with padding; every other sample is
//! zeroed in place before any later stage sees the audio. The rule and its cutoff are an
//! evaluated operating point.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_observe_audio::VadResult;
use solstone_core_processing_record::{
    OWNER_VOICE_FILTER_KEY, OWNER_VOICE_KEY, OwnerVoiceFilter, OwnerVoiceOriginal, OwnerVoicePrint,
    owner_voice_filtered, read_processing_record_header, vocab,
};

use crate::speakers::{SpeakerAnalyzeError, SpeakerAnalyzeResult};

/// Evaluated operating point: a window whose best owner-voice cosine reaches this is kept.
pub(crate) const OWNER_VOICE_KEEP_SCORE: f64 = 0.229_677_766_661_508_2;

const SAMPLE_RATE: usize = 16_000;
const WINDOW_SAMPLES: usize = 2 * SAMPLE_RATE;
const MIN_SPEECH_SAMPLES: usize = 4_800;
const PAD_BEFORE_SAMPLES: usize = 8_000;
const PAD_AFTER_SAMPLES: usize = 16_000;
/// First-pass statement ids start here, far above any id a published transcript uses.
const SYNTHETIC_ID_BASE: i64 = 1 << 40;
const EMBEDDING_DIMENSIONS: usize = 256;

/// How an attempt treats voice-only transcription.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Eligibility {
    /// The existing output is a voice-only success: it is final and is not reprocessed.
    Final,
    /// This attempt runs only through the filter.
    Filtered,
    /// The ordinary path, unfiltered.
    Today,
}

/// Decide eligibility from what is on disk under the output claim.
pub(crate) fn eligibility(audio_path: &Path, output: &Path, setting_on: bool) -> Eligibility {
    if is_import_stream(audio_path) {
        return Eligibility::Today;
    }
    match read_processing_record_header(output) {
        Some(record) if owner_voice_filtered(&record) => {
            if record.get("state").and_then(Value::as_str) == Some(vocab::STATE_FAILED) {
                Eligibility::Filtered
            } else {
                Eligibility::Final
            }
        }
        Some(record)
            if record.get("state").and_then(Value::as_str) == Some(vocab::STATE_FAILED) =>
        {
            if setting_on {
                Eligibility::Filtered
            } else {
                Eligibility::Today
            }
        }
        Some(_) => Eligibility::Today,
        None if output.exists() => Eligibility::Today,
        None if setting_on => Eligibility::Filtered,
        None => Eligibility::Today,
    }
}

fn is_import_stream(audio_path: &Path) -> bool {
    audio_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|stream| stream.starts_with("import."))
}

/// A two-second window, in samples, half-open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Window {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Window {
    fn start_s(self) -> f64 {
        self.start as f64 / SAMPLE_RATE as f64
    }
    fn end_s(self) -> f64 {
        self.end as f64 / SAMPLE_RATE as f64
    }
}

fn sample_index(seconds: f64, len: usize) -> usize {
    ((seconds * SAMPLE_RATE as f64) as usize).min(len)
}

/// The first VAD's speech as half-open sample ranges.
pub(crate) fn speech_ranges(vad: &VadResult, len: usize) -> Vec<(usize, usize)> {
    vad.speech_segments
        .iter()
        .map(|(start, end)| (sample_index(*start, len), sample_index(*end, len)))
        .filter(|(start, end)| start < end)
        .collect()
}

fn overlap_samples(a: (usize, usize), b: (usize, usize)) -> usize {
    a.1.min(b.1).saturating_sub(a.0.max(b.0))
}

/// Windows with at least 0.3 s of speech that the speaker-analysis adapter admits.
pub(crate) fn scored_windows(
    len: usize,
    speech: &[(usize, usize)],
    admits: impl Fn(Window) -> bool,
) -> Vec<Window> {
    (0..len)
        .step_by(WINDOW_SAMPLES)
        .map(|start| Window {
            start,
            end: (start + WINDOW_SAMPLES).min(len),
        })
        .filter(|window| {
            speech
                .iter()
                .map(|range| overlap_samples(*range, (window.start, window.end)))
                .sum::<usize>()
                >= MIN_SPEECH_SAMPLES
        })
        .filter(|window| admits(*window))
        .collect()
}

/// One synthetic statement for a window.
pub(crate) fn window_statement(index: usize, window: Window) -> Map<String, Value> {
    let Value::Object(statement) = json!({
        "id": SYNTHETIC_ID_BASE + index as i64,
        "start": window.start_s(),
        "end": window.end_s(),
    }) else {
        unreachable!("a JSON object literal is an object")
    };
    statement
}

/// What the first speaker-analysis pass returned for the windows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FirstPass {
    pub(crate) embeddings: Vec<Vec<f32>>,
    pub(crate) turn_ranges: Vec<(f64, f64)>,
    pub(crate) overlap_ranges: Vec<(f64, f64)>,
}

/// Why the first pass cannot be used.
#[derive(Debug)]
pub(crate) enum FirstPassProblem {
    /// The helper predates overlap ranges or speaker turns.
    HelperOutdated,
    /// The response is unusable for these windows.
    Unusable(SpeakerAnalyzeError),
}

/// Read one embedding per window, in window order, from a first-pass result.
pub(crate) fn first_pass(
    raw_path: &Path,
    windows: &[Window],
    result: &SpeakerAnalyzeResult,
) -> Result<FirstPass, FirstPassProblem> {
    let (Some(turn_ranges), Some(overlap_ranges)) =
        (result.turn_ranges.clone(), result.overlap_ranges.clone())
    else {
        return Err(FirstPassProblem::HelperOutdated);
    };
    let unusable = |reason: &str| {
        FirstPassProblem::Unusable(SpeakerAnalyzeError::new(raw_path, "payload", reason, None))
    };
    let payload = result
        .embedding_payload
        .as_ref()
        .ok_or_else(|| unusable("owner-voice-missing-embeddings"))?;
    let row_bytes = EMBEDDING_DIMENSIONS * 4;
    if payload.payload.len() != payload.statement_ids.len() * row_bytes {
        return Err(unusable("owner-voice-embedding-shape"));
    }
    let mut by_id = BTreeMap::new();
    for (row, id) in payload.statement_ids.iter().enumerate() {
        let bytes = &payload.payload[row * row_bytes..(row + 1) * row_bytes];
        let values = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect::<Vec<_>>();
        by_id.insert(*id, values);
    }
    let embeddings = (0..windows.len())
        .map(|index| by_id.remove(&(SYNTHETIC_ID_BASE + index as i64)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| unusable("owner-voice-missing-embedding"))?;
    Ok(FirstPass {
        embeddings,
        turn_ranges,
        overlap_ranges,
    })
}

/// Best cosine of an embedding against the kept prints (already L2-normalized).
pub(crate) fn score(embedding: &[f32], prints: &[&[f32]]) -> f64 {
    let norm = embedding
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return f64::NEG_INFINITY;
    }
    prints
        .iter()
        .map(|print| {
            embedding
                .iter()
                .zip(print.iter())
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum::<f64>()
                / norm
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Whether a window is kept on its own account.
pub(crate) fn keeps(window: Window, score: f64, pass: &FirstPass) -> bool {
    let (start, end) = (window.start_s(), window.end_s());
    score >= OWNER_VOICE_KEEP_SCORE
        || pass
            .overlap_ranges
            .iter()
            .any(|(s, e)| *s < end && *e > start)
        || pass
            .turn_ranges
            .iter()
            .any(|(s, e)| start <= *s && *e <= end)
}

/// Kept windows padded (0.5 s before, 1 s after), clamped and merged.
pub(crate) fn kept_union(kept: &[Window], len: usize) -> Vec<(usize, usize)> {
    let mut ranges = kept
        .iter()
        .map(|window| {
            (
                window.start.saturating_sub(PAD_BEFORE_SAMPLES),
                (window.end + PAD_AFTER_SAMPLES).min(len),
            )
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Zero every sample outside the kept union.
pub(crate) fn mask(audio: &mut [f32], union: &[(usize, usize)]) {
    let mut cursor = 0;
    for (start, end) in union {
        audio[cursor..*start].fill(0.0);
        cursor = *end;
    }
    audio[cursor..].fill(0.0);
}

/// The decision for one recording.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Decision {
    pub(crate) union: Vec<(usize, usize)>,
    pub(crate) windows_scored: u64,
    pub(crate) windows_kept: u64,
}

/// Decide what to keep from the scored windows and the first pass.
pub(crate) fn decide(windows: &[Window], pass: &FirstPass, print: &[f32], len: usize) -> Decision {
    let kept = windows
        .iter()
        .zip(&pass.embeddings)
        .filter(|(window, embedding)| keeps(**window, score(embedding, &[print]), pass))
        .map(|(window, _)| *window)
        .collect::<Vec<_>>();
    Decision {
        union: kept_union(&kept, len),
        windows_scored: windows.len() as u64,
        windows_kept: kept.len() as u64,
    }
}

/// The nothing-kept decision (no speech, or no window scored).
pub(crate) fn keep_nothing() -> Decision {
    Decision {
        union: Vec::new(),
        windows_scored: 0,
        windows_kept: 0,
    }
}

/// SHA-256 of the print's values exactly as loaded, little-endian.
pub(crate) fn print_sha256(values: &[f32]) -> String {
    let mut digest = Sha256::new();
    for value in values {
        digest.update(value.to_le_bytes());
    }
    hex(&digest.finalize())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// The `owner_voice` member for this attempt.
pub(crate) fn filter_record(
    speech: &[(usize, usize)],
    decision: &Decision,
    original_name: &str,
    original_size: u64,
    original_sha256: &[u8; 32],
    print: &[f32],
) -> OwnerVoiceFilter {
    let speech_samples = speech.iter().map(|(start, end)| end - start).sum::<usize>();
    let kept_samples = decision
        .union
        .iter()
        .map(|(start, end)| end - start)
        .sum::<usize>();
    let kept_speech = speech
        .iter()
        .map(|range| {
            decision
                .union
                .iter()
                .map(|kept| overlap_samples(*range, *kept))
                .sum::<usize>()
        })
        .sum::<usize>();
    OwnerVoiceFilter {
        version: 1,
        speech_s: speech_samples as f64 / SAMPLE_RATE as f64,
        kept_s: kept_samples as f64 / SAMPLE_RATE as f64,
        left_out_s: (speech_samples - kept_speech) as f64 / SAMPLE_RATE as f64,
        windows_scored: decision.windows_scored,
        windows_kept: decision.windows_kept,
        original: OwnerVoiceOriginal {
            name: original_name.to_owned(),
            size: original_size,
            sha256: hex(original_sha256),
        },
        print: OwnerVoicePrint {
            sha256: print_sha256(print),
        },
    }
}

/// Record extras every record of a voice-only attempt carries.
pub(crate) fn flag_extra() -> Map<String, Value> {
    let mut extra = Map::new();
    extra.insert(OWNER_VOICE_FILTER_KEY.to_owned(), Value::Bool(true));
    extra
}

/// Record extras for a voice-only success.
pub(crate) fn success_extra(filter: &OwnerVoiceFilter) -> Map<String, Value> {
    let mut extra = flag_extra();
    extra.insert(OWNER_VOICE_KEY.to_owned(), filter.to_value());
    extra
}

/// The filtered audio and what this attempt decided.
#[derive(Debug)]
pub(crate) struct FilterStep {
    pub(crate) audio: Vec<f32>,
    pub(crate) speech: Vec<(usize, usize)>,
    pub(crate) decision: Decision,
}

/// Run the filter on decoded audio and its first VAD.
///
/// `first_pass` runs the transcriber's ordinary speaker analysis over the unmasked audio
/// with the given synthetic window statements; it is called at most once, and not at all
/// when there is no speech or no window is scored.
pub(crate) fn filter_audio<F>(
    raw_path: &Path,
    mut audio: Vec<f32>,
    vad: &VadResult,
    print: &[f32],
    first_pass_call: F,
) -> Result<FilterStep, FirstPassProblem>
where
    F: FnOnce(&[f32], &[Map<String, Value>]) -> Result<SpeakerAnalyzeResult, SpeakerAnalyzeError>,
{
    let len = audio.len();
    let speech = speech_ranges(vad, len);
    let windows = if vad.has_speech {
        scored_windows(len, &speech, |window| {
            crate::speakers::admits_statement(
                raw_path,
                &audio,
                &window_statement(0, window),
                SAMPLE_RATE as u32,
            )
        })
    } else {
        Vec::new()
    };
    let decision = if windows.is_empty() {
        keep_nothing()
    } else {
        let statements = windows
            .iter()
            .enumerate()
            .map(|(index, window)| window_statement(index, *window))
            .collect::<Vec<_>>();
        let result = first_pass_call(&audio, &statements).map_err(FirstPassProblem::Unusable)?;
        let pass = first_pass(raw_path, &windows, &result)?;
        decide(&windows, &pass, print, len)
    };
    mask(&mut audio, &decision.union);
    Ok(FilterStep {
        audio,
        speech,
        decision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(turns: Vec<(f64, f64)>, overlaps: Vec<(f64, f64)>) -> FirstPass {
        FirstPass {
            embeddings: Vec::new(),
            turn_ranges: turns,
            overlap_ranges: overlaps,
        }
    }

    fn unit_print() -> Vec<f32> {
        let mut print = vec![0.0_f32; EMBEDDING_DIMENSIONS];
        print[0] = 1.0;
        print
    }

    #[test]
    fn score_is_the_normalized_best_cosine() {
        let print = unit_print();
        let mut other = vec![0.0_f32; EMBEDDING_DIMENSIONS];
        other[1] = 1.0;
        let mut embedding = vec![0.0_f32; EMBEDDING_DIMENSIONS];
        embedding[0] = 3.0 * 0.6;
        embedding[1] = 3.0 * 0.8;
        let s = score(&embedding, &[&print]);
        assert!((s - 0.6).abs() < 1e-6);
        let best = score(&embedding, &[&print, &other]);
        assert!((best - 0.8).abs() < 1e-6);
        // Scaling an embedding never changes its score.
        let doubled = embedding.iter().map(|v| v * 2.0).collect::<Vec<_>>();
        assert!((score(&doubled, &[&print]) - s).abs() < 1e-12);
    }

    #[test]
    fn the_cutoff_keeps_at_and_above_only() {
        let window = Window {
            start: 0,
            end: WINDOW_SAMPLES,
        };
        let none = pass(vec![], vec![]);
        assert!(keeps(window, OWNER_VOICE_KEEP_SCORE, &none));
        let below = f64::from_bits(OWNER_VOICE_KEEP_SCORE.to_bits() - 1);
        assert!(below < OWNER_VOICE_KEEP_SCORE);
        assert!(!keeps(window, below, &none));
    }

    #[test]
    fn uncertainty_keeps_follow_the_evaluated_rule() {
        let window = Window {
            start: 2 * WINDOW_SAMPLES,
            end: 3 * WINDOW_SAMPLES,
        }; // 4 s .. 6 s
        let low = 0.0;
        assert!(keeps(window, low, &pass(vec![], vec![(5.0, 5.1)])));
        // Touching only is not an intersection.
        assert!(!keeps(window, low, &pass(vec![], vec![(3.0, 4.0)])));
        assert!(!keeps(window, low, &pass(vec![], vec![(6.0, 7.0)])));
        assert!(keeps(window, low, &pass(vec![(4.5, 5.5)], vec![])));
        assert!(keeps(window, low, &pass(vec![(5.0, 5.0)], vec![])));
        assert!(keeps(window, low, &pass(vec![(4.0, 6.0)], vec![])));
        assert!(!keeps(window, low, &pass(vec![(3.5, 4.5)], vec![])));
    }

    #[test]
    fn padding_clamps_and_touching_spans_merge() {
        let len = 10 * SAMPLE_RATE;
        let first = Window {
            start: 0,
            end: WINDOW_SAMPLES,
        };
        let last = Window {
            start: 4 * WINDOW_SAMPLES,
            end: len,
        };
        assert_eq!(
            kept_union(&[first], len),
            vec![(0, WINDOW_SAMPLES + PAD_AFTER_SAMPLES)]
        );
        assert_eq!(
            kept_union(&[last], len),
            vec![(4 * WINDOW_SAMPLES - PAD_BEFORE_SAMPLES, len)]
        );
        // A window ending at 2 s pads to 3 s; one starting at 3.5 s pads back to 3 s: they touch.
        let a = Window {
            start: 0,
            end: WINDOW_SAMPLES,
        };
        let b = Window {
            start: 56_000,
            end: 56_000 + WINDOW_SAMPLES,
        };
        assert_eq!(
            kept_union(&[b, a], len),
            vec![(0, 56_000 + WINDOW_SAMPLES + PAD_AFTER_SAMPLES)]
        );
    }

    #[test]
    fn scoring_needs_speech_and_admission() {
        let len = 3 * WINDOW_SAMPLES;
        let speech = vec![(0, 4_799), (WINDOW_SAMPLES, WINDOW_SAMPLES + 4_800)];
        let windows = scored_windows(len, &speech, |_| true);
        assert_eq!(
            windows,
            vec![Window {
                start: WINDOW_SAMPLES,
                end: 2 * WINDOW_SAMPLES
            }]
        );
        assert!(scored_windows(len, &speech, |_| false).is_empty());
    }

    #[test]
    fn masking_zeroes_exactly_outside_the_union() {
        let mut audio = vec![1.0_f32; 100];
        mask(&mut audio, &[(10, 20), (50, 60)]);
        for (index, sample) in audio.iter().enumerate() {
            let kept = (10..20).contains(&index) || (50..60).contains(&index);
            assert_eq!(*sample, if kept { 1.0 } else { 0.0 }, "sample {index}");
        }
    }

    #[test]
    fn counts_follow_the_definitions() {
        let speech = vec![(0, 32_000), (64_000, 80_000)];
        let decision = Decision {
            union: vec![(0, 40_000)],
            windows_scored: 2,
            windows_kept: 1,
        };
        let filter = filter_record(&speech, &decision, "a.m4a", 7, &[0u8; 32], &unit_print());
        assert_eq!(filter.speech_s, 3.0);
        assert_eq!(filter.kept_s, 2.5);
        assert_eq!(filter.left_out_s, 1.0);
        assert_eq!(filter.original.sha256, "0".repeat(64));
        assert_eq!(filter.print.sha256.len(), 64);
    }

    fn vad(segments: Vec<(f64, f64)>) -> VadResult {
        VadResult {
            duration_s: 10.0,
            speech_duration_s: segments.iter().map(|(a, b)| b - a).sum(),
            has_speech: !segments.is_empty(),
            speech_segments: segments,
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        }
    }

    fn result_for(
        embeddings: &[(i64, Vec<f32>)],
        turns: Option<Vec<(f64, f64)>>,
        overlaps: Option<Vec<(f64, f64)>>,
    ) -> SpeakerAnalyzeResult {
        let payload = embeddings
            .iter()
            .flat_map(|(_, values)| values.iter().flat_map(|v| v.to_le_bytes()))
            .collect::<Vec<u8>>();
        SpeakerAnalyzeResult {
            statements: Vec::new(),
            embedding_payload: Some(crate::speakers::SpeakerEmbeddingPayload {
                payload,
                statement_ids: embeddings.iter().map(|(id, _)| *id).collect(),
                durations_s: embeddings.iter().map(|_| 2.0).collect(),
                encoder: "wespeaker-resnet34-256".into(),
            }),
            speaker_evidence: "single".into(),
            speaker_evidence_multi_fraction: 0.0,
            overlap_fraction: 0.0,
            statement_labels: None,
            turn_ranges: turns,
            overlap_ranges: overlaps,
        }
    }

    fn along(axis: usize, scale: f32) -> Vec<f32> {
        let mut values = vec![0.0_f32; EMBEDDING_DIMENSIONS];
        values[axis] = scale;
        values
    }

    #[test]
    fn filter_audio_masks_everything_downstream_sees() {
        // 10 s of non-zero audio, speech everywhere; window 1 is the owner, window 3 sits on a
        // speaker change, the rest score low.
        let len = 10 * SAMPLE_RATE;
        let audio = vec![0.5_f32; len];
        let print = unit_print();
        let calls = std::cell::Cell::new(0);
        let step = filter_audio(
            Path::new("a.m4a"),
            audio,
            &vad(vec![(0.0, 10.0)]),
            &print,
            |seen, statements| {
                calls.set(calls.get() + 1);
                assert!(
                    seen.iter().all(|v| *v == 0.5),
                    "the first pass sees the unmasked audio"
                );
                assert_eq!(statements.len(), 5);
                let ids = (0..5).map(|i| SYNTHETIC_ID_BASE + i).collect::<Vec<_>>();
                Ok(result_for(
                    &[
                        (ids[0], along(1, 2.0)),
                        (ids[1], along(0, 3.0)),
                        (ids[2], along(1, 1.0)),
                        (ids[3], along(1, 1.0)),
                        (ids[4], along(1, 1.0)),
                    ],
                    Some(vec![(6.5, 6.6)]),
                    Some(vec![]),
                ))
            },
        )
        .expect("filtered");
        assert_eq!(calls.get(), 1);
        assert_eq!(step.decision.windows_scored, 5);
        assert_eq!(step.decision.windows_kept, 2);
        // Window 1 (2-4 s) pads to 1.5-5 s; window 3 (6-8 s) pads to 5.5-9 s.
        assert_eq!(
            step.decision.union,
            vec![(24_000, 80_000), (88_000, 144_000)]
        );
        for (index, sample) in step.audio.iter().enumerate() {
            let kept = (24_000..80_000).contains(&index) || (88_000..144_000).contains(&index);
            assert_eq!(*sample, if kept { 0.5 } else { 0.0 }, "sample {index}");
        }
    }

    #[test]
    fn filter_audio_skips_the_first_pass_without_scored_speech() {
        let len = 4 * SAMPLE_RATE;
        for vad in [vad(vec![]), vad(vec![(0.0, 0.29)])] {
            let step = filter_audio(
                Path::new("a.m4a"),
                vec![0.5; len],
                &vad,
                &unit_print(),
                |_, _| panic!("no first pass without scored speech"),
            )
            .expect("filtered");
            assert_eq!(step.decision.windows_scored, 0);
            assert!(step.audio.iter().all(|v| *v == 0.0));
        }
    }

    #[test]
    fn filter_audio_reports_an_outdated_helper_and_missing_embeddings() {
        let len = 2 * SAMPLE_RATE;
        let speech = vad(vec![(0.0, 2.0)]);
        for (turns, overlaps) in [(None, Some(vec![])), (Some(vec![]), None)] {
            let outcome = filter_audio(
                Path::new("a.m4a"),
                vec![0.5; len],
                &speech,
                &unit_print(),
                |_, _| {
                    Ok(result_for(
                        &[(SYNTHETIC_ID_BASE, along(0, 1.0))],
                        turns.clone(),
                        overlaps.clone(),
                    ))
                },
            );
            assert!(matches!(outcome, Err(FirstPassProblem::HelperOutdated)));
        }
        let outcome = filter_audio(
            Path::new("a.m4a"),
            vec![0.5; len],
            &speech,
            &unit_print(),
            |_, _| Ok(result_for(&[], Some(vec![]), Some(vec![]))),
        );
        assert!(matches!(outcome, Err(FirstPassProblem::Unusable(_))));
    }

    #[test]
    fn eligibility_table() {
        let dir = tempfile::tempdir().unwrap();
        let stream = dir.path().join("chronicle/20261020/watch/120000_300");
        std::fs::create_dir_all(&stream).unwrap();
        let audio = stream.join("120000_300.m4a");
        let output = stream.join("120000_300.jsonl");
        let write = |record: Value| {
            std::fs::write(
                &output,
                format!("{}\n", json!({"_solstone_processing": record})),
            )
            .unwrap();
        };
        assert_eq!(eligibility(&audio, &output, true), Eligibility::Filtered);
        assert_eq!(eligibility(&audio, &output, false), Eligibility::Today);
        write(json!({"state": "analyzed", "owner_voice_filter": true}));
        assert_eq!(eligibility(&audio, &output, false), Eligibility::Final);
        write(json!({"state": "empty", "owner_voice": {"broken": 1}}));
        assert_eq!(eligibility(&audio, &output, false), Eligibility::Final);
        write(json!({"state": "failed", "owner_voice_filter": true}));
        assert_eq!(eligibility(&audio, &output, false), Eligibility::Filtered);
        write(json!({"state": "failed"}));
        assert_eq!(eligibility(&audio, &output, true), Eligibility::Filtered);
        assert_eq!(eligibility(&audio, &output, false), Eligibility::Today);
        write(json!({"state": "analyzed"}));
        assert_eq!(eligibility(&audio, &output, true), Eligibility::Today);
        std::fs::write(&output, "{\"start\":\"12:00:00\"}\n").unwrap();
        assert_eq!(eligibility(&audio, &output, true), Eligibility::Today);

        let import = dir
            .path()
            .join("chronicle/20261020/import.audio/120000_300");
        std::fs::create_dir_all(&import).unwrap();
        assert_eq!(
            eligibility(&import.join("a.m4a"), &import.join("a.jsonl"), true),
            Eligibility::Today
        );
    }
}
