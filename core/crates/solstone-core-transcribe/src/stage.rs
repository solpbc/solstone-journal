// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Terminal stage outcomes. Empty terminals retain the raw input.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use solstone_core_journal_config::JournalConfigRead;
use solstone_core_journal_io::{LockOptions, hold_lock};
use solstone_core_observe_audio::{SAMPLE_RATE, audio_to_wav_bytes, decode_f32_mono};
use solstone_core_speaker_id::writer::WriteResponse;
use solstone_core_spp_ratls::AttestationStateStore;

use crate::TranscribeError;
use crate::args::should_skip_process_one_processed;
use crate::audio::{reduce_audio_if_needed, run_vad, speech_ratio, tag_audio};
#[cfg(target_os = "macos")]
use crate::backend::parakeet_coreml;
use crate::backend::parakeet_cpp;
use crate::backend::{
    confidential, local_stt_backend, platform_floor_bytes, read_available_bytes,
    resolve_default_backend,
};
use crate::config::{confidential_audio_enabled, min_speech_seconds, parakeet_cpp_device};
use crate::event::{
    Timings, TranscribedEvent, TranscribedOutcome, build_transcribed_event, emit_transcribed_event,
};
use crate::processing::analyzed_record;
use crate::processing::{EmptyReason, corrupt_input_record, empty_record};
use crate::speakers::analyze_speakers;
use crate::terminal::{TerminalWrite, TerminalWriteFailure, write_terminal, write_terminal_with};
use crate::transcript::{
    FullTranscriptWrite, remove_orphan_npz, restore_statement_timestamps, write_full_transcript,
};
use solstone_core_observe_audio::reduce_audio;

const SPEAKER_EVIDENCE_VERSION: &str = "speaker-evidence-v1";
const SPEAKER_ANALYSIS_PRODUCER: &str = "solstone-core-speakers-analyze";
const OVERLAP_DETECTOR: &str = "pyannote-segmentation-3.0";

/// Immutable facts captured from the owner-media file before processing starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputFacts {
    pub(crate) path: PathBuf,
    pub(crate) input_size: u64,
}

/// Capture the input's byte count so the processing record names these bytes.
pub(crate) fn capture_input_facts(path: &Path) -> Result<InputFacts, TranscribeError> {
    let input_size = fs::metadata(path)
        .map_err(|error| TranscribeError::InputMetadata {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?
        .len();
    Ok(InputFacts {
        path: path.to_path_buf(),
        input_size,
    })
}

/// Terminal result that controls the transcribed event's outcome field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalOutcome {
    /// Terminal proof was written and the raw input was retained.
    Preserved,
    /// Decode failed, terminal failure proof was written, and input was retained.
    Failed,
}

/// A completed single-file driver disposition with an intentional zero exit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessOutcome {
    Skipped,
    Transcribed,
    Preserved,
    Failed,
}

/// Run the currently implemented native transcription path for one segment audio file.
pub(crate) fn process_one(
    audio_path: &Path,
    journal_path: &Path,
    redo: bool,
    explicit_backend: Option<&str>,
    config: &JournalConfigRead,
    attestation_state: &AttestationStateStore,
) -> Result<ProcessOutcome, TranscribeError> {
    if should_skip_process_one_processed(audio_path, redo) {
        return Ok(ProcessOutcome::Skipped);
    }
    // Live and catch-up dispatchers have separate queues. The output owner must
    // serialize the entire attempt, including the completed-output check, so a
    // duplicate cannot run inference before losing an exclusive publication race.
    // This kernel lock is released on process death; the sidecar is never removed.
    // Allow an ordinary Sense transcription's 45-minute execution budget to finish.
    let output = audio_path.with_extension("jsonl");
    let _claim = hold_lock(
        &output,
        LockOptions {
            timeout: Duration::from_secs(45 * 60),
            ..LockOptions::default()
        },
    )
    .map_err(|error| TranscribeError::ProcessingClaim {
        path: audio_path.to_path_buf(),
        detail: error.to_string(),
    })?;
    if should_skip_process_one_processed(audio_path, redo) {
        return Ok(ProcessOutcome::Skipped);
    }
    let started = Instant::now();
    let facts = capture_input_facts(audio_path)?;
    let mut timings = Timings::default();
    if let Some(queue_wait_ms) = queue_wait_ms() {
        timings.add_ms("queue_wait", queue_wait_ms);
    }
    let decoded_at = Instant::now();
    let full_audio = match decode_f32_mono(audio_path) {
        Ok(audio) => audio,
        Err(error) => {
            timings.add_ms("decode", elapsed_ms(decoded_at));
            let decode_error = TranscribeError::Decode {
                detail: error.to_string(),
            };
            let write_at = Instant::now();
            let terminal = decode_failure(&facts, redo)?;
            timings.add_ms("write", elapsed_ms(write_at));
            emit_event(
                audio_path,
                journal_path,
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
                &timings,
                None,
            );
            return Ok(match terminal {
                TerminalOutcome::Failed => ProcessOutcome::Failed,
                _ => unreachable!(),
            });
        }
    };
    timings.add_ms("decode", elapsed_ms(decoded_at));
    let audio_seconds = full_audio.len() as f64 / f64::from(SAMPLE_RATE);
    let vad_at = Instant::now();
    let vad = run_vad(&full_audio, min_speech_seconds(config))?;
    timings.add_ms("vad", elapsed_ms(vad_at));
    let sound_tags = tag_audio(&full_audio, journal_path);

    if !vad.has_speech {
        let write_at = Instant::now();
        let terminal = vad_no_speech(&facts, redo, sound_tags.as_ref())?;
        timings.add_ms("write", elapsed_ms(write_at));
        let outcome = match terminal {
            TerminalOutcome::Preserved => {
                (ProcessOutcome::Preserved, TranscribedOutcome::Preserved)
            }
            TerminalOutcome::Failed => (ProcessOutcome::Failed, TranscribedOutcome::Failed),
        };
        emit_event(
            audio_path,
            journal_path,
            outcome.1,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(audio_seconds),
            None,
            None,
            &timings,
            Some(&vad),
        );
        return Ok(outcome.0);
    }

    let reduce_at = Instant::now();
    let reduction = reduce_audio_if_needed(&full_audio, &vad, speech_ratio(&vad));
    if reduction.is_some() {
        timings.add_ms("reduce", elapsed_ms(reduce_at));
    }
    let (mut reduced_audio, mut reduction) = reduction
        .map(|(audio, reduction)| (Some(audio), Some(reduction)))
        .unwrap_or((None, None));
    let mut reduced_seconds = reduced_audio
        .as_ref()
        .map(|audio| audio.len() as f64 / f64::from(SAMPLE_RATE));

    let backend = match resolve_default_backend(
        explicit_backend,
        local_stt_backend(),
        read_available_bytes(),
        platform_floor_bytes(),
        confidential::confidential_channel_plausible(config),
        confidential_audio_enabled(config),
    ) {
        Ok(resolution) => resolution.backend,
        Err(error) => {
            emit_backend_error(
                BackendErrorContext {
                    audio_path,
                    journal_path,
                    audio_seconds,
                    reduced_seconds,
                    timings: &timings,
                    vad: &vad,
                },
                &error,
                None,
            );
            return Err(error);
        }
    };
    // 🔴 Reduce anyway when the confidential limit would otherwise refuse this audio.
    //
    // `reduce_audio_if_needed` declines when the VAD result is noisy AND speech-dense,
    // which is a sensible default -- but the confidential STT refuses ANY request over
    // its per-request limit, and the segmenter targets ~300 s, so real segments land
    // 1-5 s over it constantly. Measured on the founder's journal 2026-09-01: 60
    // `confidential_audio_too_long` refusals, the sampled ones 301-305 s, none of them
    // reduced. That is the difference between a transcript and silently losing the
    // recording, so when the alternative is refusal we reduce to speech-only -- which
    // is what an STT wants regardless.
    //
    // 🔒 Confidential-only and last-resort: every other backend keeps the heuristic
    // untouched, and audio still too long after reduction is still refused below.
    if backend == "confidential"
        && reduced_audio.is_none()
        && !confidential::confidential_request_fits(full_audio.len())
        && let Some((audio, info)) = reduce_audio(&full_audio, &vad)
    {
        reduced_seconds = Some(audio.len() as f64 / f64::from(SAMPLE_RATE));
        reduced_audio = Some(audio);
        reduction = Some(info);
    }
    let statement_audio = reduced_audio.as_deref().unwrap_or(&full_audio);

    let mut asr_at = None;
    let dispatched = if backend == "confidential" {
        // 🔴 Cap on what is actually SENT, not on the whole segment. `statement_audio`
        // below is the VAD-reduced audio, and speech is usually a fraction of a
        // segment -- so capping on `audio_seconds` refused recordings whose speech
        // was comfortably inside the limit.
        //
        // Measured on the founder's journal 2026-09-01: of 176 recent segments, 134
        // (76%) run longer than 300 s, and V2 had transcribed only 14 of 37 long
        // segments (38%) where V1 transcribed 503 of 503 (100%). Most of that gap is
        // this line: a 314 s recording with a minute of speech was refused as
        // `confidential_audio_too_long` without the reduced audio ever being measured.
        //
        // 🔒 The limit itself is unchanged, and audio that is genuinely too long after
        // reduction is still refused.
        dispatch_confidential_with_cap(
            statement_audio.len(),
            reduced_seconds.unwrap_or(audio_seconds),
            || {
                asr_at = Some(Instant::now());
                dispatch_backend(
                    config,
                    journal_path,
                    &backend,
                    statement_audio,
                    attestation_state,
                )
            },
        )
    } else {
        asr_at = Some(Instant::now());
        dispatch_backend(
            config,
            journal_path,
            &backend,
            statement_audio,
            attestation_state,
        )
    };
    if let Some(asr_at) = asr_at {
        timings.add_ms("asr", elapsed_ms(asr_at));
    }
    let (transcription, model_info) = match dispatched {
        Ok(result) => result,
        Err(error) => {
            emit_backend_error(
                BackendErrorContext {
                    audio_path,
                    journal_path,
                    audio_seconds,
                    reduced_seconds,
                    timings: &timings,
                    vad: &vad,
                },
                &error,
                Some(&backend),
            );
            return Err(error);
        }
    };

    if transcription.words.is_empty() {
        let write_at = Instant::now();
        let terminal = stt_zero_statements(&facts, redo, sound_tags.as_ref())?;
        timings.add_ms("write", elapsed_ms(write_at));
        let outcome = match terminal {
            TerminalOutcome::Preserved => {
                (ProcessOutcome::Preserved, TranscribedOutcome::Preserved)
            }
            TerminalOutcome::Failed => (ProcessOutcome::Failed, TranscribedOutcome::Failed),
        };
        emit_event(
            audio_path,
            journal_path,
            outcome.1,
            None,
            None,
            None,
            Some(&backend),
            Some(&model_info.device),
            Some(&model_info.model),
            Some(audio_seconds),
            reduced_seconds,
            None,
            &timings,
            Some(&vad),
        );
        return Ok(outcome.0);
    }

    let statements = statements_from_words(&transcription.words);
    let restored = reduction.as_ref().map_or_else(
        || statements.clone(),
        |mapping| restore_statement_timestamps(&statements, mapping),
    );
    let speakers_at = Instant::now();
    let speaker_result = match analyze_speakers(
        audio_path,
        &full_audio,
        statement_audio,
        reduced_audio.as_deref(),
        &statements,
        &restored,
        SAMPLE_RATE,
        0.25,
    ) {
        Ok(result) => result,
        Err(error) => {
            timings.add_ms("speakers_analyze", elapsed_ms(speakers_at));
            let error = TranscribeError::SpeakerAnalysis(error);
            emit_event(
                audio_path,
                journal_path,
                TranscribedOutcome::Failed,
                Some("speaker-analysis-native-failure"),
                Some("SpeakerAnalyzeError"),
                Some(&error),
                Some(&backend),
                Some(&model_info.device),
                Some(&model_info.model),
                Some(audio_seconds),
                reduced_seconds,
                None,
                &timings,
                Some(&vad),
            );
            return Err(error);
        }
    };
    timings.add_ms("speakers_analyze", elapsed_ms(speakers_at));
    let (jsonl_path, npz_path) = transcript_paths(audio_path);
    let processing = analyzed_record(facts.input_size);
    let write_at = Instant::now();
    write_full_transcript(FullTranscriptWrite {
        raw_path: audio_path,
        jsonl_path: &jsonl_path,
        npz_path: &npz_path,
        base_time_us_of_day: base_time_us_of_day(audio_path),
        source: source_from_path(audio_path).as_deref(),
        speaker_result: &speaker_result,
        backend: Some(&backend),
        model: Some(&model_info.model),
        device: Some(&model_info.device),
        compute_type: (!model_info.compute_type.is_empty())
            .then_some(model_info.compute_type.as_str()),
        observer: std::env::var("OBSERVER_NAME").ok().as_deref(),
        vad_result: Some(&vad),
        segment_meta: segment_meta().as_ref(),
        overlap_detector: Some(OVERLAP_DETECTOR),
        speaker_evidence_version: SPEAKER_EVIDENCE_VERSION,
        processing: &processing,
        sound_tags: sound_tags.as_ref(),
        speaker_analysis_producer: Some(SPEAKER_ANALYSIS_PRODUCER),
        redo,
    })?;
    timings.add_ms("write", elapsed_ms(write_at));
    emit_event(
        audio_path,
        journal_path,
        TranscribedOutcome::Transcribed,
        None,
        None,
        None,
        Some(&backend),
        Some(&model_info.device),
        Some(&model_info.model),
        Some(audio_seconds),
        reduced_seconds,
        Some(elapsed_ms(started)),
        &timings,
        Some(&vad),
    );
    Ok(ProcessOutcome::Transcribed)
}

fn dispatch_after_egress_gate<T, F>(
    config: &JournalConfigRead,
    backend: &str,
    dispatch: F,
) -> Result<T, TranscribeError>
where
    F: FnOnce() -> Result<T, TranscribeError>,
{
    confidential::refuse_confidential_egress(config, backend, confidential_audio_enabled(config))?;
    dispatch()
}

fn dispatch_confidential_with_cap<T, F>(
    samples: usize,
    audio_seconds: f64,
    dispatch: F,
) -> Result<T, TranscribeError>
where
    F: FnOnce() -> Result<T, TranscribeError>,
{
    if !confidential::confidential_request_fits(samples) {
        return Err(TranscribeError::ConfidentialDeferred {
            reason: "confidential_audio_too_long".to_owned(),
            detail: format!(
                "audio is {:.1}s and would send {} bytes; confidential STT accepts at most {} bytes",
                audio_seconds,
                confidential::confidential_request_bytes(samples),
                confidential::CONFIDENTIAL_STT_MAX_REQUEST_BYTES,
            ),
        });
    }
    dispatch()
}

fn dispatch_backend(
    config: &JournalConfigRead,
    journal_path: &Path,
    backend: &str,
    statement_audio: &[f32],
    attestation_state: &AttestationStateStore,
) -> Result<(parakeet_cpp::TranscriptionResponse, parakeet_cpp::ModelInfo), TranscribeError> {
    if uses_parakeet_cpp(backend) {
        return dispatch_after_egress_gate(config, backend, || {
            let wav = audio_to_wav_bytes(statement_audio, SAMPLE_RATE).map_err(|error| {
                TranscribeError::ParakeetCppFailure {
                    reason: "wav_encode_failed".to_owned(),
                    detail: error.to_string(),
                }
            })?;
            let server = parakeet_cpp::connect(journal_path)?;
            let transcription = parakeet_cpp::transcribe(&server, &wav)?;
            let model_info =
                parakeet_cpp::get_model_info(journal_path, parakeet_cpp_device(config).as_deref())?;
            Ok((transcription, model_info))
        });
    }
    {
        #[cfg(target_os = "macos")]
        if backend == "parakeet" && std::env::consts::ARCH == "aarch64" {
            return dispatch_after_egress_gate(config, backend, || {
                let transcription = parakeet_coreml::transcribe(statement_audio, config)?;
                let model_info = parakeet_coreml::get_model_info(config)?;
                Ok((transcription, model_info))
            });
        }
    }
    if backend == "confidential" {
        return dispatch_after_egress_gate(config, backend, || {
            confidential::transcribe(statement_audio, journal_path, config, attestation_state)
        });
    }
    Err(TranscribeError::BackendNotImplemented {
        backend: backend.to_owned(),
    })
}

fn uses_parakeet_cpp(backend: &str) -> bool {
    uses_parakeet_cpp_for(std::env::consts::OS, backend)
}

fn uses_parakeet_cpp_for(os: &str, backend: &str) -> bool {
    backend == "parakeet-cpp" || (backend == "parakeet" && os == "linux")
}

struct BackendErrorContext<'a> {
    audio_path: &'a Path,
    journal_path: &'a Path,
    audio_seconds: f64,
    reduced_seconds: Option<f64>,
    timings: &'a Timings,
    vad: &'a solstone_core_observe_audio::VadResult,
}

fn emit_backend_error(
    context: BackendErrorContext<'_>,
    error: &TranscribeError,
    backend: Option<&str>,
) {
    let outcome = if error.exit_code() == 69 {
        TranscribedOutcome::Deferred
    } else {
        TranscribedOutcome::Failed
    };
    let reason = backend_error_reason(error);
    emit_event(
        context.audio_path,
        context.journal_path,
        outcome,
        Some(reason),
        None,
        Some(error),
        backend,
        None,
        None,
        Some(context.audio_seconds),
        context.reduced_seconds,
        None,
        context.timings,
        Some(context.vad),
    );
}

fn backend_error_reason(error: &TranscribeError) -> &str {
    match error {
        TranscribeError::ParakeetCppDeferred { reason, .. }
        | TranscribeError::ParakeetCppFailure { reason, .. }
        | TranscribeError::ParakeetCoremlDeferred { reason, .. }
        | TranscribeError::ParakeetCoremlFailure { reason, .. }
        | TranscribeError::ConfidentialDeferred { reason, .. } => reason,
        TranscribeError::SttSurface { .. } => "stt_surface",
        TranscribeError::BackendNotImplemented { .. } => "backend_not_implemented",
        _ => "backend_error",
    }
}

fn statements_from_words(words: &[parakeet_cpp::TranscriptionWord]) -> Vec<Map<String, Value>> {
    let mut statements = Vec::new();
    let mut current = Vec::new();
    for word in words {
        current.push(word);
        if word.word.trim_end().ends_with(['.', '?', '!']) {
            statements.push(statement_from_words(statements.len() as i64 + 1, &current));
            current.clear();
        }
    }
    if !current.is_empty() {
        statements.push(statement_from_words(statements.len() as i64 + 1, &current));
    }
    statements
}

fn statement_from_words(id: i64, words: &[&parakeet_cpp::TranscriptionWord]) -> Map<String, Value> {
    let first = words.first().expect("statement words are nonempty");
    let last = words.last().expect("statement words are nonempty");
    Map::from_iter([
        ("id".to_owned(), Value::from(id)), ("start".to_owned(), Value::from(first.start)),
        ("end".to_owned(), Value::from(last.end)),
        ("text".to_owned(), Value::String(words.iter().map(|word| word.word.as_str()).collect::<String>().trim().to_owned())),
        ("words".to_owned(), Value::Array(words.iter().map(|word| serde_json::json!({"word": word.word, "start": word.start, "end": word.end, "probability": word.probability})).collect())),
        ("speaker".to_owned(), Value::Null),
    ])
}

#[allow(clippy::too_many_arguments)]
fn emit_event(
    audio_path: &Path,
    journal_path: &Path,
    outcome: TranscribedOutcome,
    reason: Option<&str>,
    error_label: Option<&str>,
    error: Option<&TranscribeError>,
    backend: Option<&str>,
    device: Option<&str>,
    model: Option<&str>,
    audio_seconds: Option<f64>,
    reduced_seconds: Option<f64>,
    duration_ms: Option<u64>,
    timings: &Timings,
    vad: Option<&solstone_core_observe_audio::VadResult>,
) {
    let observer = std::env::var("OBSERVER_NAME").ok();
    let input = journal_relative(journal_path, audio_path);
    let output = (outcome == TranscribedOutcome::Transcribed)
        .then(|| journal_relative(journal_path, &audio_path.with_extension("jsonl")));
    let event = build_transcribed_event(TranscribedEvent {
        outcome,
        input: &input,
        output: output.as_deref(),
        reason,
        error,
        backend,
        device,
        model,
        audio_seconds,
        reduced_seconds,
        vad_result: vad,
        duration_ms,
        day: day_from_path(audio_path).as_deref(),
        segment: segment_from_path(audio_path).as_deref(),
        observer: observer.as_deref(),
        timings,
        peak_rss_mib: peak_rss_mib(),
    });
    let _ = error_label;
    let _ = emit_transcribed_event(audio_path.parent().unwrap_or(journal_path), event);
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}
fn queue_wait_ms() -> Option<u64> {
    std::env::var("SOL_QUEUE_WAIT_MS").ok()?.parse().ok()
}
fn segment_meta() -> Option<Map<String, Value>> {
    std::env::var("SEGMENT_META")
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.as_object().cloned())
}
fn source_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    stem.strip_suffix("_audio")
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
fn base_time_us_of_day(path: &Path) -> u64 {
    segment_from_path(path)
        .and_then(|segment| segment.get(..6).map(str::to_owned))
        .and_then(|time| {
            let h = time[0..2].parse::<u64>().ok()?;
            let m = time[2..4].parse::<u64>().ok()?;
            let s = time[4..6].parse::<u64>().ok()?;
            Some((h * 3600 + m * 60 + s) * 1_000_000)
        })
        .unwrap_or(0)
}
fn segment_from_path(path: &Path) -> Option<String> {
    path.parent()?
        .file_name()?
        .to_str()
        .filter(|name| is_segment_name(name))
        .map(str::to_owned)
}
fn day_from_path(path: &Path) -> Option<String> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .find(|part| part.len() == 8 && part.bytes().all(|byte| byte.is_ascii_digit()))
        .map(str::to_owned)
}
fn is_segment_name(value: &str) -> bool {
    let Some((time, length)) = value.split_once('_') else {
        return false;
    };
    time.len() == 6
        && !length.is_empty()
        && time.bytes().all(|byte| byte.is_ascii_digit())
        && length.bytes().all(|byte| byte.is_ascii_digit())
}
fn journal_relative(journal: &Path, path: &Path) -> String {
    path.strip_prefix(journal)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
fn peak_rss_mib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
        })
        .map(|kib| kib / 1024)
        .unwrap_or(0)
}

/// Handle VAD's no-speech terminal branch.
pub(crate) fn vad_no_speech(
    facts: &InputFacts,
    redo: bool,
    sound_tags: Option<&Value>,
) -> Result<TerminalOutcome, TranscribeError> {
    vad_no_speech_with(
        facts,
        redo,
        sound_tags,
        |bytes| {
            solstone_core_speaker_id::writer::write_request(bytes)
                .map_err(TerminalWriteFailure::Typed)
        },
        remove_orphan_npz,
    )
}

/// Handle STT's zero-statement terminal branch.
pub(crate) fn stt_zero_statements(
    facts: &InputFacts,
    redo: bool,
    sound_tags: Option<&Value>,
) -> Result<TerminalOutcome, TranscribeError> {
    stt_zero_statements_with(
        facts,
        redo,
        sound_tags,
        |bytes| {
            solstone_core_speaker_id::writer::write_request(bytes)
                .map_err(TerminalWriteFailure::Typed)
        },
        remove_orphan_npz,
    )
}

/// Write decode failure proof without removing the original input.
pub(crate) fn decode_failure(
    facts: &InputFacts,
    redo: bool,
) -> Result<TerminalOutcome, TranscribeError> {
    let (jsonl_path, npz_path) = transcript_paths(&facts.path);
    write_terminal(TerminalWrite {
        raw_path: &facts.path,
        jsonl_path: &jsonl_path,
        npz_path: &npz_path,
        processing: &corrupt_input_record(facts.input_size),
        sound_tags: None,
        redo,
    })?;
    Ok(TerminalOutcome::Failed)
}

pub(crate) fn transcript_paths(audio_path: &Path) -> (PathBuf, PathBuf) {
    (
        audio_path.with_extension("jsonl"),
        audio_path.with_extension("npz"),
    )
}

fn vad_no_speech_with<W, O>(
    facts: &InputFacts,
    redo: bool,
    sound_tags: Option<&Value>,
    writer: W,
    orphan_remover: O,
) -> Result<TerminalOutcome, TranscribeError>
where
    W: FnOnce(&[u8]) -> Result<WriteResponse, TerminalWriteFailure>,
    O: FnOnce(&Path, &Path) -> Result<(), TranscribeError>,
{
    terminal_empty(
        facts,
        EmptyReason::NoSpeech,
        redo,
        sound_tags,
        writer,
        orphan_remover,
    )
}

fn stt_zero_statements_with<W, O>(
    facts: &InputFacts,
    redo: bool,
    sound_tags: Option<&Value>,
    writer: W,
    orphan_remover: O,
) -> Result<TerminalOutcome, TranscribeError>
where
    W: FnOnce(&[u8]) -> Result<WriteResponse, TerminalWriteFailure>,
    O: FnOnce(&Path, &Path) -> Result<(), TranscribeError>,
{
    terminal_empty(
        facts,
        EmptyReason::NoTranscript,
        redo,
        sound_tags,
        writer,
        orphan_remover,
    )
}

fn terminal_empty<W, O>(
    facts: &InputFacts,
    reason: EmptyReason,
    redo: bool,
    sound_tags: Option<&Value>,
    writer: W,
    orphan_remover: O,
) -> Result<TerminalOutcome, TranscribeError>
where
    W: FnOnce(&[u8]) -> Result<WriteResponse, TerminalWriteFailure>,
    O: FnOnce(&Path, &Path) -> Result<(), TranscribeError>,
{
    let (jsonl_path, npz_path) = transcript_paths(&facts.path);
    let processing = empty_record(facts.input_size, reason);
    write_terminal_with(
        TerminalWrite {
            raw_path: &facts.path,
            jsonl_path: &jsonl_path,
            npz_path: &npz_path,
            processing: &processing,
            sound_tags,
            redo,
        },
        writer,
        orphan_remover,
    )?;
    Ok(TerminalOutcome::Preserved)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use chrono::DateTime;
    use serde_json::{Value, json};
    use solstone_core_observe_audio::VadResult;
    use solstone_core_processing_record::vocab;
    use solstone_core_processing_record::{
        TerminalProofOutcome, evaluate_terminal_proof, is_failure_exhausted,
    };
    use solstone_core_speaker_id::writer::SpeakerTranscriptWriteError;

    use super::{
        BackendErrorContext, InputFacts, TerminalOutcome, capture_input_facts, decode_failure,
        emit_backend_error, remove_orphan_npz, stt_zero_statements, stt_zero_statements_with,
        transcript_paths, vad_no_speech, vad_no_speech_with,
    };
    use crate::TranscribeError;
    use crate::event::Timings;
    use crate::terminal::TerminalWriteFailure;

    #[test]
    fn overlapping_transcription_waits_then_rechecks_the_published_result() {
        use solstone_core_journal_io::{LockOptions, hold_lock};
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("audio.wav");
        fs::write(&audio, b"not decoded by the waiting attempt").unwrap();
        let output = audio.with_extension("jsonl");
        let claim = hold_lock(
            &output,
            LockOptions {
                timeout: Duration::from_secs(1),
                poll_interval: Duration::from_millis(10),
                mode: Some(0o600),
            },
        )
        .unwrap();
        let root = dir.path().to_path_buf();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let config = solstone_core_journal_config::read_journal_config(&root).unwrap();
            started_tx.send(()).unwrap();
            done_tx
                .send(super::process_one(
                    &audio,
                    &root,
                    false,
                    None,
                    &config,
                    &solstone_core_spp_ratls::AttestationStateStore::new(),
                ))
                .unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            matches!(
                done_rx.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "a second attempt must wait before decoding or publishing"
        );
        let published = b"{\"raw\":\"audio.wav\"}\n";
        fs::write(&output, published).unwrap();
        drop(claim);
        assert_eq!(
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            super::ProcessOutcome::Skipped
        );
        worker.join().unwrap();
        assert_eq!(fs::read(output).unwrap(), published);
    }

    fn input(directory: &Path) -> InputFacts {
        let path = directory.join("clip.wav");
        fs::write(&path, b"owner-media").expect("input must be writable");
        capture_input_facts(&path).expect("input facts must be captured")
    }

    fn typed_payload_failure(
        _: &[u8],
    ) -> Result<solstone_core_speaker_id::writer::WriteResponse, TerminalWriteFailure> {
        Err(TerminalWriteFailure::Typed(
            SpeakerTranscriptWriteError::PayloadInvalid {
                path: "injected".to_owned(),
                detail: "injected failure".to_owned(),
            },
        ))
    }

    fn untyped_failure(
        _: &[u8],
    ) -> Result<solstone_core_speaker_id::writer::WriteResponse, TerminalWriteFailure> {
        Err(TerminalWriteFailure::Untyped {
            detail: "injected generic failure".to_owned(),
        })
    }

    #[test]
    fn vad_no_speech_typed_write_failure_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);

        let error = vad_no_speech_with(
            &facts,
            false,
            None,
            typed_payload_failure,
            remove_orphan_npz,
        )
        .unwrap_err();

        assert_eq!(error.exit_code(), 69);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
    }

    #[test]
    fn vad_no_speech_untyped_write_failure_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);

        let error = vad_no_speech_with(&facts, false, None, untyped_failure, remove_orphan_npz)
            .unwrap_err();

        assert_eq!(error.exit_code(), 1);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
    }

    #[test]
    fn stt_zero_statements_typed_write_failure_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);

        let error = stt_zero_statements_with(
            &facts,
            false,
            None,
            typed_payload_failure,
            remove_orphan_npz,
        )
        .unwrap_err();

        assert_eq!(error.exit_code(), 69);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
    }

    #[test]
    fn stt_zero_statements_untyped_write_failure_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);

        let error =
            stt_zero_statements_with(&facts, false, None, untyped_failure, remove_orphan_npz)
                .unwrap_err();

        assert_eq!(error.exit_code(), 1);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
    }

    #[test]
    fn vad_no_speech_leaves_raw() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());

        assert_eq!(
            vad_no_speech(&facts, false, None).unwrap(),
            TerminalOutcome::Preserved
        );
        assert!(facts.path.exists());
        let header: Value =
            serde_json::from_str(&fs::read_to_string(facts.path.with_extension("jsonl")).unwrap())
                .unwrap();
        assert_eq!(
            header["_solstone_processing"]["reason_code"],
            vocab::REASON_NO_SPEECH
        );
    }

    #[test]
    fn stt_zero_statements_leaves_raw() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());

        assert_eq!(
            stt_zero_statements(&facts, false, None).unwrap(),
            TerminalOutcome::Preserved
        );
        assert!(facts.path.exists());
        let header: Value =
            serde_json::from_str(&fs::read_to_string(facts.path.with_extension("jsonl")).unwrap())
                .unwrap();
        assert_eq!(
            header["_solstone_processing"]["reason_code"],
            vocab::REASON_NO_TRANSCRIPT
        );
    }

    #[test]
    fn payload_failure_leaves_no_terminal_artifacts_and_cleans_temporary_payload() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);
        let payload_path = std::cell::RefCell::new(None);

        let error = vad_no_speech_with(
            &facts,
            false,
            None,
            |bytes| {
                let request: Value = serde_json::from_slice(bytes).unwrap();
                *payload_path.borrow_mut() = Some(PathBuf::from(
                    request["embeddings"]["payload_path"]
                        .as_str()
                        .expect("terminal request must include payload path"),
                ));
                typed_payload_failure(bytes)
            },
            remove_orphan_npz,
        )
        .unwrap_err();

        assert_eq!(error.exit_code(), 69);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
        assert!(
            !payload_path
                .into_inner()
                .expect("terminal writer must receive a payload path")
                .exists()
        );
        let entries = fs::read_dir(temporary.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(entries, vec![facts.path]);
    }

    #[test]
    fn terminal_record_holds_only_for_captured_input_size() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);

        assert_eq!(
            vad_no_speech(&facts, false, None).unwrap(),
            TerminalOutcome::Preserved
        );
        let header = read_header(&jsonl_path);
        let record = header
            .get("_solstone_processing")
            .expect("terminal header must include processing record");

        assert_eq!(
            evaluate_terminal_proof(Some(record), vocab::HANDLER_TRANSCRIBE, facts.input_size),
            TerminalProofOutcome::Held
        );
        assert_eq!(
            evaluate_terminal_proof(
                Some(record),
                vocab::HANDLER_TRANSCRIBE,
                facts.input_size + 1
            ),
            TerminalProofOutcome::Refused
        );
    }

    #[test]
    fn vad_no_speech_preserves_sound_tags_in_terminal_header() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);
        let sound_tags = json!({"tags": {"Music": 0.9}});

        assert_eq!(
            vad_no_speech(&facts, false, Some(&sound_tags)).unwrap(),
            TerminalOutcome::Preserved
        );

        assert_eq!(read_header(&jsonl_path)["sound_tags"], sound_tags);
    }

    #[test]
    fn decode_failure_writes_terminal_failed_record_without_removing_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);

        assert_eq!(
            decode_failure(&facts, false).unwrap(),
            TerminalOutcome::Failed
        );
        assert!(facts.path.exists());
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["state"], vocab::STATE_FAILED);
        assert_eq!(record["reason_code"], vocab::REASON_CORRUPT_INPUT);
        assert!(is_failure_exhausted(&record));
    }

    #[test]
    fn orphan_npz_is_removed_before_terminal_write() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (_, npz_path) = transcript_paths(&facts.path);
        fs::write(&npz_path, b"orphan").unwrap();

        assert_eq!(
            vad_no_speech(&facts, false, None).unwrap(),
            TerminalOutcome::Preserved
        );
        assert!(!npz_path.exists());
    }

    #[test]
    fn orphan_npz_removal_failure_stops_write_and_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);
        fs::write(&npz_path, b"orphan").unwrap();
        let orphan_path = npz_path.clone();

        let error = vad_no_speech_with(
            &facts,
            false,
            None,
            |_| panic!("writer must not run after orphan removal failure"),
            move |_, _| {
                Err(TranscribeError::OrphanNpzRemove {
                    path: orphan_path,
                    detail: "injected removal failure".to_owned(),
                })
            },
        )
        .unwrap_err();

        assert_eq!(error.exit_code(), 75);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(npz_path.exists());
    }

    #[test]
    fn terminal_record_omits_attempts_and_has_parseable_attempted_at() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);

        vad_no_speech(&facts, false, None).unwrap();
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();

        assert!(record.get("attempts").is_none());
        DateTime::parse_from_rfc3339(
            record["attempted_at"]
                .as_str()
                .expect("attempted_at must be a string"),
        )
        .expect("attempted_at must be RFC 3339");
    }

    #[test]
    fn confidential_egress_gate_prevents_backend_dispatch() {
        let config = solstone_core_journal_config::JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(
                serde_json::json!({
                    "services": {"confidential": {}},
                    "providers": {"local": {"credential": "present"}},
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        };
        let mut dispatched = false;
        let error = super::dispatch_after_egress_gate(&config, "remote", || {
            dispatched = true;
            Ok::<_, TranscribeError>(())
        })
        .unwrap_err();
        assert_eq!(error.exit_code(), 69);
        assert!(!dispatched);
    }

    #[test]
    fn confidential_duration_cap_prevents_backend_dispatch() {
        let mut dispatched = false;
        // Oversized by BYTES now, which is the shim's actual contract.
        let error = super::dispatch_confidential_with_cap(usize::MAX / 4, 99_999.0, || {
            dispatched = true;
            Ok::<_, TranscribeError>(())
        })
        .unwrap_err();

        let TranscribeError::ConfidentialDeferred { reason, .. } = error else {
            panic!("duration cap must defer");
        };
        assert_eq!(reason, "confidential_audio_too_long");
        assert!(!dispatched);
    }

    /// The confidential guard must mirror the shim's REAL contract: request bytes.
    ///
    /// `asr_shim.py` bounds a request at `MAX_REQUEST_BYTES = 11 MiB` and has no
    /// duration limit at all. The client mirrored that as a flat 300 s cap, which
    /// refused 301-305 s recordings the server would have accepted -- 60 of them on
    /// the founder's journal, every one comfortably inside the byte budget.
    #[test]
    fn the_confidential_guard_uses_the_shims_byte_budget() {
        let sample_rate = f64::from(solstone_core_observe_audio::SAMPLE_RATE);
        // A 302s recording is what was being refused; it must now dispatch.
        let samples_302s = (302.0 * sample_rate) as usize;
        let mut dispatched = false;
        super::dispatch_confidential_with_cap(samples_302s, 302.0, || {
            dispatched = true;
            Ok::<_, TranscribeError>(())
        })
        .expect("a 302s recording fits the 11 MiB budget");
        assert!(dispatched, "302s must reach the backend");

        // 🔒 Negative twin: genuinely oversized audio is still refused. The founder's
        // journal holds a 5626s segment, which is far past the budget.
        let samples_5626s = (5626.0 * sample_rate) as usize;
        let error = super::dispatch_confidential_with_cap(samples_5626s, 5626.0, || {
            Ok::<_, TranscribeError>(())
        })
        .unwrap_err();
        let TranscribeError::ConfidentialDeferred { reason, .. } = error else {
            panic!("an oversized request must defer");
        };
        assert_eq!(reason, "confidential_audio_too_long");

        // The boundary itself is the shim's, not a duration.
        assert!(super::super::backend::confidential::confidential_request_fits(0));
        assert!(!super::super::backend::confidential::confidential_request_fits(usize::MAX / 4));
    }

    #[test]
    fn disabled_confidential_lane_prevents_backend_dispatch() {
        let config = solstone_core_journal_config::JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(
                serde_json::json!({
                    "services": {"confidential": {}},
                    "transcribe": {"confidential_audio": false},
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        };
        let mut dispatched = false;
        let error = super::dispatch_after_egress_gate(&config, "confidential", || {
            dispatched = true;
            Ok::<_, TranscribeError>(())
        })
        .unwrap_err();

        let TranscribeError::ConfidentialDeferred { reason, .. } = error else {
            panic!("disabled confidential lane must defer");
        };
        assert_eq!(reason, "confidential_audio_disabled");
        assert!(!dispatched);
    }

    #[test]
    fn inactive_confidential_lane_prevents_backend_dispatch() {
        let config = solstone_core_journal_config::JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(serde_json::Map::new()),
        };
        let mut dispatched = false;
        let error = super::dispatch_after_egress_gate(&config, "confidential", || {
            dispatched = true;
            Ok::<_, TranscribeError>(())
        })
        .unwrap_err();

        let TranscribeError::ConfidentialDeferred { reason, .. } = error else {
            panic!("inactive confidential lane must defer");
        };
        assert_eq!(reason, "confidential_lane_inactive");
        assert!(!dispatched);
    }

    #[test]
    fn parakeet_cpp_routing_covers_linux_and_darwin_in_one_host_test() {
        assert!(super::uses_parakeet_cpp_for("linux", "parakeet-cpp"));
        assert!(super::uses_parakeet_cpp_for("linux", "parakeet"));
        assert!(!super::uses_parakeet_cpp_for("darwin", "parakeet"));
        assert!(super::uses_parakeet_cpp_for("darwin", "parakeet-cpp"));
        assert!(!super::uses_parakeet_cpp_for("linux", "confidential"));
    }

    #[test]
    fn backend_errors_emit_once_with_known_fields_only() {
        let temporary = tempfile::tempdir().unwrap();
        let segment = temporary.path().join("chronicle/20260810/audio/120000_60");
        fs::create_dir_all(&segment).unwrap();
        let audio = segment.join("clip.wav");
        fs::write(&audio, b"audio").unwrap();
        let timings = Timings::default();
        let vad = VadResult {
            duration_s: 1.0,
            speech_duration_s: 1.0,
            has_speech: true,
            speech_segments: vec![(0.0, 1.0)],
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        };
        let resolution = TranscribeError::SttSurface {
            available_bytes: None,
            floor_bytes: None,
        };
        let coreml = TranscribeError::ParakeetCoremlDeferred {
            reason: "coreml_helper_missing".to_owned(),
            detail: "test".to_owned(),
        };

        let context = || BackendErrorContext {
            audio_path: &audio,
            journal_path: temporary.path(),
            audio_seconds: 1.0,
            reduced_seconds: None,
            timings: &timings,
            vad: &vad,
        };
        emit_backend_error(context(), &resolution, None);
        emit_backend_error(context(), &coreml, Some("parakeet"));

        let events: Vec<Value> = fs::read_to_string(segment.join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["outcome"], "failed");
        assert_eq!(events[0]["reason"], "stt_surface");
        assert!(events[0].get("backend").is_none());
        assert!(events[0].get("device").is_none());
        assert!(events[0].get("model").is_none());
        assert_eq!(events[1]["outcome"], "deferred");
        assert_eq!(events[1]["reason"], "coreml_helper_missing");
        assert_eq!(events[1]["backend"], "parakeet");
        assert!(events[1].get("device").is_none());
        assert!(events[1].get("model").is_none());
    }

    fn read_header(path: &Path) -> Value {
        let contents = fs::read_to_string(path).expect("terminal JSONL must be readable");
        serde_json::from_str(contents.lines().next().expect("JSONL must contain header"))
            .expect("header must be JSON")
    }
}
