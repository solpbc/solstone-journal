// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Embedded speech, one native call at a time, through the end of the recording.
//!
//! The first unit has no leading seam. Each later unit repeats the previous
//! pyannote window inside the native-call cap. Words whose restored start falls
//! in an earlier unit's committed range are dropped. Desktop decode never enters
//! this loop.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Map, Value, json};
use solstone_core_assets::canonical_host_pair;
use solstone_core_journal_config::JournalConfigRead;
use solstone_core_local::install::capability_status::CapabilityStatus;
use solstone_core_local::install::ced_readiness::{
    CedVerdict, evaluate_ced_readiness_with_probe, fresh_model_check,
};
use solstone_core_observe_audio::{
    AudioError, AudioReduction, DEFAULT_LOUD_RMS_THRESHOLD, DEFAULT_LOUD_WINDOW_SECONDS,
    MIN_NONSPEECH_SEGMENT_SECONDS, SAMPLE_RATE, UnitDecoder, VadResult,
    compute_loud_speech_windows, compute_nonspeech_rms, reduce_audio,
};
use solstone_core_processing_record::{
    TerminalProofOutcome, evaluate_terminal_proof, read_processing_record_header, vocab,
};
use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
use solstone_core_sound_tags::{SoundTagWindows, fold_sound_tag_windows, read_sound_tag_windows};
use solstone_core_speaker_resolve::owner_centroid::OwnerVoicePrint;
use solstone_core_speakers::diarization::{
    DiarizationError, SentenceTiming, SpeakerInterval, assign_sentences,
    cluster_embeddings_cancellable,
};
use solstone_core_speakers::{
    SpeakerTurnEvidence, SpeakerTurnRange, SpeakerWindowStats, decide_speaker_evidence,
};
use solstone_core_spp_ratls::AttestationStateStore;

use crate::backend::parakeet_cpp::{ModelInfo, TranscriptionResponse, TranscriptionWord};
use crate::embedded::{EMBEDDED_AUDIO_MAX_SAMPLES, EmbeddedContext};
use crate::event::Timings;
use crate::speakers::{SpeakerAnalyzeResult, SpeakerEmbeddingPayload, UnitStatementEmbedding};
use crate::stage::{
    BackendErrorContext, ProcessOutcome, TerminalOutcome, backend_failure, base_time_us_of_day,
    commit_input, decode_failure, dispatch_backend, dispatch_confidential_with_cap,
    emit_backend_error, emit_event, finish_context_output, finish_voice_only_deletion,
    first_pass_analysis, merge_record_extra, segment_meta, source_from_path, statements_from_words,
    stt_zero_statements, transcribe_retained_intervals, transcript_paths, vad_no_speech,
    write_failed_record,
};
use crate::transcript::{FullTranscriptWrite, restore_statement_timestamps, write_full_transcript};
use crate::{TranscribeError, event::TranscribedOutcome};

/// Samples in the pyannote window repeated at the front of every unit after the first.
pub(crate) const EMBEDDED_SEAM_SAMPLES: usize =
    solstone_core_speakers::PYANNOTE_WINDOW_S as usize * SAMPLE_RATE as usize;
/// New samples committed by a full unit. The seam rides inside the native-call cap.
pub(crate) const EMBEDDED_STEP_SAMPLES: usize = EMBEDDED_AUDIO_MAX_SAMPLES - EMBEDDED_SEAM_SAMPLES;
// Caller evidence, not this process: one native 120s unit with the detector alive
// peaked at 2,339,104 KiB RSS, a 30s prefix of that binary at 1,284,684 KiB, and a
// rejected 150s draft of this pipeline at 2,130,508 KiB. Those figures are not this
// tree's peak_rss_mib. The long-wav lib test prints this process without a speech
// model; that print is not the native working set, and the KiB figures are not phone
// latency. The native-call cap stays 1_920_000 and is not an input ceiling.
const EMBEDDING_WIDTH: usize = 256;
const EMBEDDING_ENCODER: &str = "wespeaker-resnet34-256";

/// One embedded recording after the claim, input observation, and backend choice.
pub(crate) struct EmbeddedJob<'a> {
    pub(crate) audio_path: &'a Path,
    pub(crate) journal_path: &'a Path,
    pub(crate) redo: bool,
    pub(crate) backend: String,
    pub(crate) config: &'a JournalConfigRead,
    pub(crate) attestation_state: &'a AttestationStateStore,
    pub(crate) embedded: &'a EmbeddedContext<'a>,
    pub(crate) context_attempt: Option<solstone_core_system::work_retry::Attempt>,
    pub(crate) observation: InputObservation,
    pub(crate) claim: OutputClaims,
    pub(crate) facts: crate::stage::InputFacts,
    pub(crate) timings: Timings,
    pub(crate) started: Instant,
    pub(crate) owner_voice_print: Option<OwnerVoicePrint>,
    pub(crate) sound_tags: SoundTagRun,
}

/// Half-open unit windows on the original timeline.
///
/// `n == 0` yields nothing. An exact multiple of the step yields no empty unit.
/// Every window is at most [`EMBEDDED_AUDIO_MAX_SAMPLES`] long.
///
/// The streaming loop does not call this. Tests compare its windows with the
/// samples that loop actually pulls.
#[cfg(test)]
pub(crate) fn plan_embedded_units(n: usize) -> Vec<(usize, usize)> {
    let mut committed = 0;
    let mut units = Vec::new();
    while committed < n {
        let start = if committed == 0 {
            0
        } else {
            committed - EMBEDDED_SEAM_SAMPLES
        };
        let end = n.min(committed + EMBEDDED_STEP_SAMPLES);
        units.push((start, end));
        committed = end;
        if end == n {
            break;
        }
    }
    units
}

pub(crate) fn empty_vad() -> VadResult {
    VadResult {
        duration_s: 0.0,
        speech_duration_s: 0.0,
        has_speech: false,
        speech_segments: Vec::new(),
        noisy_rms: None,
        noisy_s: 0.0,
        loud_windows: 0,
        speech_loud_windows: 0,
    }
}

/// Restore word times onto the original timeline and keep those that start in this unit.
///
/// A start sample before `committed_start` belongs to the previous unit. A start exactly
/// at `committed_start` belongs to this unit. A start at or after `committed_end` belongs
/// to the following unit. Restored times are not rewritten again.
pub(crate) fn place_unit_words(
    words: &[TranscriptionWord],
    reduction: Option<&AudioReduction>,
    origin_samples: usize,
    committed_start: usize,
    committed_end: usize,
) -> Vec<TranscriptionWord> {
    let origin_s = origin_samples as f64 / f64::from(SAMPLE_RATE);
    let mut placed = Vec::new();
    for word in words {
        let (start, end) = restored_span(word.start, word.end, reduction);
        let start = start + origin_s;
        let end = end + origin_s;
        let start_sample = (start * f64::from(SAMPLE_RATE)).round() as usize;
        if start_sample < committed_start || start_sample >= committed_end {
            continue;
        }
        placed.push(TranscriptionWord {
            word: word.word.clone(),
            start,
            end,
            probability: word.probability,
        });
    }
    placed
}

/// Concatenate words that [`place_unit_words`] already restored. The loop
/// extends one vector as it goes; this helper is the same concatenation for tests.
#[cfg(test)]
pub(crate) fn merge_unit_words(
    units: impl IntoIterator<Item = Vec<TranscriptionWord>>,
) -> Vec<TranscriptionWord> {
    units.into_iter().flatten().collect()
}

/// Recognize one masked unit. The dispatch receives retained audio, never the unmasked unit.
pub(crate) fn transcribe_prepared_unit<F>(
    masked: &[f32],
    vad: &VadResult,
    dispatch: F,
) -> Result<(TranscriptionResponse, ModelInfo), TranscribeError>
where
    F: FnMut(&[f32]) -> Result<(TranscriptionResponse, ModelInfo), TranscribeError>,
{
    let retained = retain_audio(masked, vad);
    let audio = retained.samples.as_deref().unwrap_or(masked);
    transcribe_retained_intervals(audio, retained.mapping.as_ref(), dispatch)
}

/// Run the embedded unit loop for a backend that [`select_backend`](crate::embedded::select_backend) already chose.
pub(crate) fn process_embedded_units(
    mut job: EmbeddedJob<'_>,
) -> Result<ProcessOutcome, TranscribeError> {
    #[cfg(all(test, feature = "full-tests"))]
    let _vad_replay = VadReplayGuard::arm();
    let held = prior_publication_held(job.audio_path, job.facts.input_size);
    let min_speech = crate::config::min_speech_seconds(job.config);
    if job.backend == "confidential" {
        return match confidential_pass(&mut job, held, min_speech)? {
            DraftOrDone::Draft(draft) => publish(job, held, draft),
            DraftOrDone::Done(outcome) => Ok(outcome),
        };
    }
    let admission = match each_prepared_unit(&mut job, held, false, WalkKind::Admit, min_speech) {
        Ok(bundle) => bundle,
        Err(stop) => return apply_stop(&mut job, held, stop),
    };
    if !admission.summary.any_speech {
        finish_filter(&mut job, &admission.filter);
        return publish(job, held, TranscriptDraft::from_summary(admission.summary));
    }
    match transcribe_walk(&mut job, held, min_speech) {
        Ok(draft) => publish(job, held, draft),
        Err(stop) => apply_stop(&mut job, held, stop),
    }
}

fn prior_publication_held(audio: &Path, input_size: u64) -> bool {
    let header = read_processing_record_header(&audio.with_extension("jsonl"));
    evaluate_terminal_proof(header.as_ref(), vocab::HANDLER_TRANSCRIBE, input_size)
        == TerminalProofOutcome::Held
}

enum WalkStop {
    Recorded(ProcessOutcome),
    Bare(TranscribeError),
    Inference {
        error: TranscribeError,
        audio_seconds: f64,
        vad: VadResult,
    },
}

enum DraftOrDone {
    Draft(TranscriptDraft),
    Done(ProcessOutcome),
}

struct WalkSummary {
    any_speech: bool,
    total_samples: usize,
    speech_segments: Vec<(f64, f64)>,
    noisy_weighted: f64,
    noisy_s: f64,
    loud_windows: usize,
    speech_loud_windows: usize,
}

impl WalkSummary {
    fn empty() -> Self {
        Self {
            any_speech: false,
            total_samples: 0,
            speech_segments: Vec::new(),
            noisy_weighted: 0.0,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        }
    }

    fn audio_seconds(&self) -> f64 {
        self.total_samples as f64 / f64::from(SAMPLE_RATE)
    }

    fn vad(&self) -> VadResult {
        let speech_duration_s = self
            .speech_segments
            .iter()
            .map(|(start, end)| (end - start).max(0.0))
            .sum();
        let noisy_rms = (self.noisy_s > 0.0).then_some(self.noisy_weighted / self.noisy_s);
        VadResult {
            duration_s: self.audio_seconds(),
            speech_duration_s,
            has_speech: self.any_speech,
            speech_segments: self.speech_segments.clone(),
            noisy_rms,
            noisy_s: self.noisy_s,
            loud_windows: self.loud_windows,
            speech_loud_windows: self.speech_loud_windows,
        }
    }

    fn add_noisy(&mut self, span: &NoisySpan) {
        self.noisy_weighted += span.weighted;
        self.noisy_s += span.noisy_s;
        self.loud_windows += span.loud_windows;
        self.speech_loud_windows += span.speech_loud_windows;
    }
}

struct NoisySpan {
    weighted: f64,
    noisy_s: f64,
    loud_windows: usize,
    speech_loud_windows: usize,
}

impl NoisySpan {
    fn empty() -> Self {
        Self {
            weighted: 0.0,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        }
    }
}

struct FilterPiece {
    speech: Vec<(usize, usize)>,
    union: Vec<(usize, usize)>,
    windows: Vec<(usize, bool)>,
}

impl FilterPiece {
    fn empty() -> Self {
        Self {
            speech: Vec::new(),
            union: Vec::new(),
            windows: Vec::new(),
        }
    }
}

struct FilterAccum {
    speech: Vec<(usize, usize)>,
    union: Vec<(usize, usize)>,
    windows: BTreeMap<usize, bool>,
}

impl FilterAccum {
    fn new() -> Self {
        Self {
            speech: Vec::new(),
            union: Vec::new(),
            windows: BTreeMap::new(),
        }
    }

    fn absorb(&mut self, piece: FilterPiece) {
        self.speech.extend(piece.speech);
        self.union.extend(piece.union);
        for (start, kept) in piece.windows {
            self.windows.entry(start).or_insert(kept);
        }
    }
}

struct EvidencePiece {
    windows: Vec<(usize, SpeakerWindowStats)>,
    overlap_fraction: f64,
    samples: usize,
    turns: Option<Vec<(f64, f64)>>,
    overlaps: Option<Vec<(f64, f64)>>,
}

impl EvidencePiece {
    fn empty() -> Self {
        Self {
            windows: Vec::new(),
            overlap_fraction: 0.0,
            samples: 0,
            turns: None,
            overlaps: None,
        }
    }
}

struct UnitPrepared {
    masked: Vec<f32>,
    vad: VadResult,
    pre_speech: Vec<(usize, usize)>,
    decision: Option<crate::owner_voice::Decision>,
    origin_samples: usize,
    committed_start: usize,
    committed_end: usize,
}

/// One recording's recognizer output, clustered and published once.
///
/// The draft grows by one kept recognizer word, one embedding row per helper
/// interval (`EMBEDDING_WIDTH`), and one statement-embedding row per admitted
/// statement. That is the scope bound. Unit PCM is not retained.
struct TranscriptDraft {
    summary: WalkSummary,
    words: Vec<TranscriptionWord>,
    intervals: Vec<SpeakerInterval>,
    interval_embeddings: Vec<f32>,
    statement_rows: Vec<crate::speakers::UnitStatementEmbedding>,
    boundaries: Vec<usize>,
    model: Option<ModelInfo>,
    reduced_seconds: Option<f64>,
    windows: BTreeMap<usize, SpeakerWindowStats>,
    overlap_weight: f64,
    overlap_samples: usize,
    turns: Option<Vec<(f64, f64)>>,
    overlaps: Option<Vec<(f64, f64)>>,
}

impl TranscriptDraft {
    fn from_summary(summary: WalkSummary) -> Self {
        Self {
            summary,
            words: Vec::new(),
            intervals: Vec::new(),
            interval_embeddings: Vec::new(),
            statement_rows: Vec::new(),
            boundaries: Vec::new(),
            model: None,
            reduced_seconds: None,
            windows: BTreeMap::new(),
            overlap_weight: 0.0,
            overlap_samples: 0,
            turns: None,
            overlaps: None,
        }
    }
}

enum WalkKind<'a> {
    Admit,
    Count(&'a mut usize),
    Recognize {
        draft: &'a mut TranscriptDraft,
        reduced_sum: &'a mut f64,
        reduced_any: &'a mut bool,
    },
}

struct WalkBundle {
    summary: WalkSummary,
    filter: FilterAccum,
}

struct PendingTail {
    pub_start: usize,
    held_end: usize,
    audio_end: usize,
    speech: Vec<(f64, f64)>,
    noisy: NoisySpan,
    filter: FilterPiece,
    words: Vec<TranscriptionWord>,
    evidence: EvidencePiece,
    statement_rows: Vec<UnitStatementEmbedding>,
}

struct RetainedAudio {
    samples: Option<Vec<f32>>,
    mapping: Option<AudioReduction>,
}

fn retain_audio(masked: &[f32], vad: &VadResult) -> RetainedAudio {
    match reduce_audio(masked, vad) {
        Some((samples, mapping)) => RetainedAudio {
            samples: Some(samples),
            mapping: Some(mapping),
        },
        None => RetainedAudio {
            samples: None,
            mapping: None,
        },
    }
}

fn retained_sample_count(masked: &[f32], vad: &VadResult) -> usize {
    match reduce_audio(masked, vad) {
        Some((samples, _)) => samples.len(),
        None => masked.len(),
    }
}

fn restored_span(start: f64, end: f64, reduction: Option<&AudioReduction>) -> (f64, f64) {
    let Some(reduction) = reduction else {
        return (start, end);
    };
    let restored_end = if start == end {
        reduction.restore_timestamp(end)
    } else {
        reduction.restore_end_timestamp(end)
    };
    (reduction.restore_timestamp(start), restored_end)
}

fn confidential_pass(
    job: &mut EmbeddedJob<'_>,
    held: bool,
    min_speech: f64,
) -> Result<DraftOrDone, TranscribeError> {
    let mut retained = 0usize;
    let counted =
        match each_prepared_unit(job, held, false, WalkKind::Count(&mut retained), min_speech) {
            Ok(bundle) => bundle,
            Err(stop) => return apply_stop(job, held, stop).map(DraftOrDone::Done),
        };
    if !counted.summary.any_speech {
        finish_filter(job, &counted.filter);
        return Ok(DraftOrDone::Draft(TranscriptDraft::from_summary(
            counted.summary,
        )));
    }
    let seconds = counted.summary.audio_seconds();
    let vad = counted.summary.vad();
    let mut stop = None;
    let mut done = None;
    // The second walk is the closure. An over-budget total never calls it, so nothing is sent.
    let gated = dispatch_confidential_with_cap(retained, seconds, || {
        match transcribe_walk(job, held, min_speech) {
            Ok(draft) => Ok(draft),
            Err(WalkStop::Recorded(outcome)) => {
                done = Some(outcome);
                // `dispatch_confidential_with_cap` only carries `TranscribeError`. The stored
                // outcome is returned below; this marker is not published.
                Err(TranscribeError::Cancelled)
            }
            Err(other) => {
                stop = Some(other);
                Err(TranscribeError::Cancelled)
            }
        }
    });
    if let Some(stop) = stop {
        return apply_stop(job, held, stop).map(DraftOrDone::Done);
    }
    if let Some(outcome) = done {
        return Ok(DraftOrDone::Done(outcome));
    }
    match gated {
        Ok(draft) => Ok(DraftOrDone::Draft(draft)),
        Err(error) => {
            settle_inference(job, held, error, seconds, vad)?;
            unreachable!("inference settlement returns the error");
        }
    }
}

fn transcribe_walk(
    job: &mut EmbeddedJob<'_>,
    held: bool,
    min_speech: f64,
) -> Result<TranscriptDraft, WalkStop> {
    let mut draft = TranscriptDraft::from_summary(WalkSummary::empty());
    let mut reduced_sum = 0.0;
    let mut reduced_any = false;
    let bundle = each_prepared_unit(
        job,
        held,
        true,
        WalkKind::Recognize {
            draft: &mut draft,
            reduced_sum: &mut reduced_sum,
            reduced_any: &mut reduced_any,
        },
        min_speech,
    )?;
    finish_filter(job, &bundle.filter);
    draft.summary = bundle.summary;
    if reduced_any {
        draft.reduced_seconds = Some(reduced_sum);
    }
    if !draft.summary.any_speech {
        draft.words.clear();
        draft.intervals.clear();
        draft.interval_embeddings.clear();
        draft.statement_rows.clear();
    }
    Ok(draft)
}

struct RecognizeTail {
    words: Vec<TranscriptionWord>,
    evidence: EvidencePiece,
    statement_rows: Vec<UnitStatementEmbedding>,
}

fn recognize_unit(
    job: &mut EmbeddedJob<'_>,
    prepared: &UnitPrepared,
    audio_end: usize,
    draft: &mut TranscriptDraft,
    reduced_sum: &mut f64,
    reduced_any: &mut bool,
) -> Result<RecognizeTail, WalkStop> {
    let origin_s = prepared.origin_samples as f64 / f64::from(SAMPLE_RATE);
    let seen_seconds =
        (prepared.origin_samples + prepared.masked.len()) as f64 / f64::from(SAMPLE_RATE);
    let dispatch = |samples: &[f32]| {
        if job.backend == "confidential" {
            dispatch_backend(
                job.config,
                job.journal_path,
                &job.backend,
                samples,
                job.attestation_state,
                Some(job.embedded),
            )
        } else {
            job.embedded.transcribe(samples)
        }
    };
    let (response, model) = transcribe_prepared_unit(&prepared.masked, &prepared.vad, dispatch)
        .map_err(|error| classify_asr(error, seen_seconds, prepared.vad.clone()))?;
    let retained = retain_audio(&prepared.masked, &prepared.vad);
    if let Some(mapping) = retained.mapping.as_ref() {
        *reduced_any = true;
        *reduced_sum += mapping.reduced_duration_s;
    }
    // An empty recognizer result is the same no-transcript branch as desktop: it
    // publishes nothing from this unit and does not load the speaker helper.
    if response.words.is_empty() {
        draft.model = Some(model);
        return Ok(RecognizeTail {
            words: Vec::new(),
            evidence: EvidencePiece::empty(),
            statement_rows: Vec::new(),
        });
    }
    let pre_restore = statements_from_words(&response.words);
    let restored = retained.mapping.as_ref().map_or_else(
        || pre_restore.clone(),
        |mapping| restore_statement_timestamps(&pre_restore, mapping),
    );
    let rows = crate::speakers::collect_unit_speaker_rows(
        job.audio_path,
        &prepared.masked,
        retained.samples.as_deref(),
        &pre_restore,
        &restored,
        job.embedded.models,
        job.embedded.scratch,
        origin_s,
    )
    .map_err(|error| WalkStop::Inference {
        error: TranscribeError::SpeakerAnalysis(error),
        audio_seconds: seen_seconds,
        vad: prepared.vad.clone(),
    })?;
    draft.intervals.extend(rows.intervals);
    draft.interval_embeddings.extend(rows.interval_embeddings);
    draft.boundaries.push(prepared.committed_start);
    draft.model = Some(model);
    let placed = place_unit_words(
        &response.words,
        retained.mapping.as_ref(),
        prepared.origin_samples,
        prepared.committed_start,
        usize::MAX,
    );
    let held_end = prepared.committed_end;
    let mut kept_words = Vec::new();
    let mut tail_words = Vec::new();
    for word in placed {
        let sample = (word.start * f64::from(SAMPLE_RATE)).round() as usize;
        if sample >= audio_end {
            continue;
        }
        if sample >= held_end {
            tail_words.push(word);
        } else {
            kept_words.push(word);
        }
    }
    draft.words.extend(kept_words);
    let (mut kept_windows, mut tail_windows) = (Vec::new(), Vec::new());
    for (start, stats) in rows.windows {
        if start >= audio_end {
            continue;
        }
        if start >= held_end {
            tail_windows.push((start, stats));
        } else {
            kept_windows.push((start, stats));
        }
    }
    let (mut kept_rows, mut tail_rows) = (Vec::new(), Vec::new());
    for row in rows.statement_rows {
        let sample = (row.start_s * f64::from(SAMPLE_RATE)).round() as usize;
        if sample >= prepared.committed_start && sample < held_end {
            kept_rows.push(row);
        } else if sample >= held_end && sample < audio_end {
            tail_rows.push(row);
        }
    }
    draft.statement_rows.extend(kept_rows);
    absorb_evidence(
        draft,
        EvidencePiece {
            windows: kept_windows,
            overlap_fraction: rows.overlap_fraction,
            samples: held_end.saturating_sub(prepared.committed_start),
            turns: clip_optional(&rows.turns, prepared.committed_start, held_end),
            overlaps: clip_optional(&rows.overlaps, prepared.committed_start, held_end),
        },
    );
    Ok(RecognizeTail {
        words: tail_words,
        evidence: EvidencePiece {
            windows: tail_windows,
            overlap_fraction: rows.overlap_fraction,
            samples: audio_end.saturating_sub(held_end),
            turns: clip_optional(&rows.turns, held_end, audio_end),
            overlaps: clip_optional(&rows.overlaps, held_end, audio_end),
        },
        statement_rows: tail_rows,
    })
}

fn classify_asr(error: TranscribeError, audio_seconds: f64, vad: VadResult) -> WalkStop {
    match error {
        TranscribeError::Cancelled | TranscribeError::ModelAsset(_) => WalkStop::Bare(error),
        error => WalkStop::Inference {
            error,
            audio_seconds,
            vad,
        },
    }
}

fn each_prepared_unit(
    job: &mut EmbeddedJob<'_>,
    held: bool,
    record_commits: bool,
    mut kind: WalkKind<'_>,
    min_speech: f64,
) -> Result<WalkBundle, WalkStop> {
    let mut decoder =
        UnitDecoder::open(job.audio_path).map_err(|error| stop_decode(job, held, error))?;
    let mut seam = Vec::new();
    let mut audio_cursor = 0usize;
    let mut pub_cursor = 0usize;
    let mut pending: Option<PendingTail> = None;
    let mut summary = WalkSummary::empty();
    let mut filter = FilterAccum::new();
    let mut deferred_seam: Option<(usize, Vec<f32>)> = None;
    let tagging = matches!(kind, WalkKind::Admit | WalkKind::Count(_));
    loop {
        job.embedded.checkpoint().map_err(WalkStop::Bare)?;
        let pulled = decoder
            .pull(EMBEDDED_STEP_SAMPLES)
            .map_err(|error| stop_decode(job, held, error))?;
        let Some(new_samples) = pulled.filter(|samples| !samples.is_empty()) else {
            if audio_cursor == 0 && pending.is_none() {
                return Err(stop_decode(
                    job,
                    held,
                    AudioError::NoDecodedAudio {
                        path: job.audio_path.to_path_buf(),
                    },
                ));
            }
            if let Some((start, samples)) = deferred_seam.take() {
                let end = start + samples.len();
                tag_unit(job, &samples, start, &[(start, end)])?;
            }
            if let Some(tail) = pending.take() {
                let start = tail.pub_start;
                let end = tail.audio_end;
                absorb_tail(&mut summary, &mut filter, &mut kind, tail);
                note_commit(record_commits, start, end);
            }
            break;
        };
        if let Some(tail) = pending.take() {
            // The successor owns the held-back pre-pad. Drop this unit's copy of it.
            note_commit(record_commits, tail.pub_start, tail.held_end);
        }
        // The successor's masked buffer emits the seam. Drop the saved copy.
        deferred_seam = None;
        let seam_len = seam.len();
        let mut unit = std::mem::take(&mut seam);
        let new_len = new_samples.len();
        unit.extend_from_slice(&new_samples);
        debug_assert!(unit.len() <= EMBEDDED_AUDIO_MAX_SAMPLES);
        let origin = audio_cursor - seam_len;
        let audio_end = audio_cursor + new_len;
        let full = new_len == EMBEDDED_STEP_SAMPLES;
        // Hold the commit back by the owner-voice pre-pad only while a later unit may exist.
        // That later unit already contains the window at this boundary inside the 10 s seam.
        let held_end = if full {
            audio_end - crate::owner_voice::PAD_BEFORE_SAMPLES
        } else {
            audio_end
        };
        if full {
            let tail = unit.len() - EMBEDDED_SEAM_SAMPLES;
            seam = unit[tail..].to_vec();
        }
        let mut prepared = prepare_unit(job, held, unit, origin)?;
        prepared.committed_start = pub_cursor;
        prepared.committed_end = held_end;
        if tagging {
            let plan = plan_tag_windows(origin, audio_end, held_end, full);
            tag_unit(job, &prepared.masked, origin, &plan.spans)?;
            if let Some((start, end)) = plan.deferred_seam {
                let width = end - start;
                let from = prepared.masked.len() - width;
                deferred_seam = Some((start, prepared.masked[from..].to_vec()));
            }
        }
        let committed_speech =
            clip_speech(&prepared.vad.speech_segments, origin, pub_cursor, held_end);
        summary.add_noisy(&noisy_for(
            &prepared.masked,
            origin,
            &committed_speech,
            pub_cursor,
            held_end,
        ));
        summary.speech_segments.extend(committed_speech);
        filter.absorb(filter_span(&prepared, pub_cursor, held_end, 0, held_end));
        let mut tail_words = Vec::new();
        let mut tail_evidence = EvidencePiece::empty();
        let mut tail_rows = Vec::new();
        if speech_overlaps(&prepared.vad, origin, pub_cursor, audio_end) {
            match &mut kind {
                WalkKind::Admit => {}
                WalkKind::Count(retained) => {
                    **retained += retained_sample_count(&prepared.masked, &prepared.vad);
                }
                WalkKind::Recognize {
                    draft,
                    reduced_sum,
                    reduced_any,
                } => {
                    let split =
                        recognize_unit(job, &prepared, audio_end, draft, reduced_sum, reduced_any)?;
                    tail_words = split.words;
                    tail_evidence = split.evidence;
                    tail_rows = split.statement_rows;
                }
            }
        }
        if full {
            let tail_speech =
                clip_speech(&prepared.vad.speech_segments, origin, held_end, audio_end);
            pending = Some(PendingTail {
                pub_start: pub_cursor,
                held_end,
                audio_end,
                speech: tail_speech.clone(),
                noisy: noisy_for(&prepared.masked, origin, &tail_speech, held_end, audio_end),
                filter: filter_span(&prepared, held_end, audio_end, held_end, audio_end),
                words: tail_words,
                evidence: tail_evidence,
                statement_rows: tail_rows,
            });
            pub_cursor = held_end;
        } else {
            note_commit(record_commits, pub_cursor, audio_end);
            pub_cursor = audio_end;
        }
        audio_cursor = audio_end;
    }
    summary.speech_segments = merge_seconds(std::mem::take(&mut summary.speech_segments));
    let duration = summary
        .speech_segments
        .iter()
        .map(|(start, end)| end - start)
        .sum::<f64>();
    summary.any_speech = duration >= min_speech;
    summary.total_samples = audio_cursor;
    Ok(WalkBundle { summary, filter })
}

fn absorb_tail(
    summary: &mut WalkSummary,
    filter: &mut FilterAccum,
    kind: &mut WalkKind<'_>,
    tail: PendingTail,
) {
    summary.speech_segments.extend(tail.speech);
    summary.add_noisy(&tail.noisy);
    filter.absorb(tail.filter);
    if let WalkKind::Recognize { draft, .. } = kind {
        draft.words.extend(tail.words);
        draft.statement_rows.extend(tail.statement_rows);
        absorb_evidence(draft, tail.evidence);
    }
}

fn prepare_unit(
    job: &mut EmbeddedJob<'_>,
    held: bool,
    unit: Vec<f32>,
    origin_samples: usize,
) -> Result<UnitPrepared, WalkStop> {
    let mut vad = unit_vad(job, &unit).map_err(WalkStop::Bare)?;
    let mut masked = unit;
    let mut pre_speech = Vec::new();
    let mut decision = None;
    if let Some(print) = job.owner_voice_print.clone() {
        job.embedded.checkpoint().map_err(WalkStop::Bare)?;
        let step = crate::owner_voice::filter_audio(
            job.audio_path,
            std::mem::take(&mut masked),
            &vad,
            &print.values,
            |audio, statements| {
                first_pass_analysis(job.audio_path, audio, statements, Some(job.embedded))
            },
        );
        let step = match step {
            Ok(step) => step,
            Err(problem) => return Err(filter_stop(job, held, problem, &vad)),
        };
        pre_speech = step.speech;
        decision = Some(step.decision);
        masked = step.audio;
        vad = masked_unit_vad(job, &masked).map_err(WalkStop::Bare)?;
    }
    Ok(UnitPrepared {
        masked,
        vad,
        pre_speech,
        decision,
        origin_samples,
        committed_start: 0,
        committed_end: 0,
    })
}

fn unit_vad(job: &EmbeddedJob<'_>, audio: &[f32]) -> Result<VadResult, TranscribeError> {
    #[cfg(all(test, feature = "full-tests"))]
    if let Some(vad) = hooked_vad(audio) {
        return Ok(vad);
    }
    crate::audio::run_vad_embedded(audio, 0.0, job.embedded.models, job.embedded.scratch)
}

fn masked_unit_vad(job: &EmbeddedJob<'_>, audio: &[f32]) -> Result<VadResult, TranscribeError> {
    #[cfg(all(test, feature = "full-tests"))]
    if let Some(vad) = hooked_vad(audio) {
        return Ok(vad);
    }
    crate::audio::run_vad_embedded(audio, 0.0, job.embedded.models, job.embedded.scratch)
}

#[cfg(all(test, feature = "full-tests"))]
fn hooked_vad(audio: &[f32]) -> Option<VadResult> {
    if let Some(hook) = EMBEDDED_VAD_STICKY.with(|cell| *cell.borrow()) {
        return Some(hook(audio.len()));
    }
    if let Some(vad) = EMBEDDED_VAD_REPLAY.with(|cell| cell.borrow().clone()) {
        return Some(vad);
    }
    let taken = crate::stage::take_processing_test_vad()?;
    EMBEDDED_VAD_REPLAY.with(|cell| *cell.borrow_mut() = Some(taken.clone()));
    Some(taken)
}

fn finish_filter(job: &mut EmbeddedJob<'_>, filter: &FilterAccum) {
    let Some(print) = job.owner_voice_print.clone() else {
        return;
    };
    let speech = merge_samples(filter.speech.clone());
    let union = merge_samples(filter.union.clone());
    let windows_kept = filter.windows.values().filter(|kept| **kept).count() as u64;
    let decision = crate::owner_voice::Decision {
        union,
        windows_scored: filter.windows.len() as u64,
        windows_kept,
        scored: Vec::new(),
        kept: Vec::new(),
    };
    let name = job
        .audio_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let record = crate::owner_voice::filter_record(
        &speech,
        &decision,
        name,
        job.facts.input_size,
        &job.observation.input_sha256(),
        &print.values,
    );
    job.facts.record_extra = crate::owner_voice::success_extra(&record);
}

fn filter_span(
    prepared: &UnitPrepared,
    span_start: usize,
    span_end: usize,
    window_lo: usize,
    window_hi: usize,
) -> FilterPiece {
    let Some(decision) = prepared.decision.as_ref() else {
        return FilterPiece::empty();
    };
    let origin = prepared.origin_samples;
    let kept = decision
        .kept
        .iter()
        .map(|window| window.start)
        .collect::<BTreeSet<_>>();
    let windows = decision
        .scored
        .iter()
        .filter_map(|window| {
            let absolute = origin + window.start;
            (absolute >= window_lo && absolute < window_hi)
                .then_some((absolute, kept.contains(&window.start)))
        })
        .collect();
    FilterPiece {
        speech: shift_clip(&prepared.pre_speech, origin, span_start, span_end),
        union: shift_clip(&decision.union, origin, span_start, span_end),
        windows,
    }
}

fn shift_clip(
    ranges: &[(usize, usize)],
    origin: usize,
    start: usize,
    end: usize,
) -> Vec<(usize, usize)> {
    ranges
        .iter()
        .filter_map(|(range_start, range_end)| {
            let range_start = range_start.saturating_add(origin).max(start);
            let range_end = range_end.saturating_add(origin).min(end);
            (range_start < range_end).then_some((range_start, range_end))
        })
        .collect()
}

fn clip_speech(
    segments: &[(f64, f64)],
    origin: usize,
    start: usize,
    end: usize,
) -> Vec<(f64, f64)> {
    let origin_s = start_seconds(origin);
    let start_s = start_seconds(start);
    let end_s = start_seconds(end);
    segments
        .iter()
        .filter_map(|(segment_start, segment_end)| {
            let segment_start = (segment_start + origin_s).max(start_s);
            let segment_end = (segment_end + origin_s).min(end_s);
            (segment_start < segment_end).then_some((segment_start, segment_end))
        })
        .collect()
}

fn speech_overlaps(vad: &VadResult, origin: usize, start: usize, end: usize) -> bool {
    !clip_speech(&vad.speech_segments, origin, start, end).is_empty()
}

fn noisy_for(
    audio: &[f32],
    origin: usize,
    speech: &[(f64, f64)],
    start: usize,
    end: usize,
) -> NoisySpan {
    if end <= start || start < origin {
        return NoisySpan::empty();
    }
    let local_start = start - origin;
    let local_end = (end - origin).min(audio.len());
    if local_end <= local_start || local_start >= audio.len() {
        return NoisySpan::empty();
    }
    let slice = &audio[local_start..local_end];
    let shift = start_seconds(start);
    let local = speech
        .iter()
        .map(|(segment_start, segment_end)| (segment_start - shift, segment_end - shift))
        .collect::<Vec<_>>();
    let (rms, noisy_s) =
        compute_nonspeech_rms(slice, &local, SAMPLE_RATE, MIN_NONSPEECH_SEGMENT_SECONDS);
    let (loud_windows, speech_loud_windows) = compute_loud_speech_windows(
        slice,
        &local,
        SAMPLE_RATE,
        DEFAULT_LOUD_WINDOW_SECONDS,
        DEFAULT_LOUD_RMS_THRESHOLD,
    );
    NoisySpan {
        weighted: rms.map(|value| value * noisy_s).unwrap_or(0.0),
        noisy_s,
        loud_windows,
        speech_loud_windows,
    }
}

fn start_seconds(samples: usize) -> f64 {
    samples as f64 / f64::from(SAMPLE_RATE)
}

fn merge_seconds(mut ranges: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    ranges.retain(|(start, end)| start.is_finite() && end.is_finite() && start < end);
    ranges.sort_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut merged = Vec::new();
    for (start, end) in ranges {
        if let Some((_, last_end)) = merged.last_mut()
            && start <= *last_end
        {
            *last_end = last_end.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

fn merge_samples(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.retain(|(start, end)| start < end);
    ranges.sort_unstable();
    let mut merged = Vec::new();
    for (start, end) in ranges {
        if let Some((_, last_end)) = merged.last_mut()
            && start <= *last_end
        {
            *last_end = (*last_end).max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

fn clip_ranges(ranges: &[(f64, f64)], start: usize, end: usize) -> Vec<(f64, f64)> {
    let start_s = start_seconds(start);
    let end_s = start_seconds(end);
    ranges
        .iter()
        .filter_map(|(range_start, range_end)| {
            let range_start = range_start.max(start_s);
            let range_end = range_end.min(end_s);
            (range_start < range_end).then_some((range_start, range_end))
        })
        .collect()
}

fn clip_optional(
    ranges: &Option<Vec<(f64, f64)>>,
    start: usize,
    end: usize,
) -> Option<Vec<(f64, f64)>> {
    ranges
        .as_ref()
        .map(|ranges| clip_ranges(ranges, start, end))
}

fn absorb_optional_ranges(dest: &mut Option<Vec<(f64, f64)>>, piece: Option<Vec<(f64, f64)>>) {
    let Some(piece) = piece else {
        return;
    };
    match dest {
        None => *dest = Some(merge_seconds(piece)),
        Some(existing) => {
            existing.extend(piece);
            *existing = merge_seconds(std::mem::take(existing));
        }
    }
}

fn absorb_evidence(draft: &mut TranscriptDraft, piece: EvidencePiece) {
    for (start, stats) in piece.windows {
        draft.windows.entry(start).or_insert(stats);
    }
    draft.overlap_weight += piece.overlap_fraction * piece.samples as f64;
    draft.overlap_samples += piece.samples;
    absorb_optional_ranges(&mut draft.turns, piece.turns);
    absorb_optional_ranges(&mut draft.overlaps, piece.overlaps);
}

fn filter_stop(
    job: &mut EmbeddedJob<'_>,
    held: bool,
    problem: crate::owner_voice::FirstPassProblem,
    vad: &VadResult,
) -> WalkStop {
    let audio_seconds = vad.duration_s;
    match problem {
        crate::owner_voice::FirstPassProblem::Unusable(error) => {
            let error = TranscribeError::SpeakerAnalysis(error);
            if !held {
                emit_event(
                    job.audio_path,
                    job.journal_path,
                    TranscribedOutcome::Failed,
                    Some("owner-voice-first-pass-failure"),
                    Some("SpeakerAnalyzeError"),
                    Some(&error),
                    None,
                    None,
                    None,
                    Some(audio_seconds),
                    None,
                    None,
                    &job.timings,
                    Some(vad),
                );
            }
            WalkStop::Bare(error)
        }
        crate::owner_voice::FirstPassProblem::HelperOutdated => {
            if held {
                return WalkStop::Bare(TranscribeError::SpeakerAnalysis(
                    crate::speakers::SpeakerAnalyzeError::new(
                        job.audio_path,
                        "payload",
                        "owner-voice-helper-outdated",
                        None,
                    ),
                ));
            }
            if let Err(error) = job.embedded.checkpoint() {
                return WalkStop::Bare(error);
            }
            match record_helper_outdated(job) {
                Ok(()) => WalkStop::Recorded(ProcessOutcome::Failed),
                Err(error) => WalkStop::Bare(error),
            }
        }
    }
}

fn record_helper_outdated(job: &EmbeddedJob<'_>) -> Result<(), TranscribeError> {
    let checkpoint = || job.embedded.checkpoint();
    let commit = commit_input(&job.observation, &job.claim, job.audio_path, &checkpoint)?;
    let mut processing = crate::processing::backend_failure_record(
        job.facts.input_size,
        vocab::REASON_ANALYSIS_FAILED,
    );
    processing["error"] =
        json!("the speaker-analysis helper predates overlap ranges or speaker turns");
    write_failed_record(
        &job.facts,
        job.redo,
        processing,
        job.context_attempt.as_ref(),
    )?;
    drop(commit);
    finish_context_output(
        job.journal_path,
        &job.facts.path.with_extension("jsonl"),
        job.context_attempt.as_ref(),
        job.audio_path,
        &checkpoint,
    )?;
    emit_event(
        job.audio_path,
        job.journal_path,
        TranscribedOutcome::Failed,
        Some("owner_voice_helper_outdated"),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        &job.timings,
        None,
    );
    Ok(())
}

fn stop_decode(job: &mut EmbeddedJob<'_>, held: bool, error: AudioError) -> WalkStop {
    if held {
        return WalkStop::Bare(TranscribeError::Decode {
            detail: error.to_string(),
        });
    }
    if let Err(error) = job.embedded.checkpoint() {
        return WalkStop::Bare(error);
    }
    match record_decode(job, &error) {
        Ok(()) => WalkStop::Recorded(ProcessOutcome::Failed),
        Err(error) => WalkStop::Bare(error),
    }
}

fn record_decode(job: &EmbeddedJob<'_>, error: &AudioError) -> Result<(), TranscribeError> {
    let decode_error = TranscribeError::Decode {
        detail: error.to_string(),
    };
    let checkpoint = || job.embedded.checkpoint();
    let commit = commit_input(&job.observation, &job.claim, job.audio_path, &checkpoint)?;
    let terminal = decode_failure(&job.facts, job.redo, error, job.context_attempt.as_ref())?;
    drop(commit);
    finish_context_output(
        job.journal_path,
        &job.facts.path.with_extension("jsonl"),
        job.context_attempt.as_ref(),
        job.audio_path,
        &checkpoint,
    )?;
    emit_event(
        job.audio_path,
        job.journal_path,
        TranscribedOutcome::Failed,
        None,
        Some("AudioError"),
        Some(&decode_error),
        None,
        None,
        None,
        None,
        None,
        None,
        &job.timings,
        None,
    );
    match terminal {
        TerminalOutcome::Failed => Ok(()),
        _ => unreachable!("decode failure publishes a failed record"),
    }
}

fn apply_stop(
    job: &mut EmbeddedJob<'_>,
    held: bool,
    stop: WalkStop,
) -> Result<ProcessOutcome, TranscribeError> {
    match stop {
        WalkStop::Recorded(outcome) => Ok(outcome),
        WalkStop::Bare(error) => Err(error),
        WalkStop::Inference {
            error,
            audio_seconds,
            vad,
        } => settle_inference(job, held, error, audio_seconds, vad),
    }
}

fn settle_inference(
    job: &mut EmbeddedJob<'_>,
    held: bool,
    error: TranscribeError,
    audio_seconds: f64,
    vad: VadResult,
) -> Result<ProcessOutcome, TranscribeError> {
    if matches!(
        error,
        TranscribeError::Cancelled | TranscribeError::ModelAsset(_)
    ) {
        return Err(error);
    }
    if held {
        return Err(error);
    }
    job.embedded.checkpoint()?;
    let checkpoint = || job.embedded.checkpoint();
    let commit = commit_input(&job.observation, &job.claim, job.audio_path, &checkpoint)?;
    backend_failure(&job.facts, job.redo, &error, job.context_attempt.as_ref())?;
    drop(commit);
    finish_context_output(
        job.journal_path,
        &job.facts.path.with_extension("jsonl"),
        job.context_attempt.as_ref(),
        job.audio_path,
        &checkpoint,
    )?;
    emit_backend_error(
        BackendErrorContext {
            audio_path: job.audio_path,
            journal_path: job.journal_path,
            audio_seconds,
            reduced_seconds: None,
            timings: &job.timings,
            vad: &vad,
        },
        &error,
        Some(&job.backend),
    );
    Err(error)
}

fn publish(
    job: EmbeddedJob<'_>,
    held: bool,
    draft: TranscriptDraft,
) -> Result<ProcessOutcome, TranscribeError> {
    if !draft.summary.any_speech {
        return publish_empty(job, EmptyKind::NoSpeech, draft);
    }
    if draft.words.is_empty() {
        return publish_empty(job, EmptyKind::NoTranscript, draft);
    }
    let speaker_result =
        match build_speaker_result(job.audio_path, &draft, &|| job.embedded.checkpoint()) {
            Ok(result) => result,
            Err(error) => {
                let mut job = job;
                return settle_inference(
                    &mut job,
                    held,
                    error,
                    draft.summary.audio_seconds(),
                    draft.summary.vad(),
                );
            }
        };
    // A held success is replaced when this attempt was admitted. Failure writers are the
    // ones that refuse to touch a held publication.
    publish_transcript(job, draft, speaker_result)
}

enum EmptyKind {
    NoSpeech,
    NoTranscript,
}

fn publish_empty(
    job: EmbeddedJob<'_>,
    kind: EmptyKind,
    draft: TranscriptDraft,
) -> Result<ProcessOutcome, TranscribeError> {
    let EmbeddedJob {
        audio_path,
        journal_path,
        redo,
        backend,
        embedded,
        context_attempt,
        observation,
        claim,
        facts,
        timings,
        started,
        sound_tags,
        ..
    } = job;
    let vad = draft.summary.vad();
    let audio_seconds = draft.summary.audio_seconds();
    let checkpoint = || embedded.checkpoint();
    checkpoint()?;
    let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
    let (sound_tags, sound_tag_status) = sound_tags.publication();
    let terminal = match kind {
        EmptyKind::NoSpeech => vad_no_speech(
            &facts,
            redo,
            sound_tags.as_ref(),
            sound_tag_status.as_ref(),
            context_attempt.as_ref(),
        )?,
        EmptyKind::NoTranscript => stt_zero_statements(
            &facts,
            redo,
            sound_tags.as_ref(),
            sound_tag_status.as_ref(),
            context_attempt.as_ref(),
        )?,
    };
    drop(observation);
    finish_voice_only_deletion(journal_path, &facts, &claim, &commit, Some(embedded));
    drop(commit);
    finish_context_output(
        journal_path,
        &facts.path.with_extension("jsonl"),
        context_attempt.as_ref(),
        audio_path,
        &checkpoint,
    )?;
    let outcome = match terminal {
        TerminalOutcome::Preserved => (ProcessOutcome::Preserved, TranscribedOutcome::Preserved),
        TerminalOutcome::Failed => (ProcessOutcome::Failed, TranscribedOutcome::Failed),
    };
    let model = draft.model.as_ref();
    emit_event(
        audio_path,
        journal_path,
        outcome.1,
        None,
        None,
        None,
        matches!(kind, EmptyKind::NoTranscript).then_some(backend.as_str()),
        model.map(|info| info.device.as_str()),
        model.map(|info| info.model.as_str()),
        Some(audio_seconds),
        draft.reduced_seconds,
        Some(elapsed_ms(started)),
        &timings,
        Some(&vad),
    );
    Ok(outcome.0)
}

fn publish_transcript(
    job: EmbeddedJob<'_>,
    draft: TranscriptDraft,
    speaker_result: SpeakerAnalyzeResult,
) -> Result<ProcessOutcome, TranscribeError> {
    let EmbeddedJob {
        audio_path,
        journal_path,
        redo,
        backend,
        embedded,
        context_attempt,
        observation,
        claim,
        facts,
        mut timings,
        started,
        sound_tags,
        ..
    } = job;
    let (sound_tags, sound_tag_status) = sound_tags.publication();
    let vad = draft.summary.vad();
    let audio_seconds = draft.summary.audio_seconds();
    let model = draft.model.expect("speech with words has a model");
    let (jsonl_path, npz_path) = transcript_paths(audio_path);
    let mut processing = crate::processing::analyzed_record(facts.input_size);
    merge_record_extra(&mut processing, &facts.record_extra);
    if let Some(attempt) = context_attempt.as_ref() {
        solstone_core_system::work_retry::raw::stamp_attempt_from(&mut processing, attempt)
            .map_err(|detail| TranscribeError::ProcessingClaim {
                path: audio_path.to_path_buf(),
                detail,
            })?;
    } else {
        solstone_core_system::work_retry::raw::stamp_attempt(&mut processing).map_err(
            |detail| TranscribeError::ProcessingClaim {
                path: audio_path.to_path_buf(),
                detail,
            },
        )?;
    }
    let write_at = Instant::now();
    let checkpoint = || embedded.checkpoint();
    checkpoint()?;
    let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
    write_full_transcript(FullTranscriptWrite {
        raw_path: audio_path,
        jsonl_path: &jsonl_path,
        npz_path: &npz_path,
        base_time_us_of_day: base_time_us_of_day(audio_path),
        source: source_from_path(audio_path).as_deref(),
        speaker_result: &speaker_result,
        backend: Some(&backend),
        model: Some(&model.model),
        device: Some(&model.device),
        compute_type: (!model.compute_type.is_empty()).then_some(model.compute_type.as_str()),
        observer: std::env::var("OBSERVER_NAME").ok().as_deref(),
        vad_result: Some(&vad),
        segment_meta: segment_meta().as_ref(),
        overlap_detector: Some(crate::stage::OVERLAP_DETECTOR),
        speaker_evidence_version: crate::stage::SPEAKER_EVIDENCE_VERSION,
        processing: &processing,
        sound_tags: sound_tags.as_ref(),
        sound_tag_status: sound_tag_status.as_ref(),
        speaker_analysis_producer: Some(crate::stage::SPEAKER_ANALYSIS_PRODUCER),
        redo,
    })?;
    drop(observation);
    finish_voice_only_deletion(journal_path, &facts, &claim, &commit, Some(embedded));
    drop(commit);
    finish_context_output(
        journal_path,
        &jsonl_path,
        context_attempt.as_ref(),
        audio_path,
        &checkpoint,
    )?;
    timings.add_ms("write", elapsed_ms(write_at));
    emit_event(
        audio_path,
        journal_path,
        TranscribedOutcome::Transcribed,
        None,
        None,
        None,
        Some(&backend),
        Some(&model.device),
        Some(&model.model),
        Some(audio_seconds),
        draft.reduced_seconds,
        Some(elapsed_ms(started)),
        &timings,
        Some(&vad),
    );
    Ok(ProcessOutcome::Transcribed)
}

fn build_speaker_result(
    audio_path: &Path,
    draft: &TranscriptDraft,
    checkpoint: &dyn Fn() -> Result<(), TranscribeError>,
) -> Result<SpeakerAnalyzeResult, TranscribeError> {
    let mut statements = statements_from_words(&draft.words);
    let sentences = statements
        .iter()
        .map(|statement| SentenceTiming {
            start_s: statement.get("start").and_then(Value::as_f64),
            end_s: statement.get("end").and_then(Value::as_f64),
        })
        .collect::<Vec<_>>();
    let assigned = if draft.intervals.is_empty() {
        vec![None; statements.len()]
    } else {
        let rows = draft.intervals.len();
        if draft.interval_embeddings.len() != rows * EMBEDDING_WIDTH {
            return Err(TranscribeError::SpeakerAnalysis(
                crate::speakers::SpeakerAnalyzeError::new(
                    audio_path,
                    "payload",
                    "interval-embedding-width",
                    None,
                ),
            ));
        }
        checkpoint()?;
        let clustered = cluster_embeddings_cancellable(
            &draft.interval_embeddings,
            rows,
            EMBEDDING_WIDTH,
            None,
            &|| checkpoint().is_err(),
        )
        .map_err(|error| match error {
            DiarizationError::Cancelled => TranscribeError::Cancelled,
            other => TranscribeError::SpeakerAnalysis(crate::speakers::SpeakerAnalyzeError::new(
                audio_path,
                "cluster",
                other.to_string(),
                None,
            )),
        })?;
        assign_sentences(&sentences, &draft.intervals, &clustered.labels).map_err(|error| {
            TranscribeError::SpeakerAnalysis(crate::speakers::SpeakerAnalyzeError::new(
                audio_path,
                "assign",
                error.to_string(),
                None,
            ))
        })?
    };
    let labels = assigned
        .into_iter()
        .map(|label| label.map(|value| i64::try_from(value).unwrap_or(i64::MAX)))
        .collect::<Vec<_>>();
    for (statement, label) in statements.iter_mut().zip(&labels) {
        if let Some(label) = label {
            statement.insert("speaker".to_owned(), Value::from(*label));
        }
    }
    let mut payload = Vec::new();
    let mut statement_ids = Vec::new();
    let mut durations_s = Vec::new();
    for statement in &statements {
        if statement_crosses(statement, &draft.boundaries) {
            continue;
        }
        let Some(row) = matching_row(&draft.statement_rows, statement) else {
            continue;
        };
        let Some(id) = statement.get("id").and_then(Value::as_i64) else {
            continue;
        };
        payload.extend(row.values.iter().flat_map(|value| value.to_le_bytes()));
        statement_ids.push(id);
        durations_s.push(row.duration_s);
    }
    let embedding_payload = (!statement_ids.is_empty()).then(|| SpeakerEmbeddingPayload {
        payload,
        statement_ids,
        durations_s,
        encoder: EMBEDDING_ENCODER.to_owned(),
    });
    let stats = draft.windows.values().copied().collect::<Vec<_>>();
    let overlap_fraction = if draft.overlap_samples == 0 {
        0.0
    } else {
        draft.overlap_weight / draft.overlap_samples as f64
    };
    let evidence = decide_speaker_evidence(overlap_fraction, &stats);
    if let Some(ranges) = &draft.turns {
        let turn_evidence = SpeakerTurnEvidence::new(
            draft.summary.audio_seconds(),
            ranges
                .iter()
                .map(|(start, end)| SpeakerTurnRange {
                    start_s: *start,
                    end_s: *end,
                })
                .collect(),
        );
        for statement in &mut statements {
            let start_s = statement.get("start").and_then(Value::as_f64);
            let end_s = statement.get("end").and_then(Value::as_f64);
            let mut sentence = turn_evidence.clone();
            sentence.ranges.retain(|range| {
                matches!(
                    (start_s, end_s),
                    (Some(start), Some(end)) if range.end_s >= start && range.start_s <= end
                )
            });
            if sentence.flags_sentence(start_s, end_s) {
                statement.insert("turn_boundary_suspect".to_owned(), Value::Bool(true));
            }
            statement.insert("speaker_turn_evidence".to_owned(), sentence.to_value());
        }
    }
    Ok(SpeakerAnalyzeResult {
        statements,
        embedding_payload,
        speaker_evidence: evidence.speaker_evidence.as_str().to_owned(),
        speaker_evidence_multi_fraction: evidence.multi_window_fraction,
        overlap_fraction,
        statement_labels: Some(labels),
        turn_ranges: draft.turns.clone(),
        overlap_ranges: draft.overlaps.clone(),
    })
}

fn statement_crosses(statement: &Map<String, Value>, boundaries: &[usize]) -> bool {
    let Some(words) = statement.get("words").and_then(Value::as_array) else {
        return false;
    };
    let mut seen = None;
    for word in words {
        let Some(start) = word.get("start").and_then(Value::as_f64) else {
            continue;
        };
        let sample = (start * f64::from(SAMPLE_RATE)).round() as usize;
        let index = boundaries
            .partition_point(|boundary| *boundary <= sample)
            .saturating_sub(1);
        if let Some(previous) = seen {
            if previous != index {
                return true;
            }
        }
        seen = Some(index);
    }
    false
}

fn matching_row<'a>(
    rows: &'a [UnitStatementEmbedding],
    statement: &Map<String, Value>,
) -> Option<&'a UnitStatementEmbedding> {
    let start = statement.get("start")?.as_f64()?;
    let end = statement.get("end")?.as_f64()?;
    rows.iter()
        .find(|row| row.start_s == start && row.end_s == end)
}

const TAG_WINDOW_SAMPLES: usize =
    solstone_core_sound_tags::WINDOW_S * solstone_core_sound_tags::CLASSIFY_SAMPLE_RATE as usize;
const TAG_MIN_TAIL_SAMPLES: usize =
    solstone_core_sound_tags::MIN_TAIL_S * solstone_core_sound_tags::CLASSIFY_SAMPLE_RATE as usize;

/// Global ced windows for one embedded unit.
///
/// Grid spans are `[kS, (k+1)S)` with `O <= kS` and `(k+1)S <= stable_limit`.
/// A full unit defers `[E - S, E)` so the successor, or the empty pull when the
/// recording ended on that unit, emits it once. A final partial unit adds the
/// desktop tail `[N - S, N)` when the remainder is at least one second.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TagUnitPlan {
    spans: Vec<(usize, usize)>,
    deferred_seam: Option<(usize, usize)>,
}

fn plan_tag_windows(
    origin: usize,
    audio_end: usize,
    stable_limit: usize,
    full: bool,
) -> TagUnitPlan {
    let mut spans = Vec::new();
    if TAG_WINDOW_SAMPLES > 0 && origin < audio_end {
        let mut k = origin / TAG_WINDOW_SAMPLES;
        if k * TAG_WINDOW_SAMPLES < origin {
            k += 1;
        }
        loop {
            let start = k.saturating_mul(TAG_WINDOW_SAMPLES);
            let Some(end) = start.checked_add(TAG_WINDOW_SAMPLES) else {
                break;
            };
            if start < origin || end > stable_limit || end > audio_end {
                break;
            }
            spans.push((start, end));
            k += 1;
        }
    }
    if !full
        && audio_end >= TAG_WINDOW_SAMPLES
        && audio_end % TAG_WINDOW_SAMPLES >= TAG_MIN_TAIL_SAMPLES
    {
        spans.push((audio_end - TAG_WINDOW_SAMPLES, audio_end));
    }
    // `then_some` evaluates its argument immediately, so the subtraction stays
    // inside the guard. A recording under one window has `audio_end < S`.
    let deferred_seam = if full && audio_end >= TAG_WINDOW_SAMPLES {
        Some((audio_end - TAG_WINDOW_SAMPLES, audio_end))
    } else {
        None
    };
    TagUnitPlan {
        spans,
        deferred_seam,
    }
}

struct UnitClassifyView<'a> {
    samples: &'a [f32],
    windows: Vec<(usize, usize)>,
}

fn unit_classify_view<'a>(
    masked: &'a [f32],
    origin: usize,
    spans: &[(usize, usize)],
) -> UnitClassifyView<'a> {
    UnitClassifyView {
        samples: masked,
        windows: spans
            .iter()
            .map(|(start, end)| (start - origin, end - origin))
            .collect(),
    }
}

enum TagPhase {
    Pending,
    Ready,
    Finished {
        tags: Option<Value>,
        status: Option<CapabilityStatus>,
    },
}

enum SoundTagFailure {
    Prepare(String),
    Run(String),
}

/// In-process sound tags for one recording. The first walk fills it; publication reads it.
pub(crate) struct SoundTagRun {
    phase: TagPhase,
    accepted: Vec<Value>,
    library: PathBuf,
    model: PathBuf,
}

impl SoundTagRun {
    pub(crate) fn new() -> Self {
        Self {
            phase: TagPhase::Pending,
            accepted: Vec::new(),
            library: PathBuf::new(),
            model: PathBuf::new(),
        }
    }

    fn accepting(&self) -> bool {
        matches!(self.phase, TagPhase::Pending | TagPhase::Ready)
    }

    fn apply_verdict(&mut self, os: &str, arch: &str, verdict: CedVerdict) {
        match verdict {
            CedVerdict::Unsupported { os, arch } => {
                log::warn!("sound tagger disabled: ced assets unsupported on {os}/{arch}");
                self.finish(None, None);
            }
            CedVerdict::Degraded(status) => {
                if let Some(detail) = status.detail() {
                    log::warn!("{detail}");
                }
                self.finish(None, Some(status));
            }
            CedVerdict::Ready { library, .. } => match fresh_model_check(os, arch) {
                Ok(model) => {
                    self.library = library;
                    self.model = model;
                    self.phase = TagPhase::Ready;
                }
                Err(status) => {
                    if let Some(detail) = status.detail() {
                        log::warn!("{detail}");
                    }
                    self.finish(None, Some(status));
                }
            },
        }
    }

    fn absorb(&mut self, step: Result<SoundTagWindows, SoundTagFailure>) {
        match step {
            Err(SoundTagFailure::Prepare(error)) => {
                log::warn!("sound tagging could not prepare audio: {error}");
                self.finish(None, None);
            }
            Err(SoundTagFailure::Run(error)) => {
                log::warn!("sound tagging failed: {error}");
                self.finish(None, None);
            }
            Ok(SoundTagWindows::Rejected) => self.finish(None, None),
            Ok(SoundTagWindows::Accepted(windows)) => self.accepted.extend(windows),
        }
    }

    fn finish(&mut self, tags: Option<Value>, status: Option<CapabilityStatus>) {
        self.accepted.clear();
        self.phase = TagPhase::Finished { tags, status };
    }

    fn publication(self) -> (Option<Value>, Option<CapabilityStatus>) {
        match self.phase {
            TagPhase::Finished { tags, status } => (tags, status),
            TagPhase::Pending => (None, None),
            TagPhase::Ready => {
                if self.accepted.is_empty() {
                    (None, None)
                } else {
                    (Some(fold_sound_tag_windows(&self.accepted)), None)
                }
            }
        }
    }
}

fn probe_ced_in_process(library: &Path, model: &Path) -> Result<(), String> {
    let request = json!({
        "schema": solstone_core_ced_analyze::PROBE_REQUEST_SCHEMA,
        "models": {
            "ced_library_path": library,
            "ced_model_path": model,
        },
    });
    let text = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    match solstone_core_ced_analyze::run_probe_request(&text) {
        Ok(response) if response.get("ok") == Some(&Value::Bool(true)) => Ok(()),
        Ok(response) => Err(format!(
            "ced probe helper returned an unexpected response: {response}"
        )),
        Err(error) => Err(error.detail()),
    }
}

fn ensure_sound_tag_ready(job: &mut EmbeddedJob<'_>) -> Result<(), WalkStop> {
    if !matches!(job.sound_tags.phase, TagPhase::Pending) {
        return Ok(());
    }
    job.embedded.checkpoint().map_err(WalkStop::Bare)?;
    #[cfg(test)]
    if ced_stub_on() {
        CED_PROBES.with(|cell| cell.set(cell.get() + 1));
        job.embedded.checkpoint().map_err(WalkStop::Bare)?;
        job.sound_tags.phase = TagPhase::Ready;
        return Ok(());
    }
    let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    let verdict =
        evaluate_ced_readiness_with_probe(job.journal_path, os, arch, probe_ced_in_process);
    job.embedded.checkpoint().map_err(WalkStop::Bare)?;
    job.sound_tags.apply_verdict(os, arch, verdict);
    Ok(())
}

fn tag_unit(
    job: &mut EmbeddedJob<'_>,
    masked: &[f32],
    origin: usize,
    spans: &[(usize, usize)],
) -> Result<(), WalkStop> {
    if spans.is_empty() || !job.sound_tags.accepting() {
        return Ok(());
    }
    ensure_sound_tag_ready(job)?;
    if !job.sound_tags.accepting() {
        return Ok(());
    }
    job.embedded.checkpoint().map_err(WalkStop::Bare)?;
    let step = classify_sound_tag_unit(job, masked, origin, spans);
    job.embedded.checkpoint().map_err(WalkStop::Bare)?;
    job.sound_tags.absorb(step);
    Ok(())
}

fn classify_sound_tag_unit(
    job: &EmbeddedJob<'_>,
    masked: &[f32],
    origin: usize,
    spans: &[(usize, usize)],
) -> Result<SoundTagWindows, SoundTagFailure> {
    let view = unit_classify_view(masked, origin, spans);
    #[cfg(test)]
    if ced_stub_on() {
        CED_CLASSIFIES.with(|cell| cell.set(cell.get() + 1));
        CED_SEEN.with(|cell| {
            cell.borrow_mut()
                .push((view.samples.len(), view.windows.clone()))
        });
        let response = json!({
            "schema": solstone_core_ced_analyze::RESPONSE_SCHEMA,
            "windows": view
                .windows
                .iter()
                .map(|_| json!({"ok": true, "tags": {"Tick": 0.2}}))
                .collect::<Vec<_>>(),
        });
        return Ok(read_sound_tag_windows(&response, view.windows.len()));
    }
    let temporary = tempfile::Builder::new()
        .prefix(solstone_core_sound_tags::CED_ANALYZE_TEMP_PREFIX)
        .tempdir_in(job.embedded.scratch)
        .map_err(|error| SoundTagFailure::Prepare(error.to_string()))?;
    let audio_path = temporary.path().join("audio.f32le");
    write_f32le(&audio_path, view.samples)
        .map_err(|error| SoundTagFailure::Prepare(error.to_string()))?;
    let request = json!({
        "schema": solstone_core_ced_analyze::REQUEST_SCHEMA,
        "models": {
            "ced_library_path": &job.sound_tags.library,
            "ced_model_path": &job.sound_tags.model,
        },
        "audio_f32le_path": &audio_path,
        "sample_rate_hz": solstone_core_sound_tags::CLASSIFY_SAMPLE_RATE,
        "top_k": solstone_core_sound_tags::TOP_K,
        "windows": view
            .windows
            .iter()
            .map(|(start, end)| json!({"start_sample": start, "end_sample": end}))
            .collect::<Vec<_>>(),
    });
    let text =
        serde_json::to_string(&request).map_err(|error| SoundTagFailure::Run(error.to_string()))?;
    let result = solstone_core_ced_analyze::run_classify_request(&text)
        .map_err(|error| SoundTagFailure::Run(error.to_string()));
    if let Err(error) = temporary.close() {
        log::warn!("sound tagging could not prepare audio: {error}");
    }
    Ok(read_sound_tag_windows(&result?, view.windows.len()))
}

fn write_f32le(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(samples.len() * std::mem::size_of::<f32>());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes)
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn note_commit(record: bool, start: usize, end: usize) {
    if !record {
        return;
    }
    #[cfg(all(test, feature = "full-tests"))]
    EMBEDDED_UNIT_COMMITS.with(|cell| cell.borrow_mut().push((start, end)));
    #[cfg(not(all(test, feature = "full-tests")))]
    let _ = (start, end);
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static EMBEDDED_VAD_STICKY: std::cell::RefCell<Option<fn(usize) -> VadResult>> =
        const { std::cell::RefCell::new(None) };
    static EMBEDDED_VAD_REPLAY: std::cell::RefCell<Option<VadResult>> =
        const { std::cell::RefCell::new(None) };
    static EMBEDDED_UNIT_COMMITS: std::cell::RefCell<Vec<(usize, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Clears a one-shot VAD replay at the end of one recording. It does not touch the test hook.
#[cfg(all(test, feature = "full-tests"))]
struct VadReplayGuard;

#[cfg(all(test, feature = "full-tests"))]
impl VadReplayGuard {
    fn arm() -> Self {
        EMBEDDED_VAD_REPLAY.with(|cell| *cell.borrow_mut() = None);
        Self
    }
}

#[cfg(all(test, feature = "full-tests"))]
impl Drop for VadReplayGuard {
    fn drop(&mut self) {
        EMBEDDED_VAD_REPLAY.with(|cell| *cell.borrow_mut() = None);
    }
}

#[cfg(test)]
thread_local! {
    static CED_STUB_ON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CED_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CED_CLASSIFIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CED_SEEN: std::cell::RefCell<Vec<(usize, Vec<(usize, usize)>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn ced_stub_on() -> bool {
    CED_STUB_ON.with(|cell| cell.get())
}

#[cfg(test)]
fn install_ced_stub() {
    CED_STUB_ON.with(|cell| cell.set(true));
    CED_PROBES.with(|cell| cell.set(0));
    CED_CLASSIFIES.with(|cell| cell.set(0));
    CED_SEEN.with(|cell| cell.borrow_mut().clear());
}

#[cfg(test)]
fn clear_ced_stub() {
    CED_STUB_ON.with(|cell| cell.set(false));
    CED_PROBES.with(|cell| cell.set(0));
    CED_CLASSIFIES.with(|cell| cell.set(0));
    CED_SEEN.with(|cell| cell.borrow_mut().clear());
}

#[cfg(all(test, feature = "full-tests"))]
fn ced_stub_counts() -> (usize, usize) {
    (
        CED_PROBES.with(|cell| cell.get()),
        CED_CLASSIFIES.with(|cell| cell.get()),
    )
}

#[cfg(all(test, feature = "full-tests"))]
fn ced_stub_seen() -> Vec<(usize, Vec<(usize, usize)>)> {
    CED_SEEN.with(|cell| cell.borrow().clone())
}

/// Installs the non-spawning ced probe and classify stand-in for one test.
#[cfg(test)]
pub(crate) struct EmbeddedCedStubGuard;

#[cfg(test)]
impl EmbeddedCedStubGuard {
    pub(crate) fn install() -> Self {
        install_ced_stub();
        Self
    }
}

#[cfg(test)]
impl Drop for EmbeddedCedStubGuard {
    fn drop(&mut self) {
        clear_ced_stub();
    }
}

/// Full-tests stand-in for per-unit VAD. The hook stays until cleared.
#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn set_embedded_vad_sticky(hook: Option<fn(usize) -> VadResult>) {
    EMBEDDED_VAD_STICKY.with(|cell| *cell.borrow_mut() = hook);
}

/// Commit spans recorded by the transcribe walk, then cleared.
#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn take_embedded_unit_commits() -> Vec<(usize, usize)> {
    EMBEDDED_UNIT_COMMITS.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f64, end: f64) -> TranscriptionWord {
        TranscriptionWord {
            word: text.to_owned(),
            start,
            end,
            probability: None,
        }
    }

    #[test]
    fn plan_covers_step_seam_partial_and_exact_multiple() {
        assert_eq!(EMBEDDED_SEAM_SAMPLES, 160_000);
        assert_eq!(EMBEDDED_STEP_SAMPLES, 1_760_000);
        assert_eq!(EMBEDDED_AUDIO_MAX_SAMPLES, 1_920_000);
        assert!(plan_embedded_units(0).is_empty());
        assert_eq!(plan_embedded_units(1_760_000), vec![(0, 1_760_000)]);
        assert_eq!(
            plan_embedded_units(1_920_000),
            vec![(0, 1_760_000), (1_600_000, 1_920_000)]
        );
        assert_eq!(
            plan_embedded_units(1_760_000 + 10),
            vec![(0, 1_760_000), (1_600_000, 1_760_000 + 10)]
        );
        let exact = plan_embedded_units(1_760_000 * 2);
        assert_eq!(exact, vec![(0, 1_760_000), (1_600_000, 3_520_000)]);
        assert!(exact.iter().all(|(start, end)| end > start));
        for n in [
            1usize, 1_760_000, 1_760_001, 1_920_000, 3_520_000, 8_480_000,
        ] {
            let units = plan_embedded_units(n);
            assert_eq!(units.first().map(|unit| unit.0), Some(0), "{n}");
            assert_eq!(units.last().map(|unit| unit.1), Some(n), "{n}");
            let mut committed = 0;
            for (index, (start, end)) in units.iter().enumerate() {
                assert!(end - start <= EMBEDDED_AUDIO_MAX_SAMPLES, "{n}");
                let expected = if index == 0 {
                    0
                } else {
                    committed - EMBEDDED_SEAM_SAMPLES
                };
                assert_eq!(*start, expected, "{n}");
                committed = *end;
            }
            assert_eq!(committed, n);
        }
    }

    #[test]
    fn tag_grid_aligns_and_skips_audio_under_ten_seconds() {
        assert_eq!(crate::owner_voice::PAD_BEFORE_SAMPLES, 8_000);
        for samples in [0_usize, 1, 16_000, 159_999] {
            let plan = plan_tag_windows(0, samples, samples, false);
            assert!(plan.spans.is_empty(), "{samples}");
            assert!(plan.deferred_seam.is_none(), "{samples}");
            assert!(solstone_core_sound_tags::window_spans(samples).is_empty());
        }
        for samples in [160_000_usize, 176_000, 320_000, 1_809_999] {
            let plan = plan_tag_windows(0, samples, samples, false);
            assert!(plan.deferred_seam.is_none(), "{samples}");
            assert_eq!(
                plan.spans,
                solstone_core_sound_tags::window_spans(samples),
                "{samples}"
            );
        }
    }

    #[test]
    fn full_unit_defers_the_seam_and_the_successor_emits_it_once() {
        let step = 1_760_000;
        let window = 160_000;
        let first = plan_tag_windows(0, step, step - 8_000, true);
        let expected = (0..10)
            .map(|k| (k * window, (k + 1) * window))
            .collect::<Vec<_>>();
        assert_eq!(first.spans, expected);
        assert_eq!(first.deferred_seam, Some((1_600_000, step)));
        assert!(!first.spans.contains(&(1_600_000, step)));

        let origin = step - window;
        let audio_end = step + step;
        let successor = plan_tag_windows(origin, audio_end, audio_end - 8_000, true);
        let seam = (1_600_000, step);
        assert_eq!(
            successor.spans.iter().filter(|span| **span == seam).count(),
            1
        );
        assert_eq!(successor.spans[0], seam);
    }

    #[test]
    fn exact_step_end_emits_the_deferred_seam_once_and_no_tail() {
        let n = 1_760_000;
        let first = plan_tag_windows(0, n, n - 8_000, true);
        let seam = first.deferred_seam.expect("full unit defers the seam");
        assert_eq!(seam, (1_600_000, n));
        let mut emitted = first.spans;
        emitted.push(seam);
        assert_eq!(emitted, solstone_core_sound_tags::window_spans(n));
        assert_eq!(emitted.iter().filter(|span| **span == seam).count(), 1);
        assert_eq!(n % 160_000, 0);
        assert_eq!(emitted.last().copied(), Some(seam));
    }

    #[test]
    fn final_partial_unit_emits_the_tail_once() {
        let step = 1_760_000;
        let n = 1_810_000;
        let first = plan_tag_windows(0, step, step - 8_000, true);
        assert_eq!(first.deferred_seam, Some((1_600_000, step)));
        let origin = step - 160_000;
        let partial = plan_tag_windows(origin, n, n, false);
        assert!(partial.deferred_seam.is_none());
        let seam = (1_600_000, 1_760_000);
        let tail = (1_650_000, n);
        assert_ne!(seam, tail);
        assert_eq!(
            partial.spans.iter().filter(|span| **span == seam).count(),
            1
        );
        assert_eq!(
            partial.spans.iter().filter(|span| **span == tail).count(),
            1
        );
        let mut emitted = first.spans;
        emitted.extend(partial.spans);
        assert_eq!(emitted, solstone_core_sound_tags::window_spans(n));
    }

    #[test]
    fn stub_classify_sees_unit_local_slices_only() {
        let recording = 1_810_000;
        let step = 1_760_000;
        let window = 160_000;
        let first_mask = vec![0.5_f32; step];
        let first = plan_tag_windows(0, step, step - 8_000, true);
        let view = unit_classify_view(&first_mask, 0, &first.spans);
        assert_eq!(view.samples.len(), step);
        assert_ne!(view.samples.len(), recording);
        assert!(
            view.windows
                .iter()
                .all(|(start, end)| *start < *end && *end <= view.samples.len())
        );
        assert_eq!(view.windows.len(), 10);
        assert!(view.windows.iter().all(|(_, end)| *end <= 1_600_000));

        let (start, end) = first.deferred_seam.expect("seam");
        let saved = &first_mask[first_mask.len() - (end - start)..];
        let seam_view = unit_classify_view(saved, start, &[(start, end)]);
        assert_eq!(seam_view.samples.len(), window);
        assert_eq!(seam_view.windows, vec![(0, window)]);
        assert_ne!(seam_view.samples.len(), recording);

        let origin = step - window;
        let unit_len = recording - origin;
        let mut mask = vec![0.25_f32; unit_len];
        mask[..16_000].fill(0.0);
        let partial = plan_tag_windows(origin, recording, recording, false);
        let view = unit_classify_view(&mask, origin, &partial.spans);
        assert_eq!(view.samples.len(), 210_000);
        assert_ne!(view.samples.len(), recording);
        assert!(view.samples[..16_000].iter().all(|sample| *sample == 0.0));
        assert!(
            view.windows
                .iter()
                .all(|(start, end)| *start < *end && *end <= view.samples.len())
        );
        assert!(view.windows.contains(&(0, window)));
        assert!(view.windows.contains(&(50_000, 210_000)));
    }

    #[test]
    fn seam_words_are_kept_once_at_restored_times() {
        let reduction = AudioReduction {
            segments: vec![solstone_core_observe_audio::SpeechSegment {
                original_start_s: 5.0,
                original_end_s: 6.0,
                reduced_start_s: 0.0,
                reduced_end_s: 1.0,
            }],
            original_duration_s: 6.0,
            reduced_duration_s: 1.0,
        };
        let first = place_unit_words(
            &[word("seam.", 0.0, 0.4)],
            Some(&reduction),
            0,
            0,
            usize::MAX,
        );
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].start, 5.0);
        assert_eq!(first[0].end, 5.4);
        let overlap = place_unit_words(
            &[word("seam.", 0.0, 0.4)],
            Some(&reduction),
            1_600_000,
            1_760_000,
            usize::MAX,
        );
        assert!(overlap.is_empty());
        let boundary = place_unit_words(
            &[word("edge.", 10.0, 10.2)],
            None,
            1_600_000,
            1_760_000,
            usize::MAX,
        );
        assert_eq!(boundary.len(), 1);
        assert_eq!(boundary[0].start, 110.0);
        assert_eq!(boundary[0].end, 110.2);
        let merged = merge_unit_words([first, overlap, boundary]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].word, "seam.");
        assert_eq!(merged[0].start, 5.0);
        assert_eq!(merged[1].word, "edge.");
        assert_eq!(merged[1].start, 110.0);
    }

    #[test]
    fn masked_unit_reaches_the_recognizer_with_unresolved_zeros() {
        let print = axis(0);
        let audio = vec![0.25_f32; 6 * 16_000];
        let vad = speech_vad(audio.len());
        let step = crate::owner_voice::filter_audio(
            Path::new("unit.wav"),
            audio,
            &vad,
            &print,
            |_audio, statements| {
                let ids = statements
                    .iter()
                    .map(|statement| statement["id"].as_i64().expect("id"))
                    .collect::<Vec<_>>();
                assert_eq!(ids.len(), 3);
                Ok(first_pass_result(&[
                    (ids[0], axis(0)),
                    (ids[1], axis(1)),
                    (ids[2], axis(1)),
                ]))
            },
        )
        .expect("filtered");
        let seen = std::cell::RefCell::new(Vec::new());
        transcribe_prepared_unit(&step.audio, &vad, |samples| {
            assert_eq!(samples, step.audio.as_slice());
            *seen.borrow_mut() = samples.to_vec();
            Ok((
                TranscriptionResponse {
                    words: Vec::new(),
                    text: String::new(),
                },
                ModelInfo {
                    model: "stub".to_owned(),
                    device: "cpu".to_owned(),
                    compute_type: "f32".to_owned(),
                },
            ))
        })
        .expect("prepared");
        let seen = seen.into_inner();
        assert!(seen[..3 * 16_000].iter().all(|sample| *sample == 0.25));
        assert!(seen[4 * 16_000..].iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn confidential_over_budget_sends_nothing_and_under_budget_sends_each_unit() {
        let mut sends = 0;
        dispatch_confidential_with_cap(16_000, 1.0, || {
            for count in [8_000_usize, 8_000] {
                assert!(count <= EMBEDDED_AUDIO_MAX_SAMPLES);
                sends += 1;
            }
            Ok::<(), TranscribeError>(())
        })
        .expect("under budget");
        assert_eq!(sends, 2);

        let mut width = 1_usize;
        while crate::backend::confidential::confidential_request_fits(width.saturating_mul(2)) {
            width *= 2;
        }
        let over = width.saturating_mul(2);
        assert!(crate::backend::confidential::confidential_request_fits(
            width
        ));
        assert!(!crate::backend::confidential::confidential_request_fits(
            over
        ));
        let mut sends = 0;
        let error = dispatch_confidential_with_cap(over, 1.0, || {
            for count in [width, width] {
                let _ = count;
                sends += 1;
            }
            Ok::<(), TranscribeError>(())
        })
        .expect_err("over budget");
        assert_eq!(sends, 0);
        assert!(matches!(
            error,
            TranscribeError::ConfidentialDeferred { ref reason, .. }
                if reason == "confidential_audio_too_long"
        ));
    }

    fn speech_vad(samples: usize) -> VadResult {
        let duration_s = samples as f64 / f64::from(SAMPLE_RATE);
        VadResult {
            duration_s,
            speech_duration_s: duration_s,
            has_speech: samples > 0,
            speech_segments: vec![(0.0, duration_s)],
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        }
    }

    fn axis(index: usize) -> Vec<f32> {
        let mut values = vec![0.0; 256];
        values[index] = 1.0;
        values
    }

    fn first_pass_result(rows: &[(i64, Vec<f32>)]) -> SpeakerAnalyzeResult {
        let payload = rows
            .iter()
            .flat_map(|(_, values)| values.iter().copied().flat_map(f32::to_le_bytes))
            .collect::<Vec<_>>();
        SpeakerAnalyzeResult {
            statements: Vec::new(),
            embedding_payload: Some(SpeakerEmbeddingPayload {
                payload,
                statement_ids: rows.iter().map(|(id, _)| *id).collect(),
                durations_s: rows.iter().map(|_| 2.0).collect(),
                encoder: EMBEDDING_ENCODER.to_owned(),
            }),
            speaker_evidence: "single".to_owned(),
            speaker_evidence_multi_fraction: 0.0,
            overlap_fraction: 0.0,
            statement_labels: None,
            turn_ranges: Some(Vec::new()),
            overlap_ranges: Some(Vec::new()),
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    struct ClearEmbeddedHooks;

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    impl ClearEmbeddedHooks {
        fn arm() -> Self {
            install_ced_stub();
            Self
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    impl Drop for ClearEmbeddedHooks {
        fn drop(&mut self) {
            set_embedded_vad_sticky(None);
            crate::speakers::set_embedded_unit_speaker_empty(false);
            crate::speakers::clear_embedded_speaker_responses();
            crate::speakers::set_embedded_first_pass_keep(None);
            crate::backend::confidential::set_confidential_send_hook(false);
            let _ = take_embedded_unit_commits();
            clear_ced_stub();
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn stub_model() -> crate::embedded::EmbeddedModelInfo {
        crate::embedded::EmbeddedModelInfo {
            backend: "parakeet-cpp".into(),
            model: "stub".into(),
            device: "cpu".into(),
            compute_type: "f32".into(),
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn sentence_json() -> Value {
        json!({
            "text": "a.",
            "words": [{"word": "a.", "start": 0.0, "end": 0.2}]
        })
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    struct LengthAsr(std::cell::RefCell<Vec<usize>>);

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    impl crate::embedded::EmbeddedAsr for LengthAsr {
        fn transcribe(&self, samples: &[f32]) -> Result<Value, String> {
            assert!(samples.len() <= EMBEDDED_AUDIO_MAX_SAMPLES);
            self.0.borrow_mut().push(samples.len());
            Ok(sentence_json())
        }
        fn model_info(&self) -> crate::embedded::EmbeddedModelInfo {
            stub_model()
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    struct CancelAsr<'a>(&'a std::sync::atomic::AtomicBool);

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    impl crate::embedded::EmbeddedAsr for CancelAsr<'_> {
        fn transcribe(&self, samples: &[f32]) -> Result<Value, String> {
            assert!(samples.len() <= EMBEDDED_AUDIO_MAX_SAMPLES);
            self.0.store(true, std::sync::atomic::Ordering::Release);
            Ok(sentence_json())
        }
        fn model_info(&self) -> crate::embedded::EmbeddedModelInfo {
            stub_model()
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    struct FailingAsr;

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    impl crate::embedded::EmbeddedAsr for FailingAsr {
        fn transcribe(&self, samples: &[f32]) -> Result<Value, String> {
            assert!(samples.len() <= EMBEDDED_AUDIO_MAX_SAMPLES);
            Err("stub-failed".into())
        }
        fn model_info(&self) -> crate::embedded::EmbeddedModelInfo {
            stub_model()
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn write_pcm_wav(root: &Path, segment: &str, samples: usize) -> (std::path::PathBuf, Vec<u8>) {
        let dir = root.join("chronicle/20261012/audio").join(segment);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("audio.wav");
        let bytes =
            solstone_core_observe_audio::audio_to_wav_bytes(&vec![0.0; samples], SAMPLE_RATE)
                .unwrap();
        std::fs::write(&audio, &bytes).unwrap();
        (audio, bytes)
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn absent_budget(error: &TranscribeError) {
        let rendered = error.to_string();
        assert!(!rendered.contains("embedded-input-budget"), "{rendered}");
        assert!(!rendered.contains("embedded-duration-budget"), "{rendered}");
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn long_wav_units_cover_the_file_without_a_budget_hold() {
        let _hooks = ClearEmbeddedHooks::arm();
        let _ = take_embedded_unit_commits();
        set_embedded_vad_sticky(Some(speech_vad));
        crate::speakers::set_embedded_unit_speaker_empty(true);
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let n = 530 * 16_000;
        let (audio, bytes) = write_pcm_wav(root.path(), "120000_121", n);
        let mut decoder = UnitDecoder::open(&audio).unwrap();
        let mut pulled = Vec::new();
        while let Some(chunk) = decoder.pull(EMBEDDED_STEP_SAMPLES).unwrap() {
            if chunk.is_empty() {
                break;
            }
            pulled.extend(chunk);
        }
        assert_eq!(
            pulled,
            solstone_core_observe_audio::decode_f32_mono(&audio).unwrap()
        );
        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        let deleted = std::cell::Cell::new(false);
        let file = crate::embedded::process_embedded(
            &audio,
            root.path(),
            Path::new("missing-models"),
            &root.path().join("scratch"),
            &asr,
            crate::embedded::EmbeddedControl {
                owner_original_deleted: &|| deleted.set(true),
                cancelled: &|| false,
                service_io: &|_| {},
            },
            None,
        )
        .unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Transcribed);
        let lengths = asr.0.into_inner();
        let expected = [1_760_000_usize, 1_920_000, 1_920_000, 1_920_000, 1_600_000];
        assert_eq!(lengths.as_slice(), &expected);
        assert!(lengths.iter().all(|len| *len <= EMBEDDED_AUDIO_MAX_SAMPLES));
        let planned = plan_embedded_units(n)
            .into_iter()
            .map(|(start, end)| end - start)
            .collect::<Vec<_>>();
        assert_eq!(planned, expected);
        let commits = take_embedded_unit_commits();
        assert_eq!(
            commits,
            vec![
                (0, 1_752_000),
                (1_752_000, 3_512_000),
                (3_512_000, 5_272_000),
                (5_272_000, 7_032_000),
                (7_032_000, 8_480_000),
            ]
        );
        assert_eq!(std::fs::read(&audio).unwrap(), bytes);
        assert!(audio.with_extension("jsonl").is_file());
        assert!(!deleted.get());
        let record = read_processing_record_header(&audio.with_extension("jsonl")).unwrap();
        if let Some(reason) = record["reason_code"].as_str() {
            assert_ne!(reason, "embedded-input-budget");
            assert_ne!(reason, "embedded-duration-budget");
        }
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        eprintln!(
            "peak_rss_mib={} vmhwm_present={}",
            crate::stage::peak_rss_mib(),
            status.lines().any(|line| line.starts_with("VmHWM:"))
        );
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn cancel_and_failure_leave_the_original_and_do_not_delete_it() {
        let _hooks = ClearEmbeddedHooks::arm();
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let (audio, bytes) = write_pcm_wav(root.path(), "120000_121", 16_000);
        set_embedded_vad_sticky(Some(speech_vad));
        crate::speakers::set_embedded_unit_speaker_empty(true);
        let flag = std::sync::atomic::AtomicBool::new(false);
        let deleted = std::cell::Cell::new(false);
        let cancel = || flag.load(std::sync::atomic::Ordering::Acquire);
        let error = crate::embedded::process_embedded(
            &audio,
            root.path(),
            Path::new("missing-models"),
            &root.path().join("scratch"),
            &CancelAsr(&flag),
            crate::embedded::EmbeddedControl {
                owner_original_deleted: &|| deleted.set(true),
                cancelled: &cancel,
                service_io: &|_| {},
            },
            None,
        )
        .expect_err("cancel between the native call and publication");
        assert!(matches!(error, TranscribeError::Cancelled), "{error:?}");
        absent_budget(&error);
        assert_eq!(std::fs::read(&audio).unwrap(), bytes);
        assert!(!audio.with_extension("jsonl").exists());
        assert!(!deleted.get());

        set_embedded_vad_sticky(None);
        let (bare, bare_bytes) = write_pcm_wav(root.path(), "120000_122", 16_000);
        let missing = crate::embedded::process_embedded(
            &bare,
            root.path(),
            Path::new("missing-models"),
            &root.path().join("scratch"),
            &FailingAsr,
            crate::embedded::EmbeddedControl {
                owner_original_deleted: &|| deleted.set(true),
                cancelled: &|| false,
                service_io: &|_| {},
            },
            None,
        )
        .expect_err("missing VAD model");
        assert!(
            matches!(missing, TranscribeError::ModelAsset(_)),
            "{missing:?}"
        );
        absent_budget(&missing);
        assert_eq!(std::fs::read(&bare).unwrap(), bare_bytes);
        assert!(!bare.with_extension("jsonl").exists());
        assert!(!deleted.get());

        set_embedded_vad_sticky(Some(speech_vad));
        let (failed, failed_bytes) = write_pcm_wav(root.path(), "120000_123", 16_000);
        let inference = crate::embedded::process_embedded(
            &failed,
            root.path(),
            Path::new("missing-models"),
            &root.path().join("scratch"),
            &FailingAsr,
            crate::embedded::EmbeddedControl {
                owner_original_deleted: &|| deleted.set(true),
                cancelled: &|| false,
                service_io: &|_| {},
            },
            None,
        )
        .expect_err("stub recognizer");
        assert!(
            matches!(
                inference,
                TranscribeError::ParakeetCppFailure { ref reason, .. } if reason == "embedded-inference"
            ),
            "{inference:?}"
        );
        absent_budget(&inference);
        assert_eq!(std::fs::read(&failed).unwrap(), failed_bytes);
        let record = read_processing_record_header(&failed.with_extension("jsonl")).unwrap();
        assert_eq!(record["state"], "failed");
        assert_eq!(record["reason_code"], "embedded-inference");
        assert_eq!(record["attempts"], 1);
        assert!(!solstone_core_processing_record::is_failure_exhausted(
            &record
        ));
        assert!(!failed.with_extension("npz").exists());
        assert!(!deleted.get());

        let (held_audio, held_bytes) = write_pcm_wav(root.path(), "120000_124", 16_000);
        let held_jsonl = held_audio.with_extension("jsonl");
        let seeded = json!({
            "raw": "audio.wav",
            "_solstone_processing": {
                "schema": vocab::SCHEMA,
                "state": "analyzed",
                "handler": vocab::HANDLER_TRANSCRIBE,
                "input_size": held_bytes.len(),
            }
        });
        let seeded_text = format!("{seeded}\n");
        std::fs::write(&held_jsonl, &seeded_text).unwrap();
        let config = JournalConfigRead {
            present: false,
            sha256: None,
            config: None,
        };
        let attestation = AttestationStateStore::new();
        let context = crate::embedded::EmbeddedContext {
            models: Path::new("missing-models"),
            scratch: &root.path().join("scratch"),
            asr: &FailingAsr,
            cancel: &|| false,
            service_io: &|_| {},
            owner_original_deleted: &|| deleted.set(true),
        };
        let held_error = crate::stage::process_one_with(
            &held_audio,
            root.path(),
            true,
            None,
            &config,
            &attestation,
            Some(&context),
            None,
        )
        .expect_err("held publication blocks the failure writer");
        assert!(
            matches!(
                held_error,
                TranscribeError::ParakeetCppFailure { ref reason, .. } if reason == "embedded-inference"
            ),
            "{held_error:?}"
        );
        absent_budget(&held_error);
        assert_eq!(std::fs::read(&held_audio).unwrap(), held_bytes);
        assert_eq!(std::fs::read_to_string(&held_jsonl).unwrap(), seeded_text);
        assert!(!deleted.get());
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn timed_vad(len: usize, segments: Vec<(f64, f64)>) -> VadResult {
        let duration_s = len as f64 / f64::from(SAMPLE_RATE);
        VadResult {
            duration_s,
            speech_duration_s: segments.iter().map(|(start, end)| end - start).sum(),
            has_speech: !segments.is_empty(),
            speech_segments: segments,
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn min_speech_kept(len: usize) -> VadResult {
        let segments = match len {
            1_760_000 => vec![(0.0, 0.5)],
            168_000 => vec![(10.0, 10.5)],
            other => panic!("unexpected unit length {other}"),
        };
        timed_vad(len, segments)
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn min_speech_under_split(len: usize) -> VadResult {
        let segments = match len {
            1_760_000 => vec![(0.0, 0.4)],
            168_000 => vec![(10.0, 10.4)],
            other => panic!("unexpected unit length {other}"),
        };
        timed_vad(len, segments)
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn min_speech_seam_trap(len: usize) -> VadResult {
        let segments = match len {
            1_760_000 => vec![(109.2, 109.8)],
            168_000 => vec![(9.2, 9.8)],
            other => panic!("unexpected unit length {other}"),
        };
        timed_vad(len, segments)
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn min_speech_one_short(len: usize) -> VadResult {
        timed_vad(len, vec![(0.0, 0.4)])
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn confidential_under_vad(len: usize) -> VadResult {
        let segments = match len {
            1_760_000 => vec![(1.0, 2.0), (6.0, 7.0)],
            320_000 => vec![(10.0, 11.0), (15.0, 16.0)],
            other => panic!("unexpected unit length {other}"),
        };
        timed_vad(len, segments)
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    struct ScriptAsr {
        seen: std::cell::RefCell<Vec<Vec<f32>>>,
        script: fn(usize) -> Value,
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    impl crate::embedded::EmbeddedAsr for ScriptAsr {
        fn transcribe(&self, samples: &[f32]) -> Result<Value, String> {
            assert!(samples.len() <= EMBEDDED_AUDIO_MAX_SAMPLES);
            self.seen.borrow_mut().push(samples.to_vec());
            Ok((self.script)(samples.len()))
        }
        fn model_info(&self) -> crate::embedded::EmbeddedModelInfo {
            stub_model()
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    struct SwitchAsr {
        calls: std::cell::Cell<usize>,
        fail_at: usize,
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    impl crate::embedded::EmbeddedAsr for SwitchAsr {
        fn transcribe(&self, samples: &[f32]) -> Result<Value, String> {
            assert!(samples.len() <= EMBEDDED_AUDIO_MAX_SAMPLES);
            let call = self.calls.get() + 1;
            self.calls.set(call);
            if call == self.fail_at {
                Err("later-window".into())
            } else {
                Ok(sentence_json())
            }
        }
        fn model_info(&self) -> crate::embedded::EmbeddedModelInfo {
            stub_model()
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    struct PanicAsr;

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    impl crate::embedded::EmbeddedAsr for PanicAsr {
        fn transcribe(&self, _samples: &[f32]) -> Result<Value, String> {
            panic!("confidential admission must not fall through to local recognition");
        }
        fn model_info(&self) -> crate::embedded::EmbeddedModelInfo {
            stub_model()
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn write_level_wav(
        root: &Path,
        segment: &str,
        samples: usize,
        level: f32,
    ) -> (std::path::PathBuf, Vec<u8>) {
        let dir = root.join("chronicle/20261012/audio").join(segment);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("audio.wav");
        let bytes =
            solstone_core_observe_audio::audio_to_wav_bytes(&vec![level; samples], SAMPLE_RATE)
                .unwrap();
        std::fs::write(&audio, &bytes).unwrap();
        (audio, bytes)
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn write_config(root: &Path, body: &str) {
        let dir = root.join("config");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("journal.json"), body).unwrap();
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn run_file(
        audio: &Path,
        root: &Path,
        asr: &dyn crate::embedded::EmbeddedAsr,
        cancel: &dyn Fn() -> bool,
    ) -> Result<crate::TranscribeFile, TranscribeError> {
        let deleted = std::cell::Cell::new(false);
        crate::embedded::process_embedded(
            audio,
            root,
            Path::new("missing-models"),
            &root.join("scratch"),
            asr,
            crate::embedded::EmbeddedControl {
                owner_original_deleted: &|| deleted.set(true),
                cancelled: cancel,
                service_io: &|_| {},
            },
            None,
        )
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn todo_names(root: &Path) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(root.join("indexer/todo")) {
            for entry in entries.flatten() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        names
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn transcript_words(path: &Path) -> Vec<String> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut words = Vec::new();
        for line in text.lines().skip(1) {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(list) = value.get("words").and_then(Value::as_array) else {
                continue;
            };
            for word in list {
                if let Some(text) = word.get("word").and_then(Value::as_str) {
                    words.push(text.to_owned());
                }
            }
        }
        words
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn write_f32_rows(path: &Path, rows: &[Vec<f32>]) {
        let mut bytes = Vec::new();
        for row in rows {
            for value in row {
                bytes.extend(value.to_le_bytes());
            }
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn push_helper(
        dir: &Path,
        tag: &str,
        audio_len: usize,
        overlap: f64,
        multi_every: bool,
        multi_index: Option<usize>,
        intervals: &[(f64, f64, usize)],
        statement_ids: &[i64],
        statement_axes: &[usize],
        turns: Option<Vec<(f64, f64)>>,
    ) {
        let stats_len = crate::speakers::pyannote_window_starts(audio_len).len();
        let stats = (0..stats_len)
            .map(|index| {
                let multi = multi_every || multi_index == Some(index);
                json!({
                    "speech_frames": 10,
                    "active_slot_count": if multi { 2 } else { 1 },
                    "overlap_frames": 0
                })
            })
            .collect::<Vec<_>>();
        let statement_path = dir.join(format!("{tag}-statements.f32le"));
        write_f32_rows(
            &statement_path,
            &statement_axes
                .iter()
                .map(|axis_index| axis(*axis_index))
                .collect::<Vec<_>>(),
        );
        let interval_embeddings = if intervals.is_empty() {
            Value::Null
        } else {
            let interval_path = dir.join(format!("{tag}-intervals.f32le"));
            write_f32_rows(
                &interval_path,
                &intervals
                    .iter()
                    .map(|(_, _, axis_index)| axis(*axis_index))
                    .collect::<Vec<_>>(),
            );
            json!({
                "shape": [intervals.len(), 256],
                "payload_path": interval_path,
            })
        };
        let mut pyannote = serde_json::Map::new();
        pyannote.insert("window_stats".to_owned(), Value::Array(stats));
        if let Some(ranges) = turns {
            pyannote.insert(
                "speaker_turns".to_owned(),
                json!({
                    "version": "window-local-turns-v1",
                    "audio_duration_s": audio_len as f64 / f64::from(SAMPLE_RATE),
                    "ranges": ranges
                        .iter()
                        .map(|(start, end)| json!({"start_s": start, "end_s": end}))
                        .collect::<Vec<_>>(),
                }),
            );
        }
        pyannote.insert("overlap_ranges".to_owned(), json!([]));
        let response = json!({
            "diarization": {
                "interval_embeddings": interval_embeddings,
                "valid_intervals": intervals
                    .iter()
                    .map(|(start, end, _)| json!({"start_s": start, "end_s": end, "local_class": 0}))
                    .collect::<Vec<_>>(),
            },
            "statement_embeddings": {
                "statement_ids": statement_ids,
                "durations_s": vec![0.2; statement_ids.len()],
                "shape": [statement_ids.len(), 256],
                "payload_path": statement_path,
            },
            "evidence": {
                "speaker_evidence": "single",
                "multi_window_fraction": 0.0,
                "mean_window_overlap_share": 0.0,
                "overlap_fraction": overlap,
            },
            "pyannote": pyannote,
        });
        crate::speakers::push_embedded_speaker_response(response, statement_path);
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn recording_min_speech_keeps_split_speech_and_drops_the_short_total() {
        let _hooks = ClearEmbeddedHooks::arm();
        crate::speakers::set_embedded_unit_speaker_empty(true);
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        set_embedded_vad_sticky(Some(min_speech_kept));
        let (kept, _) = write_pcm_wav(root.path(), "120000_201", 1_768_000);
        let file = run_file(&kept, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Transcribed);
        assert_eq!(asr.0.borrow().len(), 2);

        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        set_embedded_vad_sticky(Some(min_speech_under_split));
        let (under, under_bytes) = write_pcm_wav(root.path(), "120000_202", 1_768_000);
        let file = run_file(&under, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Preserved);
        assert!(asr.0.borrow().is_empty());
        assert_eq!(std::fs::read(&under).unwrap(), under_bytes);

        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        set_embedded_vad_sticky(Some(min_speech_seam_trap));
        let (trap, trap_bytes) = write_pcm_wav(root.path(), "120000_203", 1_768_000);
        let file = run_file(&trap, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Preserved);
        assert!(asr.0.borrow().is_empty());
        assert_eq!(std::fs::read(&trap).unwrap(), trap_bytes);

        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        set_embedded_vad_sticky(Some(min_speech_one_short));
        let (one, one_bytes) = write_pcm_wav(root.path(), "120000_204", 16_000);
        let file = run_file(&one, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Preserved);
        assert!(asr.0.borrow().is_empty());
        assert_eq!(std::fs::read(&one).unwrap(), one_bytes);
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn owner_script(len: usize) -> Value {
        if len == 1_760_000 {
            json!({
                "text": "seam. pad.",
                "words": [
                    {"word": "seam.", "start": 105.0, "end": 105.2},
                    {"word": "pad.", "start": 109.6, "end": 109.8}
                ]
            })
        } else if len == 208_000 {
            json!({
                "text": "seam. pad.",
                "words": [
                    {"word": "seam.", "start": 5.0, "end": 5.2},
                    {"word": "pad.", "start": 9.6, "end": 9.8}
                ]
            })
        } else {
            panic!("unexpected unit length {len}")
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn keep_owner_window(len: usize, start: f64) -> bool {
        match len {
            1_760_000 => start == 104.0,
            208_000 => start == 10.0,
            _ => false,
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn owner_voice_pre_pad_is_delivered_once_with_the_recording_filter() {
        use sha2::Digest;
        let _hooks = ClearEmbeddedHooks::arm();
        crate::speakers::set_embedded_unit_speaker_empty(true);
        crate::speakers::set_embedded_first_pass_keep(Some(keep_owner_window));
        set_embedded_vad_sticky(Some(speech_vad));
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        write_config(root.path(), r#"{"transcribe":{"owner_voice_only":true}}"#);
        let entity = root.path().join("entities/owner");
        std::fs::create_dir_all(&entity).unwrap();
        std::fs::write(
            entity.join("entity.json"),
            r#"{"id":"owner","name":"Owner","type":"Person","is_principal":true}"#,
        )
        .unwrap();
        solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
            root.path(),
            "owner",
            &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
                centroid: axis(0),
                cluster_size: 1,
                timestamp: "2026-10-12T00:00:00Z".to_owned(),
                evidence_tier: "test".to_owned(),
            },
        )
        .unwrap();
        let (audio, bytes) = write_level_wav(root.path(), "120000_210", 1_808_000, 0.25);
        let asr = ScriptAsr {
            seen: std::cell::RefCell::new(Vec::new()),
            script: owner_script,
        };
        let file = run_file(&audio, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Transcribed);
        let seen = asr.seen.borrow();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].len(), 1_760_000);
        assert!(
            seen[0][1_752_000..1_760_000]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        assert_eq!(seen[0][1_656_000], 0.25);
        assert_eq!(seen[1].len(), 208_000);
        assert!(seen[1][..152_000].iter().all(|sample| *sample == 0.0));
        assert!(seen[1][152_000..].iter().all(|sample| *sample == 0.25));
        assert!(seen[1][..32_000].iter().all(|sample| *sample == 0.0));
        assert!(seen[1][192_000..].iter().all(|sample| *sample == 0.25));
        drop(seen);
        let jsonl = audio.with_extension("jsonl");
        assert_eq!(
            transcript_words(&jsonl),
            vec![" seam.".to_owned(), " pad.".to_owned()]
        );
        let record = read_processing_record_header(&jsonl).unwrap();
        let filter = solstone_core_processing_record::owner_voice(&record)
            .unwrap()
            .unwrap();
        assert_eq!(filter.windows_scored, 57);
        assert_eq!(filter.windows_kept, 2);
        assert!((filter.speech_s - 113.0).abs() < 1e-6);
        assert!((filter.kept_s - 7.0).abs() < 1e-6);
        assert!((filter.left_out_s - 106.0).abs() < 1e-6);
        assert_eq!(filter.original.name, "audio.wav");
        assert_eq!(filter.original.size, bytes.len() as u64);
        let mut digest = sha2::Sha256::new();
        digest.update(&bytes);
        assert_eq!(
            filter.original.sha256,
            crate::owner_voice::hex(digest.finalize().as_slice())
        );
        let print =
            solstone_core_speaker_resolve::owner_centroid::owner_voice_print(root.path()).unwrap();
        assert_eq!(
            filter.print.sha256,
            crate::owner_voice::print_sha256(&print.values)
        );
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn later_unit_failure_leaves_the_original_and_a_later_retry_completes() {
        let _hooks = ClearEmbeddedHooks::arm();
        crate::speakers::set_embedded_unit_speaker_empty(true);
        set_embedded_vad_sticky(Some(speech_vad));
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let before = todo_names(root.path());
        let (audio, bytes) = write_pcm_wav(root.path(), "120000_220", 1_920_000);
        let failing = SwitchAsr {
            calls: std::cell::Cell::new(0),
            fail_at: 2,
        };
        let error = run_file(&audio, root.path(), &failing, &|| false).unwrap_err();
        assert!(
            matches!(
                error,
                TranscribeError::ParakeetCppFailure { ref reason, .. } if reason == "embedded-inference"
            ),
            "{error:?}"
        );
        assert_eq!(failing.calls.get(), 2);
        assert_eq!(std::fs::read(&audio).unwrap(), bytes);
        assert!(!audio.with_extension("npz").exists());
        let record = read_processing_record_header(&audio.with_extension("jsonl")).unwrap();
        assert_eq!(record["state"], "failed");
        assert_ne!(record["state"], "analyzed");
        assert!(transcript_words(&audio.with_extension("jsonl")).is_empty());
        assert_eq!(todo_names(root.path()), before);

        let (held_audio, held_bytes) = write_pcm_wav(root.path(), "120000_221", 1_920_000);
        let held_jsonl = held_audio.with_extension("jsonl");
        let seeded = json!({
            "raw": "audio.wav",
            "_solstone_processing": {
                "schema": vocab::SCHEMA,
                "state": "analyzed",
                "handler": vocab::HANDLER_TRANSCRIBE,
                "input_size": held_bytes.len(),
            }
        });
        let seeded_text = format!("{seeded}\n");
        std::fs::write(&held_jsonl, &seeded_text).unwrap();
        let config = JournalConfigRead {
            present: false,
            sha256: None,
            config: None,
        };
        let attestation = AttestationStateStore::new();
        let held_asr = SwitchAsr {
            calls: std::cell::Cell::new(0),
            fail_at: 2,
        };
        let deleted = std::cell::Cell::new(false);
        let context = crate::embedded::EmbeddedContext {
            models: Path::new("missing-models"),
            scratch: &root.path().join("scratch"),
            asr: &held_asr,
            cancel: &|| false,
            service_io: &|_| {},
            owner_original_deleted: &|| deleted.set(true),
        };
        let held_error = crate::stage::process_one_with(
            &held_audio,
            root.path(),
            true,
            None,
            &config,
            &attestation,
            Some(&context),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(
                held_error,
                TranscribeError::ParakeetCppFailure { ref reason, .. } if reason == "embedded-inference"
            ),
            "{held_error:?}"
        );
        assert_eq!(std::fs::read(&held_audio).unwrap(), held_bytes);
        assert_eq!(std::fs::read_to_string(&held_jsonl).unwrap(), seeded_text);
        assert!(!deleted.get());

        let retry = LengthAsr(std::cell::RefCell::new(Vec::new()));
        let file = run_file(&audio, root.path(), &retry, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Transcribed);
        let record = read_processing_record_header(&audio.with_extension("jsonl")).unwrap();
        assert_eq!(record["state"], "analyzed");
        assert_eq!(
            transcript_words(&audio.with_extension("jsonl")),
            vec![" a.".to_owned()]
        );
        assert_eq!(std::fs::read(&audio).unwrap(), bytes);
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn confidential_multi_window_budget_is_the_sum_of_the_production_walks() {
        let _hooks = ClearEmbeddedHooks::arm();
        crate::speakers::set_embedded_unit_speaker_empty(true);
        crate::backend::confidential::set_confidential_send_hook(true);
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        write_config(
            root.path(),
            r#"{"transcribe":{"backend":"confidential","confidential_audio":true},"services":{"confidential":{}}}"#,
        );
        set_embedded_vad_sticky(Some(speech_vad));
        let (over, over_bytes) = write_pcm_wav(root.path(), "120000_230", 7_040_000);
        let error = run_file(&over, root.path(), &PanicAsr, &|| false).unwrap_err();
        assert!(
            matches!(
                error,
                TranscribeError::ConfidentialDeferred { ref reason, .. }
                    if reason == "confidential_audio_too_long"
            ),
            "{error:?}"
        );
        assert!(crate::backend::confidential::take_confidential_sends().is_empty());
        assert_eq!(std::fs::read(&over).unwrap(), over_bytes);

        set_embedded_vad_sticky(Some(confidential_under_vad));
        crate::backend::confidential::set_confidential_send_hook(true);
        let (under, _) = write_pcm_wav(root.path(), "120000_231", 1_920_000);
        let file = run_file(&under, root.path(), &PanicAsr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Transcribed);
        let sends = crate::backend::confidential::take_confidential_sends();
        assert_eq!(sends.len(), 4);
        assert!(sends.iter().all(|len| *len < 100_000));
        let sum = sends.iter().sum::<usize>();
        assert!(crate::backend::confidential::confidential_request_fits(sum));
        assert!(sends.iter().all(|len| *len == 48_000));
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn aba_script(len: usize) -> Value {
        if len == 1_760_000 {
            json!({"text":"a.","words":[{"word":"a.","start":1.0,"end":1.2}]})
        } else if len == 192_000 {
            json!({
                "text": "b. a.",
                "words": [
                    {"word": "b.", "start": 10.2, "end": 10.4},
                    {"word": "a.", "start": 11.0, "end": 11.2}
                ]
            })
        } else {
            panic!("unexpected unit length {len}")
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn speaker_evidence_keeps_single_overlap_and_identity_across_units() {
        let _hooks = ClearEmbeddedHooks::arm();
        set_embedded_vad_sticky(Some(speech_vad));
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();

        push_helper(
            root.path(),
            "single",
            1_408_000,
            0.0,
            false,
            Some(0),
            &[],
            &[1],
            &[0],
            Some(vec![(0.05, 0.15)]),
        );
        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        let (single, _) = write_pcm_wav(root.path(), "120000_240", 1_408_000);
        let file = run_file(&single, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Transcribed);
        let header =
            solstone_core_processing_record::read_jsonl_header(&single.with_extension("jsonl"))
                .unwrap();
        assert_eq!(header["speaker_evidence"], "single");
        assert!((header["speaker_evidence_multi_fraction"].as_f64().unwrap() - 0.025).abs() < 1e-9);
        assert_eq!(header["overlap_fraction"].as_f64().unwrap(), 0.0);
        let npz = solstone_core_speaker_id::embeddings::load_embeddings_file(
            &single.with_extension("npz"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            npz.statements.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1]
        );
        let body = std::fs::read_to_string(single.with_extension("jsonl")).unwrap();
        assert!(body.contains("speaker_turn_evidence"));

        push_helper(
            root.path(),
            "overlap",
            1_408_000,
            0.10,
            false,
            None,
            &[],
            &[1],
            &[0],
            None,
        );
        let (overlap, _) = write_pcm_wav(root.path(), "120000_241", 1_408_000);
        let file = run_file(&overlap, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Transcribed);
        let header =
            solstone_core_processing_record::read_jsonl_header(&overlap.with_extension("jsonl"))
                .unwrap();
        let fraction = header["overlap_fraction"].as_f64().unwrap();
        assert!((fraction - 0.10).abs() < 1e-9, "{fraction}");
        assert_ne!(fraction, 0.0);
        assert_eq!(header["speaker_evidence"], "multi");

        push_helper(
            root.path(),
            "aba-1",
            1_760_000,
            0.0,
            true,
            None,
            &[(1.0, 1.2, 0)],
            &[1],
            &[0],
            None,
        );
        push_helper(
            root.path(),
            "aba-2",
            192_000,
            0.0,
            true,
            None,
            &[(10.2, 10.4, 1), (11.0, 11.2, 0)],
            &[1, 2],
            &[1, 0],
            None,
        );
        let aba = ScriptAsr {
            seen: std::cell::RefCell::new(Vec::new()),
            script: aba_script,
        };
        let (recording, _) = write_pcm_wav(root.path(), "120000_242", 1_792_000);
        let file = run_file(&recording, root.path(), &aba, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Transcribed);
        let text = std::fs::read_to_string(recording.with_extension("jsonl")).unwrap();
        let mut speakers = Vec::new();
        let mut ids = Vec::new();
        for line in text.lines().skip(1) {
            let value: Value = serde_json::from_str(line).unwrap();
            ids.push(value["sentence_id"].as_i64().unwrap());
            speakers.push(value["speaker"].as_i64().unwrap());
        }
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(speakers[0], speakers[2]);
        assert_ne!(speakers[0], speakers[1]);
        let npz = solstone_core_speaker_id::embeddings::load_embeddings_file(
            &recording.with_extension("npz"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            npz.statements.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            ids
        );
        let header =
            solstone_core_processing_record::read_jsonl_header(&recording.with_extension("jsonl"))
                .unwrap();
        assert_eq!(header["speaker_evidence"], "multi");
        assert!(
            header["interval_embeddings"].is_null() || header.get("interval_embeddings").is_none()
        );
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    fn assert_sound_tags_omitted(header: &Value, body: &str) {
        assert!(header.get("sound_tags").is_none());
        assert!(header.get("sound_tag_status").is_none());
        assert!(!body.contains("Sound tagging stays on the separate ced helper"));
        assert!(!body.contains("does not start that helper"));
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn no_speech_omits_sound_tags_under_ten_seconds() {
        let _hooks = ClearEmbeddedHooks::arm();
        set_embedded_vad_sticky(Some(|samples| timed_vad(samples, Vec::new())));
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let (audio, _) = write_pcm_wav(root.path(), "120000_260", 16_000);
        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        let file = run_file(&audio, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Preserved);
        assert!(asr.0.borrow().is_empty());
        let jsonl = audio.with_extension("jsonl");
        let body = std::fs::read_to_string(&jsonl).unwrap();
        let header = solstone_core_processing_record::read_jsonl_header(&jsonl).unwrap();
        assert_sound_tags_omitted(&header, &body);
        assert_eq!(header["_solstone_processing"]["state"], "empty");
        assert_eq!(header["_solstone_processing"]["reason_code"], "no_speech");
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn empty_recognizer_omits_sound_tags_under_ten_seconds() {
        let _hooks = ClearEmbeddedHooks::arm();
        set_embedded_vad_sticky(Some(speech_vad));
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let (audio, _) = write_pcm_wav(root.path(), "120000_261", 16_000);
        let asr = ScriptAsr {
            seen: std::cell::RefCell::new(Vec::new()),
            script: |_| json!({"text": "", "words": []}),
        };
        let file = run_file(&audio, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Preserved);
        assert_eq!(asr.seen.borrow().len(), 1);
        let jsonl = audio.with_extension("jsonl");
        let body = std::fs::read_to_string(&jsonl).unwrap();
        let header = solstone_core_processing_record::read_jsonl_header(&jsonl).unwrap();
        assert_sound_tags_omitted(&header, &body);
        assert_eq!(header["_solstone_processing"]["state"], "empty");
        assert_eq!(
            header["_solstone_processing"]["reason_code"],
            "no_transcript"
        );
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn process_embedded_runs_the_in_process_adapter() {
        let _hooks = ClearEmbeddedHooks::arm();
        set_embedded_vad_sticky(Some(speech_vad));
        crate::speakers::set_embedded_unit_speaker_empty(true);
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let (audio, _) = write_pcm_wav(root.path(), "120000_270", 1_760_000);
        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        let file = run_file(&audio, root.path(), &asr, &|| false).unwrap();
        assert_eq!(file.outcome, crate::TranscribeOutcome::Transcribed);
        assert_eq!(asr.0.borrow().len(), 1);
        assert_eq!(ced_stub_counts(), (1, 2));
        let seen = ced_stub_seen();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, 1_760_000);
        assert_eq!(seen[0].1.len(), 10);
        assert!(
            seen[0]
                .1
                .iter()
                .all(|(start, end)| *start < *end && *end <= 1_600_000)
        );
        assert_eq!(seen[0].1[0], (0, 160_000));
        assert_eq!(*seen[0].1.last().unwrap(), (1_440_000, 1_600_000));
        assert_eq!(seen[1], (160_000, vec![(0, 160_000)]));
        let jsonl = audio.with_extension("jsonl");
        let body = std::fs::read_to_string(&jsonl).unwrap();
        let header = solstone_core_processing_record::read_jsonl_header(&jsonl).unwrap();
        let spans = solstone_core_sound_tags::window_spans(1_760_000);
        assert_eq!(spans.len(), 11);
        assert_eq!(header["sound_tags"]["windows"], json!(spans.len()));
        assert!(header.get("sound_tag_status").is_none());
        assert!(!body.contains("Sound tagging stays on the separate ced helper"));
        assert!(!body.contains("does not start that helper"));
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn in_process_probe_does_not_spawn() {
        let missing = std::path::PathBuf::from("/var/tmp/solstone-ced-probe-missing-library.so");
        assert!(!missing.is_file());
        let request = serde_json::to_string(&json!({
            "schema": solstone_core_ced_analyze::PROBE_REQUEST_SCHEMA,
            "models": {
                "ced_library_path": &missing,
                "ced_model_path": &missing,
            },
        }))
        .unwrap();
        let error = solstone_core_ced_analyze::run_probe_request(&request).unwrap_err();
        assert!(matches!(
            error,
            solstone_core_ced_analyze::AnalyzeError::LibraryUnreadable { .. }
        ));
        assert_eq!(error.reason(), "library-unreadable");
        let probe_detail = error.detail();
        assert!(!probe_detail.contains("could not be loaded"));

        let ran = std::cell::Cell::new(false);
        let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
        let verdict = evaluate_ced_readiness_with_probe(
            Path::new("/var/tmp/unused-journal"),
            os,
            arch,
            |_library, _model| {
                ran.set(true);
                probe_ced_in_process(&missing, &missing)
            },
        );
        match verdict {
            CedVerdict::Degraded(status) => {
                let detail = status.detail().expect("degraded status has a detail");
                assert!(!detail.is_empty());
                assert!(!detail.contains("Sound tagging stays on the separate ced helper"));
                assert!(!detail.contains("does not start that helper"));
                if ran.get() {
                    assert!(matches!(
                        status,
                        CapabilityStatus::UnloadableOrUnrunnable { .. }
                    ));
                    assert_eq!(detail, probe_detail);
                }
            }
            other => panic!("expected a real degraded status, got {other:?}"),
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn run_classify_request_on_a_missing_library_returns_no_spawn() {
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let audio = root.path().join("audio.f32le");
        std::fs::write(&audio, 0.0_f32.to_le_bytes()).unwrap();
        let library = root.path().join("missing-ced.so");
        assert!(!library.exists());
        let request = serde_json::to_string(&json!({
            "schema": solstone_core_ced_analyze::REQUEST_SCHEMA,
            "models": {
                "ced_library_path": &library,
                "ced_model_path": root.path().join("missing-model.gguf"),
            },
            "audio_f32le_path": &audio,
            "sample_rate_hz": solstone_core_sound_tags::CLASSIFY_SAMPLE_RATE,
            "top_k": solstone_core_sound_tags::TOP_K,
            "windows": [{"start_sample": 0, "end_sample": 1}],
        }))
        .unwrap();
        let error = solstone_core_ced_analyze::run_classify_request(&request).unwrap_err();
        assert!(matches!(
            error,
            solstone_core_ced_analyze::AnalyzeError::LibraryUnreadable { .. }
        ));
        assert_eq!(error.reason(), "library-unreadable");
        assert!(!error.detail().contains("could not be loaded"));
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn cancel_inside_clustering_publishes_nothing() {
        let _hooks = ClearEmbeddedHooks::arm();
        set_embedded_vad_sticky(Some(speech_vad));
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        push_helper(
            root.path(),
            "cluster-cancel",
            16_000,
            0.0,
            true,
            None,
            &[(0.0, 0.2, 0), (0.3, 0.5, 1), (0.6, 0.8, 0)],
            &[1],
            &[0],
            None,
        );
        let (audio, bytes) = write_pcm_wav(root.path(), "120000_262", 16_000);
        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        let deleted = std::cell::Cell::new(false);
        // After the helper response is consumed: the end-of-walk checkpoint, then
        // the checkpoint immediately before cluster_embeddings_cancellable. The
        // next poll is inside that function.
        let after_response = std::cell::Cell::new(0_usize);
        let cancel = || {
            if !crate::speakers::embedded_speaker_response_seen() {
                return false;
            }
            let seen = after_response.get();
            after_response.set(seen + 1);
            seen >= 2
        };
        let error = crate::embedded::process_embedded(
            &audio,
            root.path(),
            Path::new("missing-models"),
            &root.path().join("scratch"),
            &asr,
            crate::embedded::EmbeddedControl {
                owner_original_deleted: &|| deleted.set(true),
                cancelled: &cancel,
                service_io: &|_| {},
            },
            None,
        )
        .expect_err("cancel inside clustering");
        assert!(matches!(error, TranscribeError::Cancelled), "{error:?}");
        assert!(
            after_response.get() > 2,
            "cancel did not reach the cluster polls: {}",
            after_response.get()
        );
        assert_eq!(std::fs::read(&audio).unwrap(), bytes);
        assert!(!audio.with_extension("jsonl").exists());
        assert!(!audio.with_extension("npz").exists());
        assert!(!deleted.get());
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn cancel_before_cluster_publishes_nothing() {
        let _hooks = ClearEmbeddedHooks::arm();
        set_embedded_vad_sticky(Some(speech_vad));
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        push_helper(
            root.path(),
            "cancel",
            16_000,
            0.0,
            true,
            None,
            &[(0.0, 0.2, 0)],
            &[1],
            &[0],
            None,
        );
        let (audio, bytes) = write_pcm_wav(root.path(), "120000_250", 16_000);
        let asr = LengthAsr(std::cell::RefCell::new(Vec::new()));
        let error = run_file(&audio, root.path(), &asr, &|| {
            crate::speakers::embedded_speaker_response_seen()
        })
        .unwrap_err();
        assert!(matches!(error, TranscribeError::Cancelled), "{error:?}");
        assert_eq!(std::fs::read(&audio).unwrap(), bytes);
        assert!(!audio.with_extension("jsonl").exists());
        assert!(!audio.with_extension("npz").exists());
    }
}
