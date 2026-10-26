// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Embedded speech, one native call at a time, through the end of the recording.
//!
//! The first unit has no leading seam. Each later unit repeats the previous
//! pyannote window inside the native-call cap. Words whose restored start falls
//! in an earlier unit's committed range are dropped. Desktop decode never enters
//! this loop.

use std::path::Path;
use std::time::Instant;

use serde_json::{Map, Value, json};
use solstone_core_journal_config::JournalConfigRead;
use solstone_core_observe_audio::{
    AudioError, AudioReduction, SAMPLE_RATE, UnitDecoder, VadResult, reduce_audio,
};
use solstone_core_processing_record::{
    TerminalProofOutcome, evaluate_terminal_proof, read_processing_record_header, vocab,
};
use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
use solstone_core_speaker_resolve::owner_centroid::OwnerVoicePrint;
use solstone_core_speakers::diarization::{
    SentenceTiming, SpeakerInterval, assign_sentences, cluster_embeddings,
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
    first_pass_analysis, masked_audio_vad, merge_record_extra, segment_meta, source_from_path,
    statements_from_words, stt_zero_statements, transcribe_retained_intervals, transcript_paths,
    vad_no_speech, write_failed_record,
};
use crate::transcript::{FullTranscriptWrite, restore_statement_timestamps, write_full_transcript};
use crate::{TranscribeError, event::TranscribedOutcome};

/// Samples in the pyannote window repeated at the front of every unit after the first.
pub(crate) const EMBEDDED_SEAM_SAMPLES: usize =
    solstone_core_speakers::PYANNOTE_WINDOW_S as usize * SAMPLE_RATE as usize;
/// New samples committed by a full unit. The seam rides inside the native-call cap.
pub(crate) const EMBEDDED_STEP_SAMPLES: usize = EMBEDDED_AUDIO_MAX_SAMPLES - EMBEDDED_SEAM_SAMPLES;
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
/// at `committed_start` belongs to this unit. Restored times are not rewritten again.
pub(crate) fn place_unit_words(
    words: &[TranscriptionWord],
    reduction: Option<&AudioReduction>,
    origin_samples: usize,
    committed_start: usize,
) -> Vec<TranscriptionWord> {
    let origin_s = origin_samples as f64 / f64::from(SAMPLE_RATE);
    let mut placed = Vec::new();
    for word in words {
        let (start, end) = restored_span(word.start, word.end, reduction);
        let start = start + origin_s;
        let end = end + origin_s;
        let start_sample = (start * f64::from(SAMPLE_RATE)).round() as usize;
        if start_sample < committed_start {
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
    let held = prior_publication_held(job.audio_path, job.facts.input_size);
    if job.backend == "confidential" {
        return match confidential_pass(&mut job, held)? {
            DraftOrDone::Draft(draft) => publish(job, held, draft),
            DraftOrDone::Done(outcome) => Ok(outcome),
        };
    }
    match transcribe_walk(&mut job, held) {
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
}

impl WalkSummary {
    fn audio_seconds(&self) -> f64 {
        self.total_samples as f64 / f64::from(SAMPLE_RATE)
    }

    fn vad(&self) -> VadResult {
        let speech_duration_s = self
            .speech_segments
            .iter()
            .map(|(start, end)| (end - start).max(0.0))
            .sum();
        VadResult {
            duration_s: self.audio_seconds(),
            speech_duration_s,
            has_speech: self.any_speech,
            speech_segments: self.speech_segments.clone(),
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        }
    }
}

struct UnitPrepared {
    masked: Vec<f32>,
    vad: VadResult,
    origin_samples: usize,
    committed_start: usize,
}

struct TranscriptDraft {
    summary: WalkSummary,
    words: Vec<TranscriptionWord>,
    intervals: Vec<SpeakerInterval>,
    interval_embeddings: Vec<f32>,
    statement_rows: Vec<UnitStatementEmbedding>,
    boundaries: Vec<usize>,
    model: Option<ModelInfo>,
    reduced_seconds: Option<f64>,
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
) -> Result<DraftOrDone, TranscribeError> {
    let mut retained = 0usize;
    let counted = match each_prepared_unit(job, held, false, |_job, prepared| {
        retained += retained_sample_count(&prepared.masked, &prepared.vad);
        Ok(())
    }) {
        Ok(summary) => summary,
        Err(stop) => return apply_stop(job, held, stop).map(DraftOrDone::Done),
    };
    let seconds = counted.audio_seconds();
    let mut stop = None;
    let mut done = None;
    // The second walk is the closure. An over-budget total never calls it, so nothing is sent.
    let gated =
        dispatch_confidential_with_cap(retained, seconds, || match transcribe_walk(job, held) {
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
            settle_inference(job, held, error, seconds, counted.vad())?;
            unreachable!("inference settlement returns the error");
        }
    }
}

fn transcribe_walk(job: &mut EmbeddedJob<'_>, held: bool) -> Result<TranscriptDraft, WalkStop> {
    let mut draft = TranscriptDraft {
        summary: WalkSummary {
            any_speech: false,
            total_samples: 0,
            speech_segments: Vec::new(),
        },
        words: Vec::new(),
        intervals: Vec::new(),
        interval_embeddings: Vec::new(),
        statement_rows: Vec::new(),
        boundaries: Vec::new(),
        model: None,
        reduced_seconds: None,
    };
    let mut reduced_sum = 0.0;
    let mut reduced_any = false;
    let summary = each_prepared_unit(job, held, true, |job, prepared| {
        recognize_unit(
            job,
            prepared,
            &mut draft,
            &mut reduced_sum,
            &mut reduced_any,
        )
    })?;
    draft.summary = summary;
    if reduced_any {
        draft.reduced_seconds = Some(reduced_sum);
    }
    Ok(draft)
}

fn recognize_unit(
    job: &mut EmbeddedJob<'_>,
    prepared: &UnitPrepared,
    draft: &mut TranscriptDraft,
    reduced_sum: &mut f64,
    reduced_any: &mut bool,
) -> Result<(), WalkStop> {
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
    draft.statement_rows.extend(rows.statement_rows);
    draft.words.extend(place_unit_words(
        &response.words,
        retained.mapping.as_ref(),
        prepared.origin_samples,
        prepared.committed_start,
    ));
    draft.boundaries.push(prepared.committed_start);
    draft.model = Some(model);
    Ok(())
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

fn each_prepared_unit<F>(
    job: &mut EmbeddedJob<'_>,
    held: bool,
    record_commits: bool,
    mut visit: F,
) -> Result<WalkSummary, WalkStop>
where
    F: FnMut(&mut EmbeddedJob<'_>, &UnitPrepared) -> Result<(), WalkStop>,
{
    let mut decoder =
        UnitDecoder::open(job.audio_path).map_err(|error| stop_decode(job, held, error))?;
    let mut seam = Vec::new();
    let mut committed_start = 0usize;
    let mut summary = WalkSummary {
        any_speech: false,
        total_samples: 0,
        speech_segments: Vec::new(),
    };
    loop {
        job.embedded.checkpoint().map_err(WalkStop::Bare)?;
        let pulled = decoder
            .pull(EMBEDDED_STEP_SAMPLES)
            .map_err(|error| stop_decode(job, held, error))?;
        let Some(new_samples) = pulled else {
            if seam.is_empty() && committed_start == 0 {
                return Err(stop_decode(
                    job,
                    held,
                    AudioError::NoDecodedAudio {
                        path: job.audio_path.to_path_buf(),
                    },
                ));
            }
            break;
        };
        if new_samples.is_empty() {
            if seam.is_empty() && committed_start == 0 {
                return Err(stop_decode(
                    job,
                    held,
                    AudioError::NoDecodedAudio {
                        path: job.audio_path.to_path_buf(),
                    },
                ));
            }
            break;
        }
        let seam_len = seam.len();
        let mut unit = std::mem::take(&mut seam);
        let new_len = new_samples.len();
        unit.extend_from_slice(&new_samples);
        debug_assert!(unit.len() <= EMBEDDED_AUDIO_MAX_SAMPLES);
        let origin_samples = committed_start - seam_len;
        if new_len == EMBEDDED_STEP_SAMPLES {
            let tail = unit.len() - EMBEDDED_SEAM_SAMPLES;
            seam = unit[tail..].to_vec();
        }
        let mut prepared = prepare_unit(job, held, unit, origin_samples)?;
        if prepared.vad.has_speech {
            summary.any_speech = true;
            let shift = origin_samples as f64 / f64::from(SAMPLE_RATE);
            summary.speech_segments.extend(
                prepared
                    .vad
                    .speech_segments
                    .iter()
                    .map(|(start, end)| (start + shift, end + shift)),
            );
            prepared.committed_start = committed_start;
            visit(job, &prepared)?;
        }
        note_commit(record_commits, committed_start, committed_start + new_len);
        committed_start += new_len;
    }
    summary.total_samples = committed_start;
    Ok(summary)
}

fn prepare_unit(
    job: &mut EmbeddedJob<'_>,
    held: bool,
    unit: Vec<f32>,
    origin_samples: usize,
) -> Result<UnitPrepared, WalkStop> {
    let mut vad = unit_vad(job, &unit).map_err(WalkStop::Bare)?;
    let mut masked = unit;
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
        let name = job
            .audio_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let filter = crate::owner_voice::filter_record(
            &step.speech,
            &step.decision,
            name,
            job.facts.input_size,
            &job.observation.input_sha256(),
            &print.values,
        );
        job.facts.record_extra = crate::owner_voice::success_extra(&filter);
        masked = step.audio;
        vad = masked_unit_vad(job, &masked).map_err(WalkStop::Bare)?;
    }
    Ok(UnitPrepared {
        masked,
        vad,
        origin_samples,
        committed_start: 0,
    })
}

fn unit_vad(job: &EmbeddedJob<'_>, audio: &[f32]) -> Result<VadResult, TranscribeError> {
    #[cfg(all(test, feature = "full-tests"))]
    if let Some(hook) = EMBEDDED_VAD_STICKY.with(|cell| *cell.borrow()) {
        return Ok(hook(audio.len()));
    }
    crate::audio::run_vad_embedded(
        audio,
        crate::config::min_speech_seconds(job.config),
        job.embedded.models,
        job.embedded.scratch,
    )
}

fn masked_unit_vad(job: &EmbeddedJob<'_>, audio: &[f32]) -> Result<VadResult, TranscribeError> {
    #[cfg(all(test, feature = "full-tests"))]
    if let Some(hook) = EMBEDDED_VAD_STICKY.with(|cell| *cell.borrow()) {
        return Ok(hook(audio.len()));
    }
    masked_audio_vad(audio, job.config, Some(job.embedded))
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
    job.embedded.checkpoint()?;
    if !draft.summary.any_speech {
        return publish_empty(job, EmptyKind::NoSpeech, draft);
    }
    if draft.words.is_empty() {
        return publish_empty(job, EmptyKind::NoTranscript, draft);
    }
    let speaker_result = match build_speaker_result(job.audio_path, &draft) {
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
        ..
    } = job;
    let vad = draft.summary.vad();
    let audio_seconds = draft.summary.audio_seconds();
    let checkpoint = || embedded.checkpoint();
    checkpoint()?;
    let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
    let terminal = match kind {
        EmptyKind::NoSpeech => vad_no_speech(&facts, redo, None, None, context_attempt.as_ref())?,
        EmptyKind::NoTranscript => {
            stt_zero_statements(&facts, redo, None, None, context_attempt.as_ref())?
        }
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
        ..
    } = job;
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
        sound_tags: None,
        sound_tag_status: None,
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
        let clustered = cluster_embeddings(&draft.interval_embeddings, rows, EMBEDDING_WIDTH, None)
            .map_err(|error| {
                TranscribeError::SpeakerAnalysis(crate::speakers::SpeakerAnalyzeError::new(
                    audio_path,
                    "cluster",
                    error.to_string(),
                    None,
                ))
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
    let has_intervals = !draft.intervals.is_empty();
    Ok(SpeakerAnalyzeResult {
        statements,
        embedding_payload,
        speaker_evidence: if has_intervals { "multi" } else { "none" }.to_owned(),
        speaker_evidence_multi_fraction: if has_intervals { 1.0 } else { 0.0 },
        overlap_fraction: 0.0,
        statement_labels: Some(labels),
        turn_ranges: None,
        overlap_ranges: None,
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
    static EMBEDDED_UNIT_COMMITS: std::cell::RefCell<Vec<(usize, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
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
        let first = place_unit_words(&[word("seam.", 0.0, 0.4)], Some(&reduction), 0, 0);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].start, 5.0);
        assert_eq!(first[0].end, 5.4);
        let overlap = place_unit_words(
            &[word("seam.", 0.0, 0.4)],
            Some(&reduction),
            1_600_000,
            1_760_000,
        );
        assert!(overlap.is_empty());
        let boundary = place_unit_words(&[word("edge.", 10.0, 10.2)], None, 1_600_000, 1_760_000);
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
    impl Drop for ClearEmbeddedHooks {
        fn drop(&mut self) {
            set_embedded_vad_sticky(None);
            crate::speakers::set_embedded_unit_speaker_empty(false);
            let _ = take_embedded_unit_commits();
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
        let _hooks = ClearEmbeddedHooks;
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
                (0, 1_760_000),
                (1_760_000, 3_520_000),
                (3_520_000, 5_280_000),
                (5_280_000, 7_040_000),
                (7_040_000, 8_480_000),
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
        let _hooks = ClearEmbeddedHooks;
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
}
