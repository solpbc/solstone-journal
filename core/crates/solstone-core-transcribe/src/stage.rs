// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Terminal stage outcomes. Empty terminals retain the raw input.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use solstone_core_journal_config::JournalConfigRead;
use solstone_core_journal_io::LockOptions;
use solstone_core_observe_audio::{AudioError, SAMPLE_RATE, audio_to_wav_bytes, decode_f32_mono};
use solstone_core_processing_record::{read_processing_record_header, record_attempts};
use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
use solstone_core_speaker_id::writer::WriteResponse;
use solstone_core_spp_ratls::AttestationStateStore;

use crate::TranscribeError;
use crate::args::should_skip_process_one_processed;
use crate::audio::{run_vad, tag_audio};
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
use crate::processing::{
    EmptyReason, corrupt_input_record, decode_transient_record, empty_record,
    no_audio_stream_record,
};
use crate::speakers::analyze_speakers;
use crate::terminal::{TerminalWrite, TerminalWriteFailure, write_terminal, write_terminal_with};
use crate::transcript::{
    FullTranscriptWrite, remove_orphan_npz, restore_statement_timestamps, write_full_transcript,
};
use solstone_core_observe_audio::reduce_audio;

const SPEAKER_EVIDENCE_VERSION: &str = "windowed-slots-v1";
const SPEAKER_ANALYSIS_PRODUCER: &str = "solstone-core-speakers-analyze";
const OVERLAP_DETECTOR: &str = "pyannote-segmentation-3.0";

/// Immutable facts captured from the owner-media file before processing starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputFacts {
    pub(crate) path: PathBuf,
    pub(crate) input_size: u64,
    /// Members every processing record this attempt writes carries (voice-only fields).
    pub(crate) record_extra: Map<String, Value>,
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
        record_extra: Map::new(),
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

/// A single-file driver disposition; failed output must yield a nonzero CLI status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessOutcome {
    Skipped,
    Transcribed,
    Preserved,
    Failed,
}

fn commit_input<'a>(
    input: &InputObservation,
    claims: &'a OutputClaims,
    raw: &Path,
    checkpoint: &dyn Fn() -> Result<(), TranscribeError>,
) -> Result<solstone_core_segment::output_attempt::SegmentCommit<'a>, TranscribeError> {
    #[cfg(all(test, feature = "full-tests"))]
    BEFORE_PROCESSING_COMMIT.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    let commit = input
        .commit(claims)
        .map_err(|error| TranscribeError::ProcessingClaim {
            path: raw.to_path_buf(),
            detail: error.to_string(),
        })?;
    // Segment exclusion can wait after the caller's previous checkpoint.
    checkpoint()?;
    Ok(commit)
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static BEFORE_PROCESSING_COMMIT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static PROCESSING_TEST_VAD: std::cell::RefCell<Option<solstone_core_observe_audio::VadResult>> = const { std::cell::RefCell::new(None) };
}

/// Run the currently implemented native transcription path for one segment audio file.
pub(crate) fn process_one(
    audio_path: &Path,
    journal_path: &Path,
    redo: bool,
    explicit_backend: Option<&str>,
    config: &JournalConfigRead,
    attestation_state: &AttestationStateStore,
    #[cfg(windows)] generation: &solstone_core_system::process::ChildLaunchContext,
) -> Result<ProcessOutcome, TranscribeError> {
    process_one_with(
        audio_path,
        journal_path,
        redo,
        explicit_backend,
        config,
        attestation_state,
        #[cfg(windows)]
        generation,
        #[cfg(all(feature = "embedded", unix))]
        None,
        None,
    )
}

fn finish_context_output(
    journal_path: &Path,
    output: &Path,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
    audio_path: &Path,
    checkpoint: &dyn Fn() -> Result<(), TranscribeError>,
) -> Result<(), TranscribeError> {
    checkpoint()?;
    if let Some(attempt) = attempt {
        let unit = solstone_core_system::work_retry::raw::unit_for_output(
            journal_path,
            output,
            solstone_core_processing_record::vocab::HANDLER_TRANSCRIBE,
        )
        .map_err(|detail| TranscribeError::ProcessingClaim {
            path: audio_path.to_path_buf(),
            detail,
        })?;
        solstone_core_system::work_retry::raw::finish_output_checked(
            journal_path,
            &unit,
            attempt,
            &|| checkpoint().map_err(|error| error.to_string()),
        )
        .map_err(|detail| {
            checkpoint()
                .err()
                .unwrap_or_else(|| TranscribeError::ProcessingClaim {
                    path: audio_path.to_path_buf(),
                    detail,
                })
        })?;
    }
    Ok(())
}

// Read only while holding this transcript's output claim. Ordinary retries
// have no explicit retry context, but own replacement of their failed output.
fn automatic_retry_replaces_output(output: &Path) -> bool {
    use solstone_core_processing_record::{should_reenter_analysis_output, vocab};
    read_processing_record_header(output).is_some_and(|record| {
        record["schema"] == vocab::SCHEMA
            && record["handler"] == vocab::HANDLER_TRANSCRIBE
            && should_reenter_analysis_output(Some(&record), output, vocab::HANDLER_TRANSCRIBE)
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn process_one_with(
    audio_path: &Path,
    journal_path: &Path,
    redo: bool,
    explicit_backend: Option<&str>,
    config: &JournalConfigRead,
    attestation_state: &AttestationStateStore,
    #[cfg(windows)] generation: &solstone_core_system::process::ChildLaunchContext,
    #[cfg(all(feature = "embedded", unix))] embedded: Option<&crate::embedded::EmbeddedContext<'_>>,
    retry: Option<&solstone_core_system::work_retry::RetryContext>,
) -> Result<ProcessOutcome, TranscribeError> {
    let checkpoint = || {
        #[cfg(all(feature = "embedded", unix))]
        if let Some(runtime) = embedded {
            runtime.checkpoint()?;
        }
        Ok(())
    };

    #[cfg(all(feature = "embedded", unix))]
    if let Some(runtime) = embedded {
        runtime.checkpoint()?;
    }
    if should_skip_process_one_processed(audio_path, redo, Some(journal_path), retry) {
        return Ok(ProcessOutcome::Skipped);
    }
    // Live and catch-up dispatchers have separate queues. The output owner must
    // serialize the entire attempt, including the completed-output check, so a
    // duplicate cannot run inference before losing an exclusive publication race.
    // This kernel lock is released on process death; the sidecar is never removed.
    // Allow an ordinary Sense transcription's 45-minute execution budget to finish.
    let output = audio_path.with_extension("jsonl");
    let claim = OutputClaims::acquire(
        [output.clone()],
        LockOptions {
            timeout: {
                #[cfg(all(feature = "embedded", unix))]
                if embedded.is_some() {
                    Duration::ZERO
                } else {
                    Duration::from_secs(45 * 60)
                }
                #[cfg(not(all(feature = "embedded", unix)))]
                {
                    Duration::from_secs(45 * 60)
                }
            },
            ..LockOptions::default()
        },
    )
    .map_err(|error| TranscribeError::ProcessingClaim {
        path: audio_path.to_path_buf(),
        detail: error.to_string(),
    })?;
    if should_skip_process_one_processed(audio_path, redo, Some(journal_path), retry) {
        return Ok(ProcessOutcome::Skipped);
    }
    // Voice-only eligibility is decided here, under the output claim and before any retry
    // permission is consumed, from the output and the setting as they are on disk now.
    let owner_voice_setting = crate::config::read_transcribe_config(journal_path)
        .map(|config| crate::config::owner_voice_only(&config))
        .map_err(|error| TranscribeError::ProcessingClaim {
            path: audio_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let owner_voice_eligibility =
        crate::owner_voice::eligibility(audio_path, &output, owner_voice_setting);
    if owner_voice_eligibility == crate::owner_voice::Eligibility::Final {
        return Ok(ProcessOutcome::Skipped);
    }
    let owner_voice_print = if owner_voice_eligibility == crate::owner_voice::Eligibility::Filtered
    {
        match solstone_core_speaker_resolve::owner_centroid::owner_voice_print(journal_path) {
            Some(print) => Some(print),
            None => {
                let first_time = !read_processing_record_header(&output)
                    .as_ref()
                    .is_some_and(solstone_core_processing_record::owner_voice_filtered);
                let error = TranscribeError::OwnerVoiceUnavailable {
                    detail: if first_time {
                        "save a voice in Speakers, or turn off transcribe.owner_voice_only".into()
                    } else {
                        "save a voice in Speakers to finish this filtered recording".into()
                    },
                };
                emit_event(
                    audio_path,
                    journal_path,
                    TranscribedOutcome::Deferred,
                    Some("owner_voice_unavailable"),
                    Some("OwnerVoiceUnavailable"),
                    Some(&error),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    &Timings::default(),
                    None,
                );
                return Err(error);
            }
        }
    } else {
        None
    };
    let automatic_retry = automatic_retry_replaces_output(&output);
    let observation = InputObservation::capture(&claim, audio_path).map_err(|error| {
        TranscribeError::ProcessingClaim {
            path: audio_path.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    let started = Instant::now();
    let (admitted_retry, context_attempt) = if !redo {
        if let Some(context) = retry {
            let attempt = solstone_core_system::work_retry::raw::consume_context_checked(
                journal_path,
                &audio_path.with_extension("jsonl"),
                solstone_core_processing_record::vocab::HANDLER_TRANSCRIBE,
                context,
                &|| checkpoint().map_err(|error: TranscribeError| error.to_string()),
            )
            .map_err(|detail| TranscribeError::ProcessingClaim {
                path: audio_path.to_path_buf(),
                detail,
            })?;
            (attempt.is_some(), attempt)
        } else {
            let admitted = solstone_core_system::work_retry::raw::consume_from_environment(
                journal_path,
                &audio_path.with_extension("jsonl"),
                solstone_core_processing_record::vocab::HANDLER_TRANSCRIBE,
            )
            .map_err(|detail| TranscribeError::ProcessingClaim {
                path: audio_path.to_path_buf(),
                detail,
            })?;
            (admitted, None)
        }
    } else {
        (false, None)
    };
    // Keep both ordinary and explicitly admitted retry authority through
    // successful, empty and failed publication under this output claim.
    let redo = redo || admitted_retry || automatic_retry;
    let mut facts = InputFacts {
        path: audio_path.to_path_buf(),
        input_size: observation.input_size(),
        record_extra: Map::new(),
    };
    if owner_voice_print.is_some() {
        facts.record_extra = crate::owner_voice::flag_extra();
    }
    let mut timings = Timings::default();
    if let Some(queue_wait_ms) = queue_wait_ms() {
        timings.add_ms("queue_wait", queue_wait_ms);
    }
    #[cfg(all(feature = "embedded", unix))]
    let record_embedded_refusal = |error: &TranscribeError, timings: &Timings| {
        checkpoint()?;
        let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
        backend_failure(&facts, redo, error, context_attempt.as_ref())?;
        drop(commit);
        finish_context_output(
            journal_path,
            &facts.path.with_extension("jsonl"),
            context_attempt.as_ref(),
            audio_path,
            &checkpoint,
        )?;
        emit_event(
            audio_path,
            journal_path,
            TranscribedOutcome::Deferred,
            Some(backend_error_reason(error)),
            None,
            Some(error),
            None,
            None,
            None,
            None,
            None,
            None,
            timings,
            None,
        );
        Ok::<(), TranscribeError>(())
    };
    #[cfg(all(feature = "embedded", unix))]
    if embedded.is_some() && facts.input_size > 16 * 1024 * 1024 {
        let error = TranscribeError::ParakeetCppDeferred {
            reason: solstone_core_processing_record::vocab::REASON_EMBEDDED_INPUT.into(),
            detail: "embedded audio exceeds 16 MiB input budget; original retained".into(),
        };
        record_embedded_refusal(&error, &timings)?;
        return Err(error);
    }
    let decoded_at = Instant::now();
    #[cfg(all(feature = "embedded", unix))]
    let decoded = if embedded.is_some() {
        solstone_core_observe_audio::decode_f32_mono_bounded(
            audio_path,
            crate::embedded::EMBEDDED_AUDIO_MAX_SAMPLES,
        )
    } else {
        decode_f32_mono(audio_path)
    };
    #[cfg(not(all(feature = "embedded", unix)))]
    let decoded = decode_f32_mono(audio_path);
    let full_audio = match decoded {
        Err(AudioError::DecodedSampleLimit { .. }) => {
            let error = TranscribeError::ParakeetCppDeferred {
                reason: solstone_core_processing_record::vocab::REASON_EMBEDDED_DURATION.into(),
                detail: "embedded decoded sample budget exceeds 120 seconds; original retained"
                    .into(),
            };
            #[cfg(all(feature = "embedded", unix))]
            record_embedded_refusal(&error, &timings)?;
            return Err(error);
        }
        Ok(audio) => audio,
        Err(error) => {
            timings.add_ms("decode", elapsed_ms(decoded_at));
            let decode_error = TranscribeError::Decode {
                detail: error.to_string(),
            };
            let write_at = Instant::now();
            #[cfg(all(feature = "embedded", unix))]
            if let Some(runtime) = embedded {
                runtime.checkpoint()?;
            }
            let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
            let terminal = decode_failure(&facts, redo, &error, context_attempt.as_ref())?;
            drop(commit);
            finish_context_output(
                journal_path,
                &facts.path.with_extension("jsonl"),
                context_attempt.as_ref(),
                audio_path,
                &checkpoint,
            )?;
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
    #[cfg(all(feature = "embedded", unix))]
    if let Some(runtime) = embedded {
        runtime.checkpoint()?;
    }
    let vad_at = Instant::now();
    let compute_vad = || {
        #[cfg(all(feature = "embedded", unix))]
        if let Some(runtime) = embedded {
            return crate::audio::run_vad_embedded(
                &full_audio,
                min_speech_seconds(config),
                runtime.models,
                runtime.scratch,
            );
        }
        run_vad(
            &full_audio,
            min_speech_seconds(config),
            #[cfg(windows)]
            generation,
        )
    };
    #[cfg(all(test, feature = "full-tests"))]
    let vad = PROCESSING_TEST_VAD
        .with(|result| result.borrow_mut().take())
        .map_or_else(compute_vad, Ok)?;
    #[cfg(not(all(test, feature = "full-tests")))]
    let vad = compute_vad()?;
    timings.add_ms("vad", elapsed_ms(vad_at));
    #[cfg(all(feature = "embedded", unix))]
    if let Some(runtime) = embedded {
        runtime.checkpoint()?;
    }
    // Voice-only: zero what is left out before anything below sees the audio.
    let (full_audio, vad) = match owner_voice_print.as_ref() {
        None => (full_audio, vad),
        Some(print) => {
            let filter_at = Instant::now();
            let step = crate::owner_voice::filter_audio(
                audio_path,
                full_audio,
                &vad,
                &print.values,
                |audio, statements| {
                    first_pass_analysis(
                        audio_path,
                        audio,
                        statements,
                        #[cfg(all(feature = "embedded", unix))]
                        embedded,
                        #[cfg(windows)]
                        generation,
                    )
                },
            );
            timings.add_ms("owner_voice_filter", elapsed_ms(filter_at));
            #[cfg(all(feature = "embedded", unix))]
            if let Some(runtime) = embedded {
                runtime.checkpoint()?;
            }
            let step = match step {
                Ok(step) => step,
                Err(crate::owner_voice::FirstPassProblem::HelperOutdated) => {
                    let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
                    let mut processing = crate::processing::backend_failure_record(
                        facts.input_size,
                        solstone_core_processing_record::vocab::REASON_ANALYSIS_FAILED,
                    );
                    processing["error"] = json!(
                        "the speaker-analysis helper predates overlap ranges or speaker turns"
                    );
                    write_failed_record(&facts, redo, processing, context_attempt.as_ref())?;
                    drop(commit);
                    finish_context_output(
                        journal_path,
                        &facts.path.with_extension("jsonl"),
                        context_attempt.as_ref(),
                        audio_path,
                        &checkpoint,
                    )?;
                    emit_event(
                        audio_path,
                        journal_path,
                        TranscribedOutcome::Failed,
                        Some("owner_voice_helper_outdated"),
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
                    return Ok(ProcessOutcome::Failed);
                }
                Err(crate::owner_voice::FirstPassProblem::Unusable(error)) => {
                    let error = TranscribeError::SpeakerAnalysis(error);
                    emit_event(
                        audio_path,
                        journal_path,
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
                        &timings,
                        Some(&vad),
                    );
                    return Err(error);
                }
            };
            let raw_name = audio_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let filter = crate::owner_voice::filter_record(
                &step.speech,
                &step.decision,
                raw_name,
                facts.input_size,
                &observation.input_sha256(),
                &print.values,
            );
            facts.record_extra = crate::owner_voice::success_extra(&filter);
            let masked_vad = masked_audio_vad(
                &step.audio,
                config,
                #[cfg(all(feature = "embedded", unix))]
                embedded,
                #[cfg(windows)]
                generation,
            )?;
            (step.audio, masked_vad)
        }
    };
    let (sound_tags, sound_tag_status) = tag_audio(&full_audio, journal_path);

    if !vad.has_speech {
        let write_at = Instant::now();
        #[cfg(all(feature = "embedded", unix))]
        if let Some(runtime) = embedded {
            runtime.checkpoint()?;
        }
        let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
        let terminal = vad_no_speech(
            &facts,
            redo,
            sound_tags.as_ref(),
            sound_tag_status.as_ref(),
            context_attempt.as_ref(),
        )?;
        drop(observation);
        finish_voice_only_deletion(
            journal_path,
            &facts,
            &claim,
            &commit,
            #[cfg(all(feature = "embedded", unix))]
            embedded,
        );
        drop(commit);
        finish_context_output(
            journal_path,
            &facts.path.with_extension("jsonl"),
            context_attempt.as_ref(),
            audio_path,
            &checkpoint,
        )?;
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

    let resolve_backend = || {
        resolve_default_backend(
            explicit_backend,
            local_stt_backend(),
            read_available_bytes(),
            platform_floor_bytes(),
            confidential::confidential_channel_plausible(config),
            confidential_audio_enabled(config),
        )
        .map(|resolution| resolution.backend)
    };
    #[cfg(all(feature = "embedded", unix))]
    let backend =
        crate::embedded::select_backend(resolve_backend(), embedded, explicit_backend, config);
    #[cfg(not(all(feature = "embedded", unix)))]
    let backend = resolve_backend();
    let backend = backend.inspect_err(|error| {
        emit_backend_error(
            BackendErrorContext {
                audio_path,
                journal_path,
                audio_seconds,
                reduced_seconds: None,
                timings: &timings,
                vad: &vad,
            },
            error,
            None,
        );
    })?;
    // Trim long non-speech gaps after owner masking, preserving boundary context.
    // Keep the original masked timeline for diarization and transcript publication.
    let reduce_at = Instant::now();
    let reduction = reduce_audio(&full_audio, &vad);
    let (reduced_audio, reduction) = reduction
        .map(|(audio, mapping)| (Some(audio), Some(mapping)))
        .unwrap_or((None, None));
    if reduction.is_some() {
        timings.add_ms("reduce", elapsed_ms(reduce_at));
    }
    let statement_audio = reduced_audio.as_deref().unwrap_or(&full_audio);
    let reduced_seconds = reduction.as_ref().map(|mapping| mapping.reduced_duration_s);

    let mut asr_at = None;
    let dispatch = |samples: &[f32]| {
        asr_at.get_or_insert_with(Instant::now);
        #[cfg(all(feature = "embedded", unix))]
        if let Some(runtime) = embedded.filter(|_| backend != "confidential") {
            return runtime.transcribe(samples);
        }
        dispatch_backend(
            config,
            journal_path,
            &backend,
            samples,
            attestation_state,
            #[cfg(all(feature = "embedded", unix))]
            embedded,
        )
    };
    let dispatch = || {
        if backend == "confidential" {
            // Keep the combined byte admission before any request. Splitting retained
            // spans must not bypass the existing over-budget retained-raw deferral.
            dispatch_confidential_with_cap(
                statement_audio.len(),
                reduced_seconds.unwrap_or(audio_seconds),
                || transcribe_retained_intervals(statement_audio, reduction.as_ref(), dispatch),
            )
        } else {
            transcribe_retained_intervals(statement_audio, reduction.as_ref(), dispatch)
        }
    };
    let dispatched = dispatch();
    if let Some(asr_at) = asr_at {
        timings.add_ms("asr", elapsed_ms(asr_at));
    }
    let (transcription, model_info) = match dispatched {
        Ok(result) => result,
        Err(error) => {
            #[cfg(all(feature = "embedded", unix))]
            if let Some(runtime) = embedded {
                runtime.checkpoint()?;
            }
            if matches!(error, TranscribeError::Cancelled) {
                return Err(error);
            }
            let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
            backend_failure(&facts, redo, &error, context_attempt.as_ref())?;
            drop(commit);
            finish_context_output(
                journal_path,
                &facts.path.with_extension("jsonl"),
                context_attempt.as_ref(),
                audio_path,
                &checkpoint,
            )?;
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
        #[cfg(all(feature = "embedded", unix))]
        if let Some(runtime) = embedded {
            runtime.checkpoint()?;
        }
        let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
        let terminal = stt_zero_statements(
            &facts,
            redo,
            sound_tags.as_ref(),
            sound_tag_status.as_ref(),
            context_attempt.as_ref(),
        )?;
        drop(observation);
        finish_voice_only_deletion(
            journal_path,
            &facts,
            &claim,
            &commit,
            #[cfg(all(feature = "embedded", unix))]
            embedded,
        );
        drop(commit);
        finish_context_output(
            journal_path,
            &facts.path.with_extension("jsonl"),
            context_attempt.as_ref(),
            audio_path,
            &checkpoint,
        )?;
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
    #[cfg(all(feature = "embedded", unix))]
    let speaker_analysis = if let Some(runtime) = embedded {
        runtime.checkpoint()?;
        crate::speakers::analyze_speakers_embedded(
            audio_path,
            &full_audio,
            statement_audio,
            reduced_audio.as_deref(),
            &statements,
            &restored,
            runtime.models,
            runtime.scratch,
        )
    } else {
        analyze_speakers(
            audio_path,
            &full_audio,
            statement_audio,
            reduced_audio.as_deref(),
            &statements,
            &restored,
            SAMPLE_RATE,
            crate::speakers::MIN_STATEMENT_DURATION_S,
        )
    };
    #[cfg(not(all(feature = "embedded", unix)))]
    let speaker_analysis = analyze_speakers(
        audio_path,
        &full_audio,
        statement_audio,
        reduced_audio.as_deref(),
        &statements,
        &restored,
        SAMPLE_RATE,
        crate::speakers::MIN_STATEMENT_DURATION_S,
        #[cfg(windows)]
        generation,
    );
    let speaker_result = match speaker_analysis {
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
    let mut processing = analyzed_record(facts.input_size);
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
    #[cfg(all(feature = "embedded", unix))]
    if let Some(runtime) = embedded {
        runtime.checkpoint()?;
    }
    let commit = commit_input(&observation, &claim, audio_path, &checkpoint)?;
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
        sound_tag_status: sound_tag_status.as_ref(),
        speaker_analysis_producer: Some(SPEAKER_ANALYSIS_PRODUCER),
        redo,
    })?;
    drop(observation);
    finish_voice_only_deletion(
        journal_path,
        &facts,
        &claim,
        &commit,
        #[cfg(all(feature = "embedded", unix))]
        embedded,
    );
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

/// The voice-only first pass: the ordinary speaker analysis over the unmasked audio, with
/// one synthetic statement per scored window, through the same transport as today.
fn first_pass_analysis(
    audio_path: &Path,
    audio: &[f32],
    statements: &[Map<String, Value>],
    #[cfg(all(feature = "embedded", unix))] embedded: Option<&crate::embedded::EmbeddedContext<'_>>,
    #[cfg(windows)] generation: &solstone_core_system::process::ChildLaunchContext,
) -> Result<crate::speakers::SpeakerAnalyzeResult, crate::speakers::SpeakerAnalyzeError> {
    #[cfg(all(feature = "embedded", unix))]
    if let Some(runtime) = embedded {
        return crate::speakers::analyze_speakers_embedded(
            audio_path,
            audio,
            audio,
            None,
            statements,
            statements,
            runtime.models,
            runtime.scratch,
        );
    }
    analyze_speakers(
        audio_path,
        audio,
        audio,
        None,
        statements,
        statements,
        SAMPLE_RATE,
        crate::speakers::MIN_STATEMENT_DURATION_S,
        #[cfg(windows)]
        generation,
    )
}

/// VAD over the voice-only masked audio, through the same transport as the first VAD.
fn masked_audio_vad(
    audio: &[f32],
    config: &JournalConfigRead,
    #[cfg(all(feature = "embedded", unix))] embedded: Option<&crate::embedded::EmbeddedContext<'_>>,
    #[cfg(windows)] generation: &solstone_core_system::process::ChildLaunchContext,
) -> Result<solstone_core_observe_audio::VadResult, TranscribeError> {
    #[cfg(all(feature = "embedded", unix))]
    if let Some(runtime) = embedded {
        return crate::audio::run_vad_embedded(
            audio,
            min_speech_seconds(config),
            runtime.models,
            runtime.scratch,
        );
    }
    run_vad(
        audio,
        min_speech_seconds(config),
        #[cfg(windows)]
        generation,
    )
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

fn transcribe_retained_intervals<F>(
    audio: &[f32],
    reduction: Option<&solstone_core_observe_audio::AudioReduction>,
    mut dispatch: F,
) -> Result<(parakeet_cpp::TranscriptionResponse, parakeet_cpp::ModelInfo), TranscribeError>
where
    F: FnMut(
        &[f32],
    ) -> Result<
        (parakeet_cpp::TranscriptionResponse, parakeet_cpp::ModelInfo),
        TranscribeError,
    >,
{
    let Some(reduction) = reduction else {
        return dispatch(audio);
    };
    let mut combined = parakeet_cpp::TranscriptionResponse {
        words: Vec::new(),
        text: String::new(),
    };
    let mut model_info = None;
    // Recognize each continuous retained interval once. Joining unrelated intervals
    // changes utterance normalization/context and can blank an otherwise audible clip.
    for segment in &reduction.segments {
        let start = (segment.reduced_start_s * f64::from(SAMPLE_RATE)).round() as usize;
        let end = (segment.reduced_end_s * f64::from(SAMPLE_RATE)).round() as usize;
        let (mut transcript, info) = dispatch(&audio[start..end])?;
        for word in &mut transcript.words {
            // Bound each local result to its own interval. Offset addition can round
            // an exclusive end past a cut into the next original interval.
            word.start = (word.start + segment.reduced_start_s)
                .clamp(segment.reduced_start_s, segment.reduced_end_s);
            word.end = (word.end + segment.reduced_start_s)
                .clamp(segment.reduced_start_s, segment.reduced_end_s);
        }
        if !combined.text.is_empty() && !transcript.text.is_empty() {
            combined.text.push(' ');
        }
        combined.text.push_str(&transcript.text);
        combined.words.extend(transcript.words);
        model_info = Some(info);
    }
    Ok((
        combined,
        model_info.expect("a speech reduction contains retained intervals"),
    ))
}

fn dispatch_backend(
    config: &JournalConfigRead,
    journal_path: &Path,
    backend: &str,
    statement_audio: &[f32],
    attestation_state: &AttestationStateStore,
    #[cfg(all(feature = "embedded", unix))] embedded: Option<&crate::embedded::EmbeddedContext<'_>>,
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
            #[cfg(all(feature = "embedded", unix))]
            if let Some(runtime) = embedded {
                return confidential::transcribe_cancellable(
                    statement_audio,
                    journal_path,
                    config,
                    attestation_state,
                    &|| runtime.checkpoint(),
                    runtime.service_io,
                );
            }
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
    backend == "parakeet-cpp" || (backend == "parakeet" && matches!(os, "linux" | "windows"))
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
        TranscribeError::OwnerVoiceUnavailable { .. } => "owner_voice_unavailable",
        _ => "backend_error",
    }
}

pub(crate) fn statements_from_words(
    words: &[parakeet_cpp::TranscriptionWord],
) -> Vec<Map<String, Value>> {
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
fn audio_capture_meta() -> Option<Map<String, Value>> {
    let capture = segment_meta()?.remove("audio_capture")?;
    Some(Map::from_iter([("audio_capture".to_owned(), capture)]))
}
fn source_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    stem.strip_suffix("_audio")
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
fn base_time_us_of_day(path: &Path) -> u64 {
    let base = segment_from_path(path)
        .and_then(|segment| segment.get(..6).filter(|s| s.is_ascii()).map(str::to_owned))
        .and_then(|time| {
            let h = time[0..2].parse::<u64>().ok()?;
            let m = time[2..4].parse::<u64>().ok()?;
            let s = time[4..6].parse::<u64>().ok()?;
            Some((h * 3600 + m * 60 + s) * 1_000_000)
        })
        .unwrap_or(0);

    let offset_us = (|| -> Option<u64> {
        let parent = path.parent()?;
        let file_name = path.file_name()?.to_str()?;
        let placement_file =
            parent.join(solstone_core_format::audio_archive::AUDIO_ARCHIVE_PLACEMENT_FILENAME);
        let content = std::fs::read_to_string(placement_file).ok()?;
        let placement = serde_json::from_str::<
            solstone_core_format::audio_archive::AudioArchivePlacement,
        >(&content)
        .ok()?;
        let part = placement
            .parts
            .iter()
            .find(|part| part.stored_name == file_name)?;
        (part.offset_ms >= 0).then(|| (part.offset_ms as u64) * 1_000)
    })()
    .unwrap_or(0);

    base + offset_us
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
    sound_tag_status: Option<&solstone_core_local::install::capability_status::CapabilityStatus>,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<TerminalOutcome, TranscribeError> {
    vad_no_speech_with(
        facts,
        redo,
        sound_tags,
        sound_tag_status,
        attempt,
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
    sound_tag_status: Option<&solstone_core_local::install::capability_status::CapabilityStatus>,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<TerminalOutcome, TranscribeError> {
    stt_zero_statements_with(
        facts,
        redo,
        sound_tags,
        sound_tag_status,
        attempt,
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
    error: &AudioError,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<TerminalOutcome, TranscribeError> {
    let mut processing = match error {
        AudioError::NoAudioStream { .. } => no_audio_stream_record(facts.input_size),
        _ if error.is_transient_decode() => decode_transient_record(facts.input_size),
        _ => corrupt_input_record(facts.input_size),
    };
    if let Some(errno) = error.ffmpeg_errno() {
        processing["errno"] = json!(errno);
    }
    write_failed_record(facts, redo, processing, attempt)
}

fn backend_failure(
    facts: &InputFacts,
    redo: bool,
    error: &TranscribeError,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<TerminalOutcome, TranscribeError> {
    let mut processing =
        crate::processing::backend_failure_record(facts.input_size, backend_error_reason(error));
    processing["error"] = json!(error.to_string());
    write_failed_record(facts, redo, processing, attempt)
}

fn write_failed_record(
    facts: &InputFacts,
    redo: bool,
    mut processing: Value,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<TerminalOutcome, TranscribeError> {
    let (jsonl_path, npz_path) = transcript_paths(&facts.path);
    let prior_header = read_processing_record_header(&jsonl_path);
    let prior_attempts = prior_header.as_ref().map(record_attempts).unwrap_or(0);
    let base = std::cmp::max(0, prior_attempts);
    processing["attempts"] = json!(base + 1);
    merge_record_extra(&mut processing, &facts.record_extra);

    write_terminal(TerminalWrite {
        raw_path: &facts.path,
        jsonl_path: &jsonl_path,
        npz_path: &npz_path,
        processing: &processing,
        sound_tags: None,
        sound_tag_status: None,
        segment_meta: audio_capture_meta().as_ref(),
        redo: redo || jsonl_path.exists(),
        attempt,
    })?;
    Ok(TerminalOutcome::Failed)
}

/// After a voice-only success is published, delete its original through retention while this
/// attempt still holds the transcript's claim and segment commit. Whatever happens, the
/// published words stand; an unfinished deletion stays owed for the daily cleanup.
fn finish_voice_only_deletion(
    journal_path: &Path,
    facts: &InputFacts,
    claim: &OutputClaims,
    commit: &solstone_core_segment::output_attempt::SegmentCommit<'_>,
    #[cfg(all(feature = "embedded", unix))] embedded: Option<&crate::embedded::EmbeddedContext<'_>>,
) {
    use solstone_core_retention::owner_voice::{OwnerVoiceDeletion, finish_owner_voice_deletion};
    if !facts
        .record_extra
        .contains_key(solstone_core_processing_record::OWNER_VOICE_KEY)
    {
        return;
    }
    let jsonl = facts.path.with_extension("jsonl");
    match finish_owner_voice_deletion(journal_path, &jsonl, claim, commit, chrono::Utc::now()) {
        outcome @ (OwnerVoiceDeletion::Deleted | OwnerVoiceDeletion::DeletedUnrecorded) => {
            if outcome == OwnerVoiceDeletion::DeletedUnrecorded {
                log::warn!("voice-only original deleted but deletion event was not recorded");
            }
            #[cfg(all(feature = "embedded", unix))]
            if let Some(runtime) = embedded {
                (runtime.owner_original_deleted)();
            }
        }
        OwnerVoiceDeletion::Gone => {}
        outcome => log::warn!("voice-only original not deleted now: {outcome:?}"),
    }
}

fn merge_record_extra(processing: &mut Value, extra: &Map<String, Value>) {
    if let Some(record) = processing.as_object_mut() {
        for (key, value) in extra {
            record.insert(key.clone(), value.clone());
        }
    }
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
    sound_tag_status: Option<&solstone_core_local::install::capability_status::CapabilityStatus>,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
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
        sound_tag_status,
        attempt,
        writer,
        orphan_remover,
    )
}

fn stt_zero_statements_with<W, O>(
    facts: &InputFacts,
    redo: bool,
    sound_tags: Option<&Value>,
    sound_tag_status: Option<&solstone_core_local::install::capability_status::CapabilityStatus>,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
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
        sound_tag_status,
        attempt,
        writer,
        orphan_remover,
    )
}

#[allow(clippy::too_many_arguments)]
fn terminal_empty<W, O>(
    facts: &InputFacts,
    reason: EmptyReason,
    redo: bool,
    sound_tags: Option<&Value>,
    sound_tag_status: Option<&solstone_core_local::install::capability_status::CapabilityStatus>,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
    writer: W,
    orphan_remover: O,
) -> Result<TerminalOutcome, TranscribeError>
where
    W: FnOnce(&[u8]) -> Result<WriteResponse, TerminalWriteFailure>,
    O: FnOnce(&Path, &Path) -> Result<(), TranscribeError>,
{
    let (jsonl_path, npz_path) = transcript_paths(&facts.path);
    let mut processing = empty_record(facts.input_size, reason);
    merge_record_extra(&mut processing, &facts.record_extra);
    write_terminal_with(
        TerminalWrite {
            raw_path: &facts.path,
            jsonl_path: &jsonl_path,
            npz_path: &npz_path,
            processing: &processing,
            sound_tags,
            sound_tag_status,
            segment_meta: audio_capture_meta().as_ref(),
            redo,
            attempt,
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

    #[test]
    fn base_time_us_of_day_reads_placement_offset() {
        let temp = tempfile::tempdir().unwrap();
        let segment_dir = temp
            .path()
            .join("chronicle/20260811/import.archive/120000_300");
        std::fs::create_dir_all(&segment_dir).unwrap();
        let raw_file = segment_dir.join("p0000_0123456789abcdef.opus");
        std::fs::write(&raw_file, b"audio").unwrap();

        let segment_clock = 12 * 3600 * 1_000_000;
        // Without placement: segment clock only
        assert_eq!(super::base_time_us_of_day(&raw_file), segment_clock);

        // With placement: segment clock plus 90 seconds in microseconds
        let placement = serde_json::json!({
            "version": 1,
            "parts": [
                {
                    "stored_name": "p0000_0123456789abcdef.opus",
                    "original_occurrence": "rec1",
                    "origin_ms": 0,
                    "offset_ms": 90_000,
                    "length_ms": 1000
                }
            ]
        });
        std::fs::write(segment_dir.join("placement.json"), placement.to_string()).unwrap();

        assert_eq!(
            super::base_time_us_of_day(&raw_file),
            segment_clock + 90 * 1_000_000
        );
    }

    use chrono::DateTime;
    use serde_json::{Value, json};
    use solstone_core_observe_audio::{AudioError, VadResult};
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

    #[cfg(all(feature = "embedded", unix))]
    #[test]
    fn only_committed_owner_deletion_completes_embedded_custody() {
        use std::cell::Cell;
        use std::os::unix::fs::PermissionsExt;

        use sha2::{Digest, Sha256};
        use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};

        // This exercises publication/retention/checkpoint lifetime only. The
        // recognizer must not run; physical owner tests execute the real models.
        struct UnusedAsr;
        impl crate::embedded::EmbeddedAsr for UnusedAsr {
            fn transcribe(&self, _: &[f32]) -> Result<Value, String> {
                panic!("no inference belongs to a publication lifetime test")
            }
            fn model_info(&self) -> crate::embedded::EmbeddedModelInfo {
                panic!("no model belongs to a publication lifetime test")
            }
        }

        for state in ["empty", "analyzed"] {
            for change in [
                "none",
                "unrecorded",
                "missing",
                "changed",
                "interrupted",
                "reappeared",
                "reappeared_identical",
            ] {
                let root = tempfile::tempdir().unwrap();
                let segment = root.path().join("chronicle/20261023/device_audio/120000_1");
                fs::create_dir_all(&segment).unwrap();
                let raw = segment.join("audio.wav");
                let audio = b"synthetic owner audio";
                fs::write(&raw, audio).unwrap();
                let mut facts = capture_input_facts(&raw).unwrap();
                let owner = json!({
                    "version": 1, "speech_s": 1.0, "kept_s": 1.0, "left_out_s": 0.0,
                    "windows_scored": 1, "windows_kept": 1,
                    "original": {"name": "audio.wav", "size": audio.len(),
                        "sha256": format!("{:x}", Sha256::digest(audio))},
                    "print": {"sha256": "0".repeat(64)}
                });
                facts
                    .record_extra
                    .insert("owner_voice".into(), owner.clone());
                let jsonl = raw.with_extension("jsonl");
                let claims = OutputClaims::acquire([jsonl.clone()], Default::default()).unwrap();
                let observation = InputObservation::capture(&claims, &raw).unwrap();
                let commit = observation.commit(&claims).unwrap();
                let header = json!({"raw": "audio.wav", "_solstone_processing": {
                    "schema": vocab::SCHEMA, "handler": vocab::HANDLER_TRANSCRIBE,
                    "state": state, "reason_code": if state == "empty" { "no_speech" } else { "ok" },
                    "input_size": audio.len(), "owner_voice_filter": true, "owner_voice": owner
                }});
                let mut published = format!("{header}\n");
                if state == "analyzed" {
                    published
                        .push_str("{\"start\":\"12:00:00\",\"text\":\"public fixture owner\"}\n");
                }
                fs::write(&jsonl, &published).unwrap();
                drop(observation);

                let authorized = Cell::new(false);
                let interrupted = Cell::new(false);
                let cancel = || {
                    interrupted.get()
                        || if authorized.get() {
                            !matches!(fs::symlink_metadata(&raw), Err(error)
                                if error.kind() == std::io::ErrorKind::NotFound)
                        } else {
                            match fs::read(&raw) {
                                Ok(current) => current != audio,
                                Err(_) => true,
                            }
                        }
                };
                let notify = || authorized.set(true);
                let runtime = crate::embedded::EmbeddedContext {
                    models: root.path(),
                    scratch: root.path(),
                    asr: &UnusedAsr,
                    cancel: &cancel,
                    service_io: &|_| {},
                    owner_original_deleted: &notify,
                };
                let events = segment.join("events.jsonl");
                match change {
                    "missing" => fs::remove_file(&raw).unwrap(),
                    "changed" => fs::write(&raw, b"different owner bytes").unwrap(),
                    "unrecorded" => {
                        fs::write(&events, b"").unwrap();
                        fs::set_permissions(&events, fs::Permissions::from_mode(0o444)).unwrap();
                        // A privileged test user can bypass mode bits. That
                        // environment cannot exercise this append-failure control.
                        if fs::OpenOptions::new().append(true).open(&events).is_ok() {
                            eprintln!(
                                "skip unrecorded-event control: test user can append to a read-only file"
                            );
                            continue;
                        }
                    }
                    _ => (),
                }
                super::finish_voice_only_deletion(
                    root.path(),
                    &facts,
                    &claims,
                    &commit,
                    Some(&runtime),
                );
                assert_eq!(
                    authorized.get(),
                    !matches!(change, "missing" | "changed"),
                    "{state}/{change}"
                );
                if change == "unrecorded" {
                    assert!(fs::read(&events).unwrap().is_empty());
                }
                if change == "interrupted" {
                    interrupted.set(true);
                } else if change == "reappeared" {
                    fs::write(&raw, b"different owner bytes").unwrap();
                } else if change == "reappeared_identical" {
                    fs::write(&raw, audio).unwrap();
                }
                drop(commit);
                let finished =
                    super::finish_context_output(root.path(), &jsonl, None, &raw, &|| {
                        runtime.checkpoint()
                    });
                assert_eq!(
                    finished.is_ok(),
                    matches!(change, "none" | "unrecorded"),
                    "{state}/{change}"
                );
                assert_eq!(
                    fs::read_to_string(&jsonl).unwrap(),
                    published,
                    "{state}/{change}"
                );
                assert_eq!(
                    raw.exists(),
                    matches!(change, "changed" | "reappeared" | "reappeared_identical"),
                    "{state}/{change}"
                );
            }
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn admitted_transcript_retry_can_publish_through_the_real_writer() {
        use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
        use solstone_core_system::work_retry::{self, raw};
        if std::env::var_os("TRANSCRIBE_RETRY_TEST_CHILD").is_none() {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "stage::tests::admitted_transcript_retry_can_publish_through_the_real_writer",
                    "--nocapture",
                ])
                .env("TRANSCRIBE_RETRY_TEST_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}",
                String::from_utf8_lossy(&child.stderr)
            );
            assert!(
                String::from_utf8_lossy(&child.stdout).contains("1 passed"),
                "{}",
                String::from_utf8_lossy(&child.stdout)
            );
            return;
        }
        for empty in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let segment = root
                .path()
                .join("chronicle")
                .join("20260101")
                .join("desktop")
                .join("120000_300");
            fs::create_dir_all(&segment).unwrap();
            let facts = input(&segment);
            let (jsonl, npz) = transcript_paths(&facts.path);
            let raw_name = facts.path.file_name().unwrap().to_str().unwrap();
            let header = json!({"raw":raw_name,"_solstone_processing":{"schema":vocab::SCHEMA,"state":"failed","handler":"transcribe","reason_code":"analysis_failed","attempts":3}});
            fs::write(&jsonl, format!("{header}\n")).unwrap();
            let evidence = raw::inspect_output(root.path(), &jsonl).unwrap().unwrap();
            raw::observe(root.path(), &evidence).unwrap();
            work_retry::request(root.path(), &evidence.unit).unwrap();
            let id = evidence.unit.id().unwrap();
            let attempt = work_retry::read(root.path(), &id)
                .unwrap()
                .unwrap()
                .permission(&evidence.input, &[])
                .unwrap();
            temp_env::with_vars(
                [
                    (raw::JOURNAL_ENV, Some(root.path().to_str().unwrap())),
                    (raw::UNIT_ENV, Some(id.as_str())),
                    (raw::CAUSE_ENV, Some(attempt.cause.as_str())),
                    (
                        raw::ATTEMPT_ENV,
                        Some(serde_json::to_string(&attempt).unwrap().as_str()),
                    ),
                ],
                || {
                    let claims =
                        OutputClaims::acquire([jsonl.clone()], Default::default()).unwrap();
                    let input = InputObservation::capture(&claims, &facts.path).unwrap();
                    let admitted =
                        raw::consume_from_environment(root.path(), &jsonl, "transcribe").unwrap();
                    assert!(admitted);
                    // A consumed attempt without a new native publication is interrupted.
                    assert!(raw::finish_output(root.path(), &evidence.unit, &attempt).is_err());
                    let _commit = input.commit(&claims).unwrap();
                    if empty {
                        vad_no_speech(&facts, admitted, None, None, None).unwrap();
                    } else {
                        let result = crate::speakers::SpeakerAnalyzeResult {
                            statements: vec![
                                json!({"id":1,"start":0.0,"end":1.0,"text":"recovered words"})
                                    .as_object()
                                    .unwrap()
                                    .clone(),
                            ],
                            embedding_payload: None,
                            speaker_evidence: "none".into(),
                            speaker_evidence_multi_fraction: 0.0,
                            overlap_fraction: 0.0,
                            statement_labels: None,
                            turn_ranges: None,
                            overlap_ranges: None,
                        };
                        let mut processing = crate::processing::analyzed_record(facts.input_size);
                        raw::stamp_attempt(&mut processing).unwrap();
                        super::write_full_transcript(super::FullTranscriptWrite {
                            raw_path: &facts.path,
                            jsonl_path: &jsonl,
                            npz_path: &npz,
                            base_time_us_of_day: 0,
                            source: None,
                            speaker_result: &result,
                            backend: Some("parakeet-cpp"),
                            model: Some("parakeet"),
                            device: Some("cpu"),
                            compute_type: None,
                            observer: None,
                            vad_result: None,
                            segment_meta: None,
                            overlap_detector: None,
                            speaker_evidence_version: "windowed-slots-v1",
                            processing: &processing,
                            sound_tags: None,
                            sound_tag_status: None,
                            speaker_analysis_producer: None,
                            redo: admitted,
                        })
                        .unwrap();
                    }
                },
            );
            raw::finish_output(root.path(), &evidence.unit, &attempt).unwrap();
            assert!(!work_retry::read(root.path(), &id).unwrap().unwrap().held);
            let header =
                solstone_core_processing_record::read_processing_record_header(&jsonl).unwrap();
            assert_eq!(header["state"], if empty { "empty" } else { "analyzed" });
        }
    }

    #[cfg(feature = "full-tests")]
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
                    #[cfg(windows)]
                    &solstone_core_system::process::ChildLaunchContext::default(),
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

    #[cfg(feature = "full-tests")]
    #[test]
    fn cancelled_transcription_after_segment_commit_lock_wait_and_positive_twins() {
        use solstone_core_journal_io::{LockOptions, hold_lock};
        use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, mpsc};
        use std::time::Duration;
        for kind in ["vad", "stt", "decode", "backend", "analyzed"] {
            for cancel in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let segment = root.path().join("segment");
                fs::create_dir(&segment).unwrap();
                let facts = input(&segment);
                let original = fs::read(&facts.path).unwrap();
                let (jsonl, npz) = transcript_paths(&facts.path);
                let held = hold_lock(&segment, LockOptions::default()).unwrap();
                let cancelled = Arc::new(AtomicBool::new(false));
                let flag = cancelled.clone();
                let (started, ready) = mpsc::channel();
                let (checkpoint, reached) = mpsc::channel();
                let worker = std::thread::spawn(move || {
                    let claims =
                        OutputClaims::acquire([jsonl.clone()], Default::default()).unwrap();
                    let observed = InputObservation::capture(&claims, &facts.path).unwrap();
                    started.send(()).unwrap();
                    let _commit = super::commit_input(&observed, &claims, &facts.path, &|| {
                        checkpoint.send(()).unwrap();
                        if flag.load(Ordering::SeqCst) {
                            Err(TranscribeError::Cancelled)
                        } else {
                            Ok(())
                        }
                    })?;
                    match kind {
                        "vad" => {
                            vad_no_speech(&facts, false, None, None, None)?;
                        }
                        "stt" => {
                            stt_zero_statements(&facts, false, None, None, None)?;
                        }
                        "decode" => {
                            decode_failure(
                                &facts,
                                false,
                                &AudioError::CorruptInput {
                                    path: facts.path.clone(),
                                    detail: "synthetic corrupt input".into(),
                                    errno: None,
                                },
                                None,
                            )?;
                        }
                        "backend" => {
                            super::backend_failure(
                                &facts,
                                false,
                                &TranscribeError::ConfidentialDeferred {
                                    reason: "local_endpoint_unreachable".into(),
                                    detail: "synthetic transport".into(),
                                },
                                None,
                            )?;
                        }
                        _ => {
                            let result = crate::speakers::SpeakerAnalyzeResult {
                                statements: vec![
                                    json!({"id":1,"start":0.0,"end":1.0,"text":"synthetic words"})
                                        .as_object()
                                        .unwrap()
                                        .clone(),
                                ],
                                embedding_payload: None,
                                speaker_evidence: "none".into(),
                                speaker_evidence_multi_fraction: 0.0,
                                overlap_fraction: 0.0,
                                statement_labels: None,
                                turn_ranges: None,
                                overlap_ranges: None,
                            };
                            let processing = crate::processing::analyzed_record(facts.input_size);
                            super::write_full_transcript(super::FullTranscriptWrite {
                                raw_path: &facts.path,
                                jsonl_path: &jsonl,
                                npz_path: &npz,
                                base_time_us_of_day: 0,
                                source: None,
                                speaker_result: &result,
                                backend: Some("parakeet-cpp"),
                                model: Some("parakeet"),
                                device: Some("cpu"),
                                compute_type: None,
                                observer: None,
                                vad_result: None,
                                segment_meta: None,
                                overlap_detector: None,
                                speaker_evidence_version: "windowed-slots-v1",
                                processing: &processing,
                                sound_tags: None,
                                sound_tag_status: None,
                                speaker_analysis_producer: None,
                                redo: false,
                            })?;
                        }
                    }
                    Ok::<(), TranscribeError>(())
                });
                ready.recv_timeout(Duration::from_secs(2)).unwrap();
                assert!(
                    reached.recv_timeout(Duration::from_millis(100)).is_err(),
                    "checkpoint must follow actual segment exclusion"
                );
                cancelled.store(cancel, Ordering::SeqCst);
                drop(held);
                let result = worker.join().unwrap();
                let output = segment.join("clip.jsonl");
                if cancel {
                    assert!(matches!(result, Err(TranscribeError::Cancelled)));
                    assert!(!output.exists());
                } else {
                    result.unwrap();
                    let record =
                        solstone_core_processing_record::read_processing_record_header(&output)
                            .unwrap();
                    let state = match kind {
                        "vad" | "stt" => "empty",
                        "analyzed" => "analyzed",
                        _ => "failed",
                    };
                    assert_eq!(record["state"], state);
                }
                assert_eq!(fs::read(segment.join("clip.wav")).unwrap(), original);
            }
        }
    }
    #[cfg(feature = "full-tests")]
    #[test]
    fn writer_commit_guards_success_empty_and_failure_publications() {
        use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
        for state in ["analyzed", "empty", "failed"] {
            for mutation in ["unchanged", "bytes", "deleted", "missing", "replaced"] {
                let dir = tempfile::tempdir().unwrap();
                let facts = input(dir.path());
                let (jsonl, npz) = transcript_paths(&facts.path);
                let claims = OutputClaims::acquire([jsonl.clone()], Default::default()).unwrap();
                let observed = InputObservation::capture(&claims, &facts.path).unwrap();
                match mutation {
                    "bytes" => fs::write(&facts.path, b"OWNER-MEDIA").unwrap(),
                    "deleted" => fs::write(dir.path().join("tombstone.json"), b"{}").unwrap(),
                    "missing" => fs::remove_file(&facts.path).unwrap(),
                    "replaced" => {
                        fs::remove_file(&facts.path).unwrap();
                        fs::write(&facts.path, b"owner-media").unwrap();
                    }
                    _ => (),
                }
                let commit = super::commit_input(&observed, &claims, &facts.path, &|| Ok(()));
                if mutation != "unchanged" {
                    assert!(commit.is_err(), "{state}/{mutation}");
                    assert!(!jsonl.exists());
                    continue;
                }
                let _commit = commit.unwrap();
                match state {
                    "empty" => {
                        vad_no_speech(&facts, false, None, None, None).unwrap();
                    }
                    "failed" => {
                        decode_failure(
                            &facts,
                            false,
                            &AudioError::CorruptInput {
                                path: facts.path.clone(),
                                detail: "synthetic corrupt input".into(),
                                errno: None,
                            },
                            None,
                        )
                        .unwrap();
                    }
                    _ => {
                        let result = crate::speakers::SpeakerAnalyzeResult {
                            statements: vec![
                                json!({"id": 1, "start": 0.0, "end": 1.0, "text": "synthetic words"})
                                    .as_object()
                                    .unwrap()
                                    .clone(),
                            ],
                            embedding_payload: None,
                            speaker_evidence: "none".into(),
                            speaker_evidence_multi_fraction: 0.0,
                            overlap_fraction: 0.0,
                            statement_labels: None,
                            turn_ranges: None,
                            overlap_ranges: None,
                        };
                        let processing = crate::processing::analyzed_record(facts.input_size);
                        super::write_full_transcript(super::FullTranscriptWrite {
                            raw_path: &facts.path,
                            jsonl_path: &jsonl,
                            npz_path: &npz,
                            base_time_us_of_day: 0,
                            source: None,
                            speaker_result: &result,
                            backend: Some("parakeet-cpp"),
                            model: Some("parakeet"),
                            device: Some("cpu"),
                            compute_type: None,
                            observer: None,
                            vad_result: None,
                            segment_meta: None,
                            overlap_detector: None,
                            speaker_evidence_version: "windowed-slots-v1",
                            processing: &processing,
                            sound_tags: None,
                            sound_tag_status: None,
                            speaker_analysis_producer: None,
                            redo: false,
                        })
                        .unwrap();
                    }
                }
                let header =
                    solstone_core_processing_record::read_processing_record_header(&jsonl).unwrap();
                assert_eq!(header["handler"], "transcribe");
                assert_eq!(header["state"], state);
                assert_eq!(header["input_size"], facts.input_size);
            }
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn actual_decode_failure_refuses_input_changed_after_admission() {
        for mutation in ["unchanged", "bytes", "deleted"] {
            let dir = tempfile::tempdir().unwrap();
            let raw = dir.path().join("audio.wav");
            fs::write(&raw, b"corrupt audio").unwrap();
            let hook_path = raw.clone();
            super::BEFORE_PROCESSING_COMMIT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || match mutation {
                    "bytes" => fs::write(&hook_path, b"changed audio").unwrap(),
                    "deleted" => {
                        fs::write(hook_path.parent().unwrap().join("tombstone.json"), b"{}")
                            .unwrap()
                    }
                    _ => (),
                }))
            });
            let config = solstone_core_journal_config::read_journal_config(dir.path()).unwrap();
            let result = super::process_one(
                &raw,
                dir.path(),
                false,
                None,
                &config,
                &solstone_core_spp_ratls::AttestationStateStore::new(),
                #[cfg(windows)]
                &solstone_core_system::process::ChildLaunchContext::default(),
            );
            if mutation == "unchanged" {
                assert_eq!(result.unwrap(), super::ProcessOutcome::Failed);
                assert!(raw.with_extension("jsonl").exists());
            } else {
                assert!(result.is_err());
                assert!(!raw.with_extension("jsonl").exists());
            }
        }
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
            None,
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

        let error = vad_no_speech_with(
            &facts,
            false,
            None,
            None,
            None,
            untyped_failure,
            remove_orphan_npz,
        )
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
            None,
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

        let error = stt_zero_statements_with(
            &facts,
            false,
            None,
            None,
            None,
            untyped_failure,
            remove_orphan_npz,
        )
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
            vad_no_speech(&facts, false, None, None, None).unwrap(),
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
            stt_zero_statements(&facts, false, None, None, None).unwrap(),
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
            None,
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
            vad_no_speech(&facts, false, None, None, None).unwrap(),
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
            vad_no_speech(&facts, false, Some(&sound_tags), None, None).unwrap(),
            TerminalOutcome::Preserved
        );

        assert_eq!(read_header(&jsonl_path)["sound_tags"], sound_tags);
        assert!(read_header(&jsonl_path).get("sound_tag_status").is_none());
    }

    #[test]
    fn terminal_header_records_sound_tag_status_on_degraded_status() {
        use solstone_core_installed_payload::code;
        use solstone_core_local::install::capability_status::CapabilityStatus;

        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);

        // 1. Missing member -> Absent
        let status_missing = CapabilityStatus::Absent {
            capability: "ced".to_owned(),
            detail: format!("{}: member missing", code::MEMBER_MISSING),
        };
        assert_eq!(
            vad_no_speech(&facts, false, None, Some(&status_missing), None).unwrap(),
            TerminalOutcome::Preserved
        );
        let header = read_header(&jsonl_path);
        assert!(header.get("sound_tags").is_none());
        assert_eq!(header["sound_tag_status"]["status"], "absent");
        assert!(
            header["sound_tag_status"]["detail"]
                .as_str()
                .unwrap()
                .contains(code::MEMBER_MISSING)
        );

        // 2. Model changed -> IntegrityInvalid
        let status_changed = CapabilityStatus::IntegrityInvalid {
            capability: "ced".to_owned(),
            detail: format!("{}: member changed", code::MEMBER_CHANGED),
        };
        assert_eq!(
            vad_no_speech(&facts, true, None, Some(&status_changed), None).unwrap(),
            TerminalOutcome::Preserved
        );
        let header2 = read_header(&jsonl_path);
        assert!(header2.get("sound_tags").is_none());
        assert_eq!(header2["sound_tag_status"]["status"], "integrity_invalid");
        assert!(
            header2["sound_tag_status"]["detail"]
                .as_str()
                .unwrap()
                .contains(code::MEMBER_CHANGED)
        );
    }

    #[test]
    fn decode_failure_writes_terminal_failed_record_without_removing_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);
        let error = AudioError::EmptyInput {
            path: facts.path.clone(),
        };

        assert_eq!(
            decode_failure(&facts, false, &error, None).unwrap(),
            TerminalOutcome::Failed
        );
        assert!(facts.path.exists());
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["state"], vocab::STATE_FAILED);
        assert_eq!(record["reason_code"], vocab::REASON_CORRUPT_INPUT);
        assert!(is_failure_exhausted(&record));
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn automatic_transient_retry_publishes_empty_without_replacing_terminal_outputs() {
        for (state, attempts, retry) in [
            ("failed", 1, true),
            ("failed", 3, false),
            ("analyzed", 1, false),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let facts = input(temporary.path());
            let audio = vec![0.0; 16_000];
            fs::write(
                &facts.path,
                solstone_core_observe_audio::audio_to_wav_bytes(&audio, 16_000).unwrap(),
            )
            .unwrap();
            let original = fs::read(&facts.path).unwrap();
            let output = facts.path.with_extension("jsonl");
            let header = json!({
                "raw": facts.path.file_name().unwrap().to_str().unwrap(),
                "_solstone_processing": {
                    "schema": vocab::SCHEMA, "handler": vocab::HANDLER_TRANSCRIBE,
                    "state": state, "reason_code": "hosted_transcribe_unreachable",
                    "attempts": attempts, "input_size": original.len()
                }
            });
            let before = format!("{header}\n");
            fs::write(&output, &before).unwrap();
            if retry {
                super::PROCESSING_TEST_VAD.with(|result| {
                    *result.borrow_mut() = Some(VadResult {
                        duration_s: 1.0,
                        speech_duration_s: 0.0,
                        speech_segments: Vec::new(),
                        has_speech: false,
                        noisy_rms: None,
                        noisy_s: 0.0,
                        loud_windows: 0,
                        speech_loud_windows: 0,
                    });
                });
            }
            let outcome = super::process_one_with(
                &facts.path,
                temporary.path(),
                false,
                None,
                &solstone_core_journal_config::JournalConfigRead {
                    present: false,
                    sha256: None,
                    config: None,
                },
                &solstone_core_spp_ratls::AttestationStateStore::new(),
                #[cfg(all(feature = "embedded", unix))]
                None,
                None,
            )
            .unwrap();
            if retry {
                assert_eq!(outcome, super::ProcessOutcome::Preserved);
                let record = read_header(&output)["_solstone_processing"].clone();
                assert_eq!(record["state"], vocab::STATE_EMPTY);
            } else {
                assert_eq!(outcome, super::ProcessOutcome::Skipped);
                assert_eq!(fs::read_to_string(&output).unwrap(), before);
            }
            assert!(super::PROCESSING_TEST_VAD.with(|result| result.borrow().is_none()));
            assert_eq!(fs::read(&facts.path).unwrap(), original);
        }
    }

    #[cfg(all(unix, feature = "embedded", feature = "full-tests"))]
    #[test]
    fn embedded_recognizer_receives_context_padded_speech_without_long_quiet_gaps() {
        use std::cell::RefCell;

        struct Recorder(RefCell<Vec<Vec<f32>>>);
        impl crate::embedded::EmbeddedAsr for Recorder {
            fn transcribe(&self, samples: &[f32]) -> Result<Value, String> {
                self.0.borrow_mut().push(samples.to_vec());
                Ok(json!({"text": "", "words": []}))
            }

            fn model_info(&self) -> crate::embedded::EmbeddedModelInfo {
                crate::embedded::EmbeddedModelInfo {
                    backend: "parakeet-cpp".into(),
                    model: "fixture".into(),
                    device: "cpu".into(),
                    compute_type: "float32".into(),
                }
            }
        }

        for (speech, expected) in [
            (vec![(5.0, 6.0), (15.0, 16.0)], vec![(4, 7), (14, 17)]),
            (vec![(0.0, 20.0)], vec![(0, 20)]),
            (Vec::new(), Vec::new()),
        ] {
            let root = tempfile::tempdir().unwrap();
            let facts = input(root.path());
            let rate = super::SAMPLE_RATE as usize;
            let audio: Vec<f32> = (0..20 * rate)
                .map(|index| (index / rate) as f32 / 32.0)
                .collect();
            fs::write(
                &facts.path,
                solstone_core_observe_audio::audio_to_wav_bytes(&audio, super::SAMPLE_RATE)
                    .unwrap(),
            )
            .unwrap();
            let original = fs::read(&facts.path).unwrap();
            let decoded = solstone_core_observe_audio::decode_f32_mono(&facts.path).unwrap();
            super::PROCESSING_TEST_VAD.with(|result| {
                *result.borrow_mut() = Some(VadResult {
                    duration_s: 20.0,
                    speech_duration_s: speech.iter().map(|(start, end)| end - start).sum(),
                    has_speech: !speech.is_empty(),
                    speech_segments: speech,
                    noisy_rms: None,
                    noisy_s: 0.0,
                    loud_windows: 0,
                    speech_loud_windows: 0,
                });
            });
            let recorder = Recorder(RefCell::new(Vec::new()));
            let runtime = crate::embedded::EmbeddedContext {
                models: root.path(),
                scratch: root.path(),
                asr: &recorder,
                cancel: &|| false,
                service_io: &|_| {},
                owner_original_deleted: &|| {},
            };
            let config = solstone_core_journal_config::JournalConfigRead {
                present: false,
                sha256: None,
                config: None,
            };
            super::process_one_with(
                &facts.path,
                root.path(),
                false,
                None,
                &config,
                &solstone_core_spp_ratls::AttestationStateStore::new(),
                Some(&runtime),
                None,
            )
            .unwrap();

            let calls = recorder.0.borrow();
            if expected.is_empty() {
                assert!(calls.is_empty(), "no-speech must never reach recognition");
            } else {
                assert_eq!(calls.len(), expected.len());
                for (actual, (start, end)) in calls.iter().zip(expected) {
                    assert_eq!(actual.len(), (end - start) * rate);
                    assert_eq!(
                        actual.as_slice(),
                        &decoded[start * rate..end * rate],
                        "each continuous original interval reaches ASR exactly once"
                    );
                }
            }
            assert_eq!(
                fs::read(&facts.path).unwrap(),
                original,
                "raw input is unchanged"
            );
            assert!(super::PROCESSING_TEST_VAD.with(|result| result.borrow().is_none()));
        }
    }

    #[test]
    fn retained_intervals_are_recognized_once_and_restore_word_and_statement_times() {
        let rate = super::SAMPLE_RATE as usize;
        let audio = (0..20 * rate).map(|index| index as f32).collect::<Vec<_>>();
        let vad = VadResult {
            duration_s: 20.0,
            speech_duration_s: 2.0,
            has_speech: true,
            speech_segments: vec![(5.0, 6.0), (15.0, 16.0)],
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        };
        let (reduced, mapping) = solstone_core_observe_audio::reduce_audio(&audio, &vad).unwrap();
        let mut calls = 0;
        let (transcript, _) =
            super::transcribe_retained_intervals(&reduced, Some(&mapping), |samples| {
                let start = if calls == 0 { 4 } else { 14 };
                assert_eq!(samples, &audio[start * rate..(start + 3) * rate]);
                calls += 1;
                Ok((
                    crate::backend::parakeet_cpp::TranscriptionResponse {
                        words: vec![crate::backend::parakeet_cpp::TranscriptionWord {
                            word: "hello.".into(),
                            start: 1.0,
                            end: 1.5,
                            probability: None,
                        }],
                        text: "hello.".into(),
                    },
                    crate::backend::parakeet_cpp::ModelInfo {
                        model: "fixture".into(),
                        device: "cpu".into(),
                        compute_type: "float32".into(),
                    },
                ))
            })
            .unwrap();
        assert_eq!(
            calls, 2,
            "each retained interval gets one pass without retry"
        );
        assert_eq!(transcript.text, "hello. hello.");
        assert_eq!(transcript.words[0].start, 1.0);
        assert_eq!(transcript.words[1].start, 4.0);
        let statements = super::statements_from_words(&transcript.words);
        let restored = crate::transcript::restore_statement_timestamps(&statements, &mapping);
        assert_eq!(restored.len(), 2);
        for (statement, start) in restored.iter().zip([5.0, 15.0]) {
            assert_eq!(statement["start"], json!(start));
            assert_eq!(statement["end"], json!(start + 0.5));
            assert_eq!(statement["words"][0]["start"], json!(start));
            assert_eq!(statement["words"][0]["end"], json!(start + 0.5));
        }
        let mut calls = 0;
        let failed = super::transcribe_retained_intervals(&reduced, Some(&mapping), |_| {
            calls += 1;
            Err(TranscribeError::Cancelled)
        });
        assert!(matches!(failed, Err(TranscribeError::Cancelled)));
        assert_eq!(
            calls, 1,
            "cancellation stops before another interval is sent"
        );
    }

    #[test]
    fn nonintegral_utterance_end_stays_at_its_own_original_cut_boundary() {
        let vad = VadResult {
            duration_s: 12.0,
            speech_duration_s: 3.06,
            has_speech: true,
            speech_segments: vec![(0.0, 1.0), (5.0, 6.06), (10.0, 11.0)],
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        };
        let (audio, mapping) =
            solstone_core_observe_audio::reduce_audio(&vec![0.0; 12 * 16_000], &vad).unwrap();
        let (transcript, _) =
            super::transcribe_retained_intervals(&audio, Some(&mapping), |samples| {
                Ok((
                    crate::backend::parakeet_cpp::TranscriptionResponse {
                        words: vec![crate::backend::parakeet_cpp::TranscriptionWord {
                            word: "hello.".into(),
                            start: 0.1,
                            end: samples.len() as f64 / f64::from(super::SAMPLE_RATE),
                            probability: None,
                        }],
                        text: "hello.".into(),
                    },
                    crate::backend::parakeet_cpp::ModelInfo {
                        model: "fixture".into(),
                        device: "cpu".into(),
                        compute_type: "float32".into(),
                    },
                ))
            })
            .unwrap();
        let restored = crate::transcript::restore_statement_timestamps(
            &super::statements_from_words(&transcript.words),
            &mapping,
        );
        assert_eq!(restored[1]["end"], json!(7.06));
        assert_eq!(restored[1]["words"][0]["end"], json!(7.06));
    }

    #[test]
    fn automatic_retry_authority_reaches_full_and_empty_transcript_writers() {
        use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
        for (reason, attempts, empty) in [
            ("hosted_transcribe_unreachable", 1, false),
            ("hosted_transcribe_unreachable", 1, true),
            ("server_not_ready", 3, false),
            ("no_port", 1000, false),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let facts = input(temporary.path());
            let original = fs::read(&facts.path).unwrap();
            let (jsonl, npz) = transcript_paths(&facts.path);
            let header = json!({
                "raw": "clip.wav", "_solstone_processing": {
                    "schema": vocab::SCHEMA, "handler": vocab::HANDLER_TRANSCRIBE,
                    "state": "failed", "reason_code": reason,
                    "attempts": attempts, "input_size": original.len()
                }
            });
            fs::write(&jsonl, format!("{header}\n")).unwrap();
            let claims = OutputClaims::acquire([jsonl.clone()], Default::default()).unwrap();
            let observed = InputObservation::capture(&claims, &facts.path).unwrap();
            let replaces = super::automatic_retry_replaces_output(&jsonl);
            assert!(replaces);
            let _commit = observed.commit(&claims).unwrap();
            if empty {
                stt_zero_statements(&facts, replaces, None, None, None).unwrap();
            } else {
                let speaker = crate::speakers::SpeakerAnalyzeResult {
                    statements: vec![
                        json!({"id":1,"start":0.0,"end":1.0,"text":"retained recovered speech"})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ],
                    embedding_payload: None,
                    speaker_evidence: "none".into(),
                    speaker_evidence_multi_fraction: 0.0,
                    overlap_fraction: 0.0,
                    statement_labels: None,
                    turn_ranges: None,
                    overlap_ranges: None,
                };
                super::write_full_transcript(super::FullTranscriptWrite {
                    raw_path: &facts.path,
                    jsonl_path: &jsonl,
                    npz_path: &npz,
                    base_time_us_of_day: 0,
                    source: None,
                    speaker_result: &speaker,
                    backend: Some("confidential"),
                    model: Some("fixture"),
                    device: Some("remote"),
                    compute_type: None,
                    observer: None,
                    vad_result: None,
                    segment_meta: None,
                    overlap_detector: None,
                    speaker_evidence_version: "windowed-slots-v1",
                    processing: &crate::processing::analyzed_record(facts.input_size),
                    sound_tags: None,
                    sound_tag_status: None,
                    speaker_analysis_producer: None,
                    redo: replaces,
                })
                .unwrap();
            }
            assert_eq!(
                read_header(&jsonl)["_solstone_processing"]["state"],
                if empty { "empty" } else { "analyzed" }
            );
            if !empty {
                assert!(
                    fs::read_to_string(&jsonl)
                        .unwrap()
                        .contains("retained recovered speech")
                );
            }
            assert!(!super::automatic_retry_replaces_output(&jsonl));
            assert_eq!(fs::read(&facts.path).unwrap(), original);
        }
    }

    #[test]
    fn parakeet_availability_failure_after_three_attempts_remains_ordinary_work() {
        for reason in ["server_not_ready", "no_port"] {
            let temporary = tempfile::tempdir().unwrap();
            let facts = input(temporary.path());
            let original = fs::read(&facts.path).unwrap();
            let output = facts.path.with_extension("jsonl");
            let old = json!({"raw":"clip.wav","_solstone_processing":{
                "schema":vocab::SCHEMA,"handler":"transcribe","state":"failed",
                "reason_code":reason,"attempts":3,"input_size":original.len()
            }});
            fs::write(&output, format!("{old}\n")).unwrap();
            let replaces = super::automatic_retry_replaces_output(&output);
            assert!(replaces);
            assert!(!crate::args::should_skip_process_one_processed(
                &facts.path,
                false,
                None,
                None
            ));
            let error = TranscribeError::ParakeetCppDeferred {
                reason: reason.into(),
                detail: "provider is still warming".into(),
            };
            assert_eq!(
                super::backend_failure(&facts, replaces, &error, None).unwrap(),
                TerminalOutcome::Failed
            );
            let record = read_header(&output)["_solstone_processing"].clone();
            assert_eq!(record["attempts"], 4);
            assert_eq!(record["reason_code"], reason);
            assert_eq!(record["state"], "failed");
            assert!(!is_failure_exhausted(&record));
            assert_eq!(
                evaluate_terminal_proof(Some(&record), "transcribe", facts.input_size),
                TerminalProofOutcome::Refused
            );
            assert!(!crate::args::should_skip_process_one_processed(
                &facts.path,
                false,
                None,
                None
            ));
            assert_eq!(fs::read(&facts.path).unwrap(), original);
        }
    }

    #[cfg(all(feature = "full-tests", feature = "embedded", unix))]
    #[test]
    fn deferred_provider_outcomes_keep_reason_and_existing_attempt_bound() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let original = fs::read(&facts.path).unwrap();
        let error = crate::TranscribeError::ConfidentialDeferred {
            reason: "local_endpoint_unreachable".into(),
            detail: "fixture connection interrupted".into(),
        };
        for attempt in 1..=3 {
            assert_eq!(
                super::backend_failure(&facts, false, &error, None).unwrap(),
                TerminalOutcome::Failed
            );
            let record =
                read_header(&facts.path.with_extension("jsonl"))["_solstone_processing"].clone();
            assert_eq!(record["attempts"], attempt);
            assert_eq!(record["reason_code"], "local_endpoint_unreachable");
            assert_eq!(record["error"], error.to_string());
            assert_eq!(
                crate::embedded::embedded_retryable_failure(&facts.path),
                attempt < 3
            );
        }
        assert_eq!(fs::read(&facts.path).unwrap(), original);
    }

    #[cfg(all(feature = "full-tests", feature = "embedded", unix))]
    #[test]
    fn corrected_provider_terminal_stays_skipped_after_retryable_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let error = crate::TranscribeError::ConfidentialDeferred {
            reason: "local_endpoint_unreachable".into(),
            detail: "fixture connection interrupted".into(),
        };
        super::backend_failure(&facts, false, &error, None).unwrap();
        assert!(crate::embedded::embedded_retryable_failure(&facts.path));
        stt_zero_statements(&facts, true, None, None, None).unwrap();
        assert!(!crate::embedded::embedded_retryable_failure(&facts.path));
        assert!(crate::args::should_skip_process_one_processed(
            &facts.path,
            false,
            None,
            None
        ));
    }

    #[test]
    fn ac7_decode_failure_attempts_monotonic_clamped_increment() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);
        let error = AudioError::CorruptInput {
            path: facts.path.clone(),
            detail: "eio".to_owned(),
            errno: Some(5),
        };

        assert_eq!(
            decode_failure(&facts, false, &error, None).unwrap(),
            TerminalOutcome::Failed
        );
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["attempts"], 1);

        fs::write(
            &jsonl_path,
            "{\"_solstone_processing\":{\"state\":\"failed\",\"reason_code\":\"decode_transient\",\"handler\":\"transcribe\",\"attempts\":null}}\n",
        )
        .unwrap();
        assert_eq!(
            decode_failure(&facts, false, &error, None).unwrap(),
            TerminalOutcome::Failed
        );
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["attempts"], 1);

        fs::write(
            &jsonl_path,
            "{\"_solstone_processing\":{\"state\":\"failed\",\"reason_code\":\"decode_transient\",\"handler\":\"transcribe\",\"attempts\":-1}}\n",
        )
        .unwrap();
        assert_eq!(
            decode_failure(&facts, false, &error, None).unwrap(),
            TerminalOutcome::Failed
        );
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["attempts"], 1);

        fs::write(
            &jsonl_path,
            "{\"_solstone_processing\":{\"state\":\"failed\",\"reason_code\":\"decode_transient\",\"handler\":\"transcribe\",\"attempts\":1}}\n",
        )
        .unwrap();
        assert_eq!(
            decode_failure(&facts, false, &error, None).unwrap(),
            TerminalOutcome::Failed
        );
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["attempts"], 2);

        fs::write(
            &jsonl_path,
            "{\"_solstone_processing\":{\"state\":\"failed\",\"reason_code\":\"decode_transient\",\"handler\":\"transcribe\",\"attempts\":2}}\n",
        )
        .unwrap();
        assert_eq!(
            decode_failure(&facts, false, &error, None).unwrap(),
            TerminalOutcome::Failed
        );
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["attempts"], 3);
    }

    #[test]
    fn ac8_decode_failure_writes_errno_for_transient_error() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);
        let error = AudioError::CorruptInput {
            path: facts.path.clone(),
            detail: "eio".to_owned(),
            errno: Some(5),
        };

        decode_failure(&facts, false, &error, None).unwrap();
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["errno"], 5);
        assert_eq!(record["reason_code"], vocab::REASON_DECODE_TRANSIENT);
        assert!(!is_failure_exhausted(&record));
    }

    #[test]
    fn ac10_no_audio_stream_writes_reason_and_is_immediately_exhausted() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);
        let error = AudioError::NoAudioStream {
            path: facts.path.clone(),
        };

        decode_failure(&facts, false, &error, None).unwrap();
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["state"], vocab::STATE_FAILED);
        assert_eq!(record["reason_code"], vocab::REASON_NO_AUDIO_STREAM);
        assert_eq!(record["attempts"], 1);
        assert!(record.get("errno").is_none());
        assert!(is_failure_exhausted(&record));
    }

    #[test]
    fn orphan_npz_is_removed_before_terminal_write() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (_, npz_path) = transcript_paths(&facts.path);
        fs::write(&npz_path, b"orphan").unwrap();

        assert_eq!(
            vad_no_speech(&facts, false, None, None, None).unwrap(),
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
            None,
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

        vad_no_speech(&facts, false, None, None, None).unwrap();
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
        let TranscribeError::ConfidentialDeferred { reason, .. } = error else {
            panic!("an unknown backend on an active confidential lane must defer");
        };
        assert_eq!(reason, "confidential_egress_blocked");
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
    /// one owner's journal, every one comfortably inside the byte budget.
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

        // 🔒 Negative twin: genuinely oversized audio is still refused. That owner's
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
    fn unknown_backend_returns_not_implemented_before_dispatch() {
        let active_config = solstone_core_journal_config::JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(
                serde_json::json!({
                    "services": {"confidential": {"device": "abc"}},
                    "providers": {"local": {"endpoint_url": "https://endpoint", "served_model_id": "served", "credential": "secret"}},
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        };
        let empty_config = solstone_core_journal_config::JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(serde_json::Map::new()),
        };
        let store = solstone_core_spp_ratls::AttestationStateStore::new();
        let dummy_journal = Path::new("/dummy/journal");

        for config in [&active_config, &empty_config] {
            let error = super::dispatch_backend(
                config,
                dummy_journal,
                "remote",
                &[],
                &store,
                #[cfg(all(feature = "embedded", unix))]
                None,
            )
            .unwrap_err();
            match error {
                TranscribeError::BackendNotImplemented { backend } => {
                    assert_eq!(backend, "remote");
                }
                other => panic!("expected BackendNotImplemented, got {other:?}"),
            }
        }
    }

    #[test]
    fn parakeet_cpp_routing_covers_linux_macos_and_windows_in_one_host_test() {
        assert!(super::uses_parakeet_cpp_for("linux", "parakeet-cpp"));
        assert!(super::uses_parakeet_cpp_for("linux", "parakeet"));
        // `std::env::consts::OS` is "macos" on a Mac, never "darwin".
        assert!(!super::uses_parakeet_cpp_for("macos", "parakeet"));
        assert!(super::uses_parakeet_cpp_for("macos", "parakeet-cpp"));
        assert!(!super::uses_parakeet_cpp_for("linux", "confidential"));
        assert!(super::uses_parakeet_cpp_for("windows", "parakeet"));
        assert!(super::uses_parakeet_cpp_for("windows", "parakeet-cpp"));
        assert!(!super::uses_parakeet_cpp_for("windows", "confidential"));
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
        let confidential_err = TranscribeError::ConfidentialDeferred {
            reason: "attestation_failed".to_owned(),
            detail: "the confidential attestation channel is not ready".to_owned(),
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
        emit_backend_error(context(), &confidential_err, Some("confidential"));

        let events: Vec<Value> = fs::read_to_string(segment.join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 3);
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
        assert_eq!(events[2]["outcome"], "deferred");
        assert_eq!(events[2]["reason"], "attestation_failed");
        assert_eq!(events[2]["backend"], "confidential");
        assert!(events[2].get("device").is_none());
        assert!(events[2].get("model").is_none());
    }

    fn read_header(path: &Path) -> Value {
        let contents = fs::read_to_string(path).expect("terminal JSONL must be readable");
        serde_json::from_str(contents.lines().next().expect("JSONL must contain header"))
            .expect("header must be JSON")
    }

    fn setup_retry_fixture(
        root: &Path,
        segment_name: &str,
        file_name: &str,
        attempts: u64,
        reason_code: &str,
    ) -> (
        InputFacts,
        PathBuf,
        solstone_core_system::work_retry::RetryContext,
        solstone_core_system::work_retry::Unit,
        String,
    ) {
        use solstone_core_system::work_retry::{self, RetryContext, raw};
        let segment = root
            .join("chronicle")
            .join("20260101")
            .join("desktop")
            .join(segment_name);
        fs::create_dir_all(&segment).unwrap();
        let audio = segment.join(file_name);
        fs::write(&audio, b"owner-media-content").unwrap();
        let facts = capture_input_facts(&audio).unwrap();
        let (jsonl, _) = transcript_paths(&facts.path);
        let raw_name = facts.path.file_name().unwrap().to_str().unwrap();
        let header = json!({
            "raw": raw_name,
            "_solstone_processing": {
                "schema": vocab::SCHEMA,
                "state": "failed",
                "handler": "transcribe",
                "reason_code": reason_code,
                "attempts": attempts,
            }
        });
        fs::write(&jsonl, format!("{header}\n")).unwrap();
        let evidence = raw::inspect_output(root, &jsonl).unwrap().unwrap();
        raw::observe(root, &evidence).unwrap();
        work_retry::request(root, &evidence.unit).unwrap();
        let id = evidence.unit.id().unwrap();
        let attempt = work_retry::read(root, &id)
            .unwrap()
            .unwrap()
            .permission(&evidence.input, &[])
            .unwrap();
        let context = RetryContext {
            unit_id: id,
            attempt,
        };
        (facts, jsonl, context, evidence.unit, evidence.input)
    }

    #[test]
    fn retry_context_consumed_once_under_claim() {
        use solstone_core_segment::output_attempt::OutputClaims;
        use solstone_core_system::work_retry::raw;
        let root = tempfile::tempdir().unwrap();
        let (_facts, jsonl, context, _, _) =
            setup_retry_fixture(root.path(), "120000_300", "clip.wav", 3, "analysis_failed");
        let _claims = OutputClaims::acquire([jsonl.clone()], Default::default()).unwrap();
        let consumed = raw::consume_context(root.path(), &jsonl, "transcribe", &context).unwrap();
        assert_eq!(consumed, Some(context.attempt.clone()));

        let second = raw::consume_context(root.path(), &jsonl, "transcribe", &context);
        assert!(second.is_err());
    }

    #[test]
    fn retry_context_sibling_unit_unauthorized() {
        use solstone_core_segment::output_attempt::OutputClaims;
        use solstone_core_system::work_retry::{self, raw};
        let root = tempfile::tempdir().unwrap();
        let (_facts_a, _jsonl_a, context_a, _, input_a) = setup_retry_fixture(
            root.path(),
            "120000_300",
            "clip_a.wav",
            3,
            "analysis_failed",
        );
        let (_facts_b, jsonl_b, _context_b, _, _) = setup_retry_fixture(
            root.path(),
            "120000_300",
            "clip_b.wav",
            3,
            "analysis_failed",
        );

        let _claims = OutputClaims::acquire([jsonl_b.clone()], Default::default()).unwrap();
        let consumed =
            raw::consume_context(root.path(), &jsonl_b, "transcribe", &context_a).unwrap();
        assert_eq!(consumed, None);

        let record_a = work_retry::read(root.path(), &context_a.unit_id)
            .unwrap()
            .unwrap();
        assert!(record_a.permission(&input_a, &[]).is_some());
        assert_eq!(record_a.attempt, None);
    }

    #[test]
    fn retry_context_success_stamps_and_finishes() {
        use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
        use solstone_core_system::work_retry::{self, raw};
        let root = tempfile::tempdir().unwrap();
        let (facts, jsonl, context, unit, _) =
            setup_retry_fixture(root.path(), "120000_300", "clip.wav", 3, "analysis_failed");
        let claims = OutputClaims::acquire([jsonl.clone()], Default::default()).unwrap();
        let _input = InputObservation::capture(&claims, &facts.path).unwrap();
        let consumed = raw::consume_context(root.path(), &jsonl, "transcribe", &context).unwrap();
        assert_eq!(consumed, Some(context.attempt.clone()));

        let (_, npz) = transcript_paths(&facts.path);
        let result = crate::speakers::SpeakerAnalyzeResult {
            statements: vec![
                json!({"id":1,"start":0.0,"end":1.0,"text":"recovered words"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ],
            embedding_payload: None,
            speaker_evidence: "none".into(),
            speaker_evidence_multi_fraction: 0.0,
            overlap_fraction: 0.0,
            statement_labels: None,
            turn_ranges: None,
            overlap_ranges: None,
        };
        let mut processing = crate::processing::analyzed_record(facts.input_size);
        raw::stamp_attempt_from(&mut processing, &context.attempt).unwrap();
        super::write_full_transcript(super::FullTranscriptWrite {
            raw_path: &facts.path,
            jsonl_path: &jsonl,
            npz_path: &npz,
            base_time_us_of_day: 0,
            source: None,
            speaker_result: &result,
            backend: Some("parakeet-cpp"),
            model: Some("parakeet"),
            device: Some("cpu"),
            compute_type: None,
            observer: None,
            vad_result: None,
            segment_meta: None,
            overlap_detector: None,
            speaker_evidence_version: "windowed-slots-v1",
            processing: &processing,
            sound_tags: None,
            sound_tag_status: None,
            speaker_analysis_producer: None,
            redo: true,
        })
        .unwrap();

        raw::finish_output(root.path(), &unit, &context.attempt).unwrap();
        assert!(
            !work_retry::read(root.path(), &context.unit_id)
                .unwrap()
                .unwrap()
                .held
        );
        let header = read_header(&jsonl);
        assert_eq!(header["_solstone_processing"]["state"], "analyzed");
        assert_eq!(
            header["_solstone_processing"]["retry_attempt"]["cause"],
            context.attempt.cause
        );
    }

    #[test]
    fn retry_context_terminal_failure_stamps_and_finishes() {
        use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
        use solstone_core_system::work_retry::raw;
        let root = tempfile::tempdir().unwrap();
        let (facts, jsonl, context, unit, _) = setup_retry_fixture(
            root.path(),
            "120000_300",
            "clip.wav",
            3,
            vocab::REASON_DECODE_TRANSIENT,
        );
        let claims = OutputClaims::acquire([jsonl.clone()], Default::default()).unwrap();
        let _input = InputObservation::capture(&claims, &facts.path).unwrap();
        let consumed = raw::consume_context(root.path(), &jsonl, "transcribe", &context).unwrap();
        assert_eq!(consumed, Some(context.attempt.clone()));

        let error = AudioError::CorruptInput {
            path: facts.path.clone(),
            detail: "eio".into(),
            errno: Some(5),
        };
        assert!(error.is_transient_decode());
        let terminal = decode_failure(&facts, true, &error, Some(&context.attempt)).unwrap();
        assert_eq!(terminal, TerminalOutcome::Failed);

        raw::finish_output(root.path(), &unit, &context.attempt).unwrap();
        let header = read_header(&jsonl);
        assert_eq!(header["_solstone_processing"]["state"], "failed");
        assert_eq!(
            header["_solstone_processing"]["reason_code"],
            vocab::REASON_DECODE_TRANSIENT
        );
        assert_eq!(
            header["_solstone_processing"]["retry_attempt"]["cause"],
            context.attempt.cause
        );
    }

    #[cfg(all(feature = "embedded", unix))]
    #[test]
    fn retry_context_budget_refusal_publishes_and_settles_its_attempt() {
        use solstone_core_journal_config::JournalConfigRead;
        use solstone_core_segment::output_attempt::OutputClaims;
        use solstone_core_spp_ratls::AttestationStateStore;
        use solstone_core_system::work_retry::{self, RetryContext, raw};

        let root = tempfile::tempdir().unwrap();
        let segment = root
            .path()
            .join("chronicle")
            .join("20260101")
            .join("desktop")
            .join("120000_300");
        fs::create_dir_all(&segment).unwrap();
        let audio = segment.join("sparse.wav");
        let file = std::fs::File::create(&audio).unwrap();
        file.set_len(17 * 1024 * 1024).unwrap();
        let (jsonl, _) = transcript_paths(&audio);
        let raw_name = audio.file_name().unwrap().to_str().unwrap();
        let header = json!({
            "raw": raw_name,
            "_solstone_processing": {
                "schema": vocab::SCHEMA,
                "state": "failed",
                "handler": "transcribe",
                "reason_code": "analysis_failed",
                "attempts": 3,
            }
        });
        fs::write(&jsonl, format!("{header}\n")).unwrap();
        let evidence = raw::inspect_output(root.path(), &jsonl).unwrap().unwrap();
        raw::observe(root.path(), &evidence).unwrap();
        work_retry::request(root.path(), &evidence.unit).unwrap();
        let id = evidence.unit.id().unwrap();
        let attempt = work_retry::read(root.path(), &id)
            .unwrap()
            .unwrap()
            .permission(&evidence.input, &[])
            .unwrap();
        let context = RetryContext {
            unit_id: id,
            attempt,
        };

        struct StubAsr;
        impl crate::embedded::EmbeddedAsr for StubAsr {
            fn transcribe(&self, _samples: &[f32]) -> Result<Value, String> {
                panic!("stub asr must not run");
            }
            fn model_info(&self) -> crate::embedded::EmbeddedModelInfo {
                crate::embedded::EmbeddedModelInfo {
                    backend: "embedded".into(),
                    model: "stub".into(),
                    device: "cpu".into(),
                    compute_type: "float32".into(),
                }
            }
        }
        let stub_asr = StubAsr;
        let cancel_fn = || false;
        let embedded = crate::embedded::EmbeddedContext {
            owner_original_deleted: &|| {},
            models: root.path(),
            scratch: root.path(),
            asr: &stub_asr,
            cancel: &cancel_fn,
            service_io: &|_| {},
        };
        let config = JournalConfigRead {
            present: false,
            sha256: None,
            config: None,
        };
        let attestation_state = AttestationStateStore::new();

        let err = super::process_one_with(
            &audio,
            root.path(),
            false,
            None,
            &config,
            &attestation_state,
            Some(&embedded),
            Some(&context),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            TranscribeError::ParakeetCppDeferred { ref reason, .. } if reason == "embedded-input-budget"
        ));

        let record = work_retry::read(root.path(), &context.unit_id)
            .unwrap()
            .unwrap();
        assert!(record.consumed.contains(&context.attempt.cause));
        assert_eq!(record.attempt, None);
        assert_eq!(record.reason, "embedded-input-budget");
        assert!(record.held);

        let header = read_header(&jsonl);
        assert_eq!(
            header["_solstone_processing"]["retry_attempt"]["cause"],
            context.attempt.cause
        );

        assert!(raw::finish_output(root.path(), &evidence.unit, &context.attempt).is_err());

        // A second pass does not consume again:
        let second = super::process_one_with(
            &audio,
            root.path(),
            false,
            None,
            &config,
            &attestation_state,
            Some(&embedded),
            Some(&context),
        );
        assert_eq!(second.unwrap(), super::ProcessOutcome::Skipped);

        let claims = OutputClaims::acquire([jsonl.clone()], Default::default()).unwrap();
        assert!(raw::consume_context(root.path(), &jsonl, "transcribe", &context).is_err());
        drop(claims);
    }
}
