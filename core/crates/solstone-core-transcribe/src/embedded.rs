// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Explicit in-process transcription for dedicated mobile runtimes.

use std::path::Path;

use serde_json::Value;

use crate::{TranscribeError, TranscribeFile, backend::parakeet_cpp};

/// Native-call cap, in mono 16 kHz samples, for one recognizer invocation.
pub const EMBEDDED_AUDIO_MAX_SAMPLES: usize = 120 * 16_000;

/// The caller's cancellation lifetime and content-free service-I/O observation.
pub struct EmbeddedControl<'a> {
    pub cancelled: &'a dyn Fn() -> bool,
    pub service_io: &'a dyn Fn(crate::ConfidentialIoPhase),
    /// Retention deleted the verified owner-voice original after publication,
    /// while the transcript's output claim and segment commit were still held.
    pub owner_original_deleted: &'a dyn Fn(),
}

/// Provenance of the model actually used by the embedded caller.
pub struct EmbeddedModelInfo {
    /// Registered backend identifier.
    pub backend: String,
    /// Model identity.
    pub model: String,
    /// Compute device.
    pub device: String,
    /// Compute precision.
    pub compute_type: String,
}

/// A callable local recognizer. Input is mono 16 kHz PCM; output uses the normal
/// verbose JSON word contract (`word`, `start`, `end`, optional `conf`).
pub trait EmbeddedAsr {
    /// Transcribe one bounded set of samples, including word timings.
    fn transcribe(&self, samples: &[f32]) -> Result<Value, String>;
    /// Describe the loaded model and compute device.
    fn model_info(&self) -> EmbeddedModelInfo;
}

pub(crate) struct EmbeddedContext<'a> {
    pub(crate) models: &'a Path,
    pub(crate) scratch: &'a Path,
    pub(crate) asr: &'a dyn EmbeddedAsr,
    pub(crate) cancel: &'a dyn Fn() -> bool,
    pub(crate) service_io: &'a dyn Fn(crate::ConfidentialIoPhase),
    pub(crate) owner_original_deleted: &'a dyn Fn(),
}

pub(crate) fn select_backend(
    resolved: Result<String, TranscribeError>,
    embedded: Option<&EmbeddedContext<'_>>,
    requested: Option<&str>,
    config: &solstone_core_journal_config::JournalConfigRead,
) -> Result<String, TranscribeError> {
    if !matches!(&resolved, Ok(_) | Err(TranscribeError::SttSurface { .. })) {
        return resolved;
    }
    // Lost credentials or endpoint fields must not turn a confidential choice
    // into local inference. Keep that choice and let its normal admission refuse.
    if embedded.is_some()
        && crate::config::confidential_audio_enabled(config)
        && !matches!(requested, Some("parakeet" | "parakeet-cpp"))
        && (requested == Some("confidential")
            || crate::backend::confidential::confidential_provenance(config).is_some())
    {
        return Ok("confidential".into());
    }
    match (resolved, embedded) {
        (Ok(backend), _) if backend == "confidential" => Ok(backend),
        (Ok(_), Some(runtime)) | (Err(TranscribeError::SttSurface { .. }), Some(runtime)) => {
            Ok(runtime.asr.model_info().backend)
        }
        (resolved, _) => resolved,
    }
}

impl EmbeddedContext<'_> {
    pub(crate) fn checkpoint(&self) -> Result<(), TranscribeError> {
        if (self.cancel)() {
            Err(TranscribeError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub(crate) fn transcribe(
        &self,
        samples: &[f32],
    ) -> Result<(parakeet_cpp::TranscriptionResponse, parakeet_cpp::ModelInfo), TranscribeError>
    {
        self.checkpoint()?;
        let payload =
            self.asr
                .transcribe(samples)
                .map_err(|detail| TranscribeError::ParakeetCppFailure {
                    reason: "embedded-inference".to_owned(),
                    detail,
                })?;
        self.checkpoint()?;
        let transcript =
            parakeet_cpp::parse_verbose_json(&payload.to_string()).map_err(|error| {
                TranscribeError::ParakeetCppFailure {
                    reason: "embedded-word-contract".to_owned(),
                    detail: format!("{error:?}"),
                }
            })?;
        let info = self.asr.model_info();
        Ok((
            transcript,
            parakeet_cpp::ModelInfo {
                model: info.model,
                device: info.device,
                compute_type: info.compute_type,
            },
        ))
    }
}

pub use solstone_core_system::work_retry::raw::RetryContext;

pub fn embedded_retry_context(
    journal: &Path,
    audio: &Path,
) -> Result<Option<RetryContext>, String> {
    solstone_core_system::work_retry::raw::context_for_output(
        journal,
        &audio.with_extension("jsonl"),
        solstone_core_processing_record::vocab::HANDLER_TRANSCRIBE,
    )
}

pub fn embedded_already_processed(
    journal: &Path,
    audio: &Path,
    retry: Option<&RetryContext>,
) -> bool {
    crate::args::should_skip_process_one_processed(audio, false, Some(journal), retry)
}

pub fn embedded_retryable_failure(audio: &Path) -> bool {
    use solstone_core_processing_record::{
        is_failure_exhausted, read_processing_record_header, vocab,
    };
    read_processing_record_header(&audio.with_extension("jsonl")).is_some_and(|record| {
        record["schema"] == vocab::SCHEMA
            && record["handler"] == vocab::HANDLER_TRANSCRIBE
            && record["state"] == vocab::STATE_FAILED
            && !is_failure_exhausted(&record)
    })
}

/// Run the normal claimed decode/VAD/ASR/speaker/publication pipeline in process.
///
/// The caller opts in explicitly and supplies app-private pinned model and scratch
/// directories. Cancellation is cooperative between stages and immediately before
/// publication. Inference already executing is allowed to return. Errors retain
/// the raw input and do not publish a successful transcript.
pub fn process_embedded(
    audio: &Path,
    journal: &Path,
    models: &Path,
    scratch: &Path,
    asr: &dyn EmbeddedAsr,
    control: EmbeddedControl<'_>,
    retry: Option<&RetryContext>,
) -> Result<TranscribeFile, TranscribeError> {
    let context = EmbeddedContext {
        models,
        scratch,
        asr,
        cancel: control.cancelled,
        service_io: control.service_io,
        owner_original_deleted: control.owner_original_deleted,
    };
    context.checkpoint()?;
    std::fs::create_dir_all(scratch).map_err(|error| TranscribeError::VadTemporary {
        detail: format!("could not create embedded scratch: {error}"),
    })?;
    let config = crate::config::read_transcribe_config(journal).map_err(|error| {
        TranscribeError::ProcessingClaim {
            path: audio.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    let backend = crate::requested_backend(None, &config).map_err(|detail| {
        TranscribeError::ProcessingClaim {
            path: audio.to_path_buf(),
            detail,
        }
    })?;
    let outcome = crate::stage::process_one_with(
        audio,
        journal,
        false,
        backend,
        &config,
        &solstone_core_spp_ratls::AttestationStateStore::new(),
        Some(&context),
        retry,
    )?;
    Ok(crate::record_transcribe_file(outcome, audio))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct LocalRecognizer;
    impl EmbeddedAsr for LocalRecognizer {
        fn transcribe(&self, _: &[f32]) -> Result<Value, String> {
            panic!("backend choice must not run inference")
        }
        fn model_info(&self) -> EmbeddedModelInfo {
            EmbeddedModelInfo {
                backend: "parakeet-cpp".into(),
                model: "pinned-local".into(),
                device: "cpu".into(),
                compute_type: "q8_0".into(),
            }
        }
    }

    #[test]
    fn incomplete_confidential_configuration_never_falls_back_to_local_inference() {
        use crate::backend::{confidential, resolve_default_backend};
        for (field, replacement) in [
            ("credential", None),
            ("credential", Some(Value::Null)),
            ("credential", Some(Value::String(String::new()))),
            ("endpoint_url", None),
            ("served_model_id", None),
        ] {
            for (backend, audio_enabled, expected) in [
                (None, true, "confidential"),
                (Some("confidential"), true, "confidential"),
                (Some("parakeet"), true, "parakeet-cpp"),
                (Some("parakeet-cpp"), true, "parakeet-cpp"),
                (None, false, "parakeet-cpp"),
                (Some("confidential"), false, "parakeet-cpp"),
            ] {
                let mut value = serde_json::json!({
                    "transcribe":{"confidential_audio":audio_enabled},
                    "services":{"confidential":{
                        "device":"abc", "endpoint_url":"https://endpoint", "served_model_id":"served",
                        "credential_fingerprint_sha256":"cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                    }},
                    "providers":{
                        "active":{"provider":"local","model":"served"},
                        "local":{"endpoint_url":"https://endpoint","served_model_id":"served","credential":"endpoint-credential"}
                    }
                });
                if let Some(backend) = backend {
                    value["transcribe"]["backend"] = backend.into();
                }
                let local = value["providers"]["local"].as_object_mut().unwrap();
                match replacement.clone() {
                    Some(replacement) => {
                        local.insert(field.into(), replacement);
                    }
                    None => {
                        local.remove(field);
                    }
                }
                let config = solstone_core_journal_config::JournalConfigRead {
                    present: true,
                    sha256: None,
                    config: Some(value.as_object().unwrap().clone()),
                };
                let requested = crate::requested_backend(None, &config).unwrap();
                let resolve = || {
                    resolve_default_backend(
                        requested,
                        None,
                        None,
                        None,
                        confidential::confidential_channel_plausible(&config),
                        crate::config::confidential_audio_enabled(&config),
                    )
                    .map(|resolution| resolution.backend)
                };
                let context = EmbeddedContext {
                    owner_original_deleted: &|| {},
                    models: Path::new("unused"),
                    scratch: Path::new("unused"),
                    asr: &LocalRecognizer,
                    cancel: &|| false,
                    service_io: &|_| {},
                };
                assert_eq!(
                    select_backend(resolve(), Some(&context), requested, &config).unwrap(),
                    expected,
                    "field={field}, backend={backend:?}, audio={audio_enabled}"
                );
                // The desktop resolver's result (including refusal) remains untouched.
                let original = resolve();
                let desktop = select_backend(resolve(), None, requested, &config);
                assert_eq!(format!("{desktop:?}"), format!("{original:?}"));
            }
        }
    }

    #[test]
    fn explicit_confidential_without_provenance_stays_confidential_for_normal_refusal() {
        let value = serde_json::json!({"transcribe":{"backend":"confidential"}});
        let config = solstone_core_journal_config::JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(value.as_object().unwrap().clone()),
        };
        let context = EmbeddedContext {
            owner_original_deleted: &|| {},
            models: Path::new("unused"),
            scratch: Path::new("unused"),
            asr: &LocalRecognizer,
            cancel: &|| false,
            service_io: &|_| {},
        };
        let selected = select_backend(
            Err(TranscribeError::SttSurface {
                available_bytes: None,
                floor_bytes: None,
            }),
            Some(&context),
            Some("confidential"),
            &config,
        )
        .unwrap();
        assert_eq!(selected, "confidential");
        assert!(
            crate::backend::confidential::refuse_confidential_egress(&config, &selected, true)
                .is_err()
        );
    }

    #[test]
    fn configured_local_choices_and_audio_opt_out_keep_the_embedded_local_lane() {
        use crate::backend::{confidential, resolve_default_backend};
        use solstone_core_journal_config::JournalConfigRead;
        for (backend, audio_enabled, expected) in [
            (Some("parakeet"), true, "parakeet-cpp"),
            (Some("parakeet-cpp"), true, "parakeet-cpp"),
            (None, false, "parakeet-cpp"),
            (None, true, "confidential"),
            (Some("confidential"), false, "parakeet-cpp"),
        ] {
            let mut value = serde_json::json!({
                "transcribe":{"confidential_audio":audio_enabled},
                "services":{"confidential":{
                    "device":"abc", "endpoint_url":"https://endpoint",
                    "served_model_id":"served",
                    "credential_fingerprint_sha256":"cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }},
                "providers":{
                    "active":{"provider":"local","model":"served"},
                    "local":{"endpoint_url":"https://endpoint","served_model_id":"served","credential":"endpoint-credential"}
                }
            });
            if let Some(backend) = backend {
                value["transcribe"]["backend"] = backend.into();
            }
            let config = JournalConfigRead {
                present: true,
                sha256: None,
                config: Some(value.as_object().unwrap().clone()),
            };
            assert!(confidential::confidential_channel_plausible(&config));
            let resolved = resolve_default_backend(
                crate::requested_backend(None, &config).unwrap(),
                None,
                None,
                None,
                confidential::confidential_channel_plausible(&config),
                crate::config::confidential_audio_enabled(&config),
            )
            .map(|resolution| resolution.backend);
            let context = EmbeddedContext {
                owner_original_deleted: &|| {},
                models: Path::new("unused"),
                scratch: Path::new("unused"),
                asr: &LocalRecognizer,
                cancel: &|| false,
                service_io: &|_| {},
            };
            assert_eq!(
                select_backend(
                    resolved,
                    Some(&context),
                    crate::requested_backend(None, &config).unwrap(),
                    &config
                )
                .unwrap(),
                expected
            );
            assert!(matches!(
                select_backend(
                    Err(TranscribeError::Cancelled),
                    Some(&context),
                    crate::requested_backend(None, &config).unwrap(),
                    &config
                ),
                Err(TranscribeError::Cancelled)
            ));
        }
    }

    #[test]
    fn invalid_configured_backend_refuses_before_processing_or_inference() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            br#"{"transcribe":{"backend":"invalid"}}"#,
        )
        .unwrap();
        let result = process_embedded(
            Path::new("nonexistent-audio.wav"),
            journal.path(),
            Path::new("unused-models"),
            &journal.path().join("scratch"),
            &LocalRecognizer,
            EmbeddedControl {
                owner_original_deleted: &|| {},
                cancelled: &|| false,
                service_io: &|_| {},
            },
            None,
        );
        assert!(
            matches!(result, Err(TranscribeError::ProcessingClaim { detail, .. })
            if detail.contains("invalid transcribe.backend"))
        );
    }

    #[test]
    fn confidential_selection_never_inspects_the_local_recognizer() {
        struct LocalMustNotRun;
        impl EmbeddedAsr for LocalMustNotRun {
            fn transcribe(&self, _: &[f32]) -> Result<Value, String> {
                panic!("local ASR selected")
            }
            fn model_info(&self) -> EmbeddedModelInfo {
                panic!("local model inspected")
            }
        }
        let context = EmbeddedContext {
            owner_original_deleted: &|| {},
            models: Path::new("unused"),
            scratch: Path::new("unused"),
            asr: &LocalMustNotRun,
            cancel: &|| false,
            service_io: &|_| {},
        };
        let config = solstone_core_journal_config::JournalConfigRead {
            present: false,
            sha256: None,
            config: None,
        };
        assert_eq!(
            select_backend(Ok("confidential".into()), Some(&context), None, &config).unwrap(),
            "confidential"
        );
        assert_eq!(
            select_backend(Ok("confidential".into()), None, None, &config).unwrap(),
            "confidential"
        );
    }

    struct CancelDuringAsr(AtomicBool);
    impl EmbeddedAsr for CancelDuringAsr {
        fn transcribe(&self, _: &[f32]) -> Result<Value, String> {
            self.0.store(true, Ordering::Release);
            Ok(
                serde_json::json!({"text":"hello", "words":[{"word":"hello", "start":0.0, "end":0.5}]}),
            )
        }
        fn model_info(&self) -> EmbeddedModelInfo {
            panic!("cancelled inference must not be accepted")
        }
    }

    #[test]
    fn cancellation_during_native_inference_discards_its_result() {
        let asr = CancelDuringAsr(AtomicBool::new(false));
        let cancel = || asr.0.load(Ordering::Acquire);
        let context = EmbeddedContext {
            owner_original_deleted: &|| {},
            models: Path::new("unused"),
            scratch: Path::new("unused"),
            asr: &asr,
            cancel: &cancel,
            service_io: &|_| {},
        };
        assert!(matches!(
            context.transcribe(&[0.0; 16000]),
            Err(TranscribeError::Cancelled)
        ));
        assert!(matches!(
            process_embedded(
                Path::new("unused"),
                Path::new("unused"),
                Path::new("unused"),
                Path::new("unused"),
                &asr,
                EmbeddedControl {
                    owner_original_deleted: &|| {},
                    cancelled: &cancel,
                    service_io: &|_| {}
                },
                None,
            ),
            Err(TranscribeError::Cancelled)
        ));
    }

    #[test]
    fn unit_past_one_native_call_reaches_vad_without_a_budget_hold() {
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let segment = root.path().join("chronicle/20261012/audio/120000_121");
        std::fs::create_dir_all(&segment).unwrap();
        let audio = segment.join("audio.wav");
        let bytes = solstone_core_observe_audio::audio_to_wav_bytes(
            &vec![0.0; EMBEDDED_AUDIO_MAX_SAMPLES + 1],
            16000,
        )
        .unwrap();
        std::fs::write(&audio, &bytes).unwrap();
        let deleted = std::cell::Cell::new(false);
        let result = process_embedded(
            &audio,
            root.path(),
            Path::new("missing-models"),
            &root.path().join("scratch"),
            &LocalRecognizer,
            EmbeddedControl {
                owner_original_deleted: &|| deleted.set(true),
                cancelled: &|| false,
                service_io: &|_| {},
            },
            None,
        );
        // The first unit reaches VAD. A missing model returns before later pulls
        // and before the recognizer. There is no recording ceiling.
        let error = result.expect_err("missing VAD model");
        assert!(matches!(error, TranscribeError::ModelAsset(_)), "{error:?}");
        assert!(!error.to_string().contains("embedded-duration-budget"));
        assert!(!error.to_string().contains("embedded-input-budget"));
        assert_eq!(std::fs::read(&audio).unwrap(), bytes);
        assert!(!audio.with_extension("jsonl").exists());
        assert!(!audio.with_extension("npz").exists());
        assert!(!deleted.get());
    }

    #[test]
    fn short_and_cap_length_units_reach_vad_without_loading_a_model() {
        for samples in [30 * 16_000, 44 * 16_000, EMBEDDED_AUDIO_MAX_SAMPLES] {
            let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
            let segment = root.path().join("chronicle/20261012/audio/120000_120");
            std::fs::create_dir_all(&segment).unwrap();
            let audio = segment.join("audio.wav");
            let bytes = solstone_core_observe_audio::audio_to_wav_bytes(&vec![0.0; samples], 16000)
                .unwrap();
            std::fs::write(&audio, &bytes).unwrap();
            let result = process_embedded(
                &audio,
                root.path(),
                Path::new("missing-models"),
                &root.path().join("scratch"),
                &LocalRecognizer,
                EmbeddedControl {
                    cancelled: &|| false,
                    service_io: &|_| {},
                    owner_original_deleted: &|| {},
                },
                None,
            );
            // A missing VAD model is the first boundary on the first unit.
            // The recognizer above panics if that unit runs it.
            assert!(
                matches!(result, Err(TranscribeError::ModelAsset(_))),
                "{samples}: {result:?}"
            );
            assert_eq!(std::fs::read(&audio).unwrap(), bytes);
            assert!(!audio.with_extension("jsonl").exists());
        }
    }
}
