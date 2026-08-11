// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decoded-audio VAD, reduction, and sound-tagging boundaries.

use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use solstone_core_observe_audio::{AudioReduction, VadResult, reduce_audio, write_f32le_exclusive};

use crate::{TranscribeError, resolve_model_asset};

const VAD_BINARY_NAME: &str = "solstone-core-vad-analyze";
const VAD_BINARY_ENV: &str = "SOLSTONE_VAD_BINARY";
const REQUEST_SCHEMA: &str = "solstone-vad-request-v1";
const RESPONSE_SCHEMA: &str = "solstone-vad-response-v1";
const ERROR_SCHEMA: &str = "solstone-vad-error-v1";
const NOISY_RMS_THRESHOLD: f64 = 0.01;
const REDUCTION_SPEECH_RATIO_THRESHOLD: f64 = 0.7;
const SAMPLE_RATE_HZ: f64 = 16_000.0;

/// Run the sibling VAD helper over decoded mono samples.
pub(crate) fn run_vad(
    audio: &[f32],
    min_speech_seconds: f64,
) -> Result<VadResult, TranscribeError> {
    let model_path = resolve_model_asset("silero_vad_v6.onnx")?;
    let binary = resolve_vad_binary()?;
    let temporary = tempfile::Builder::new()
        .prefix("solstone-transcribe-vad-")
        .tempdir()
        .map_err(|error| TranscribeError::VadTemporary {
            detail: format!("could not create VAD input directory: {error}"),
        })?;
    let audio_path = temporary.path().join("audio.f32le");

    let write_result =
        write_f32le_exclusive(&audio_path, audio).map_err(|error| TranscribeError::VadTemporary {
            detail: format!("could not write VAD input: {error}"),
        });
    if let Err(error) = write_result {
        let _ = temporary.close();
        return Err(error);
    }

    let request = vad_request(&audio_path, &model_path, min_speech_seconds)?;
    let result = invoke_vad_helper(&binary, &request);
    let cleanup_result = temporary
        .close()
        .map_err(|error| TranscribeError::VadTemporary {
            detail: format!("could not remove VAD input directory: {error}"),
        });

    match result {
        Err(error) => Err(error),
        Ok(vad) => {
            cleanup_result?;
            Ok(vad)
        }
    }
}

/// Return the local VAD input's speech fraction, avoiding a zero-duration NaN.
pub(crate) fn speech_ratio(vad: &VadResult) -> f64 {
    if vad.duration_s == 0.0 {
        0.0
    } else {
        vad.speech_duration_s / vad.duration_s
    }
}

/// True when noisy, speech-dense audio should bypass silence reduction.
pub(crate) fn should_skip_reduction(vad: &VadResult, ratio: f64) -> bool {
    vad.is_noisy(NOISY_RMS_THRESHOLD) && ratio >= REDUCTION_SPEECH_RATIO_THRESHOLD
}

/// Reduce silence unless the VAD result is noisy and speech-dense.
pub(crate) fn reduce_audio_if_needed(
    audio: &[f32],
    vad: &VadResult,
    ratio: f64,
) -> Option<(Vec<f32>, AudioReduction)> {
    (!should_skip_reduction(vad, ratio))
        .then(|| reduce_audio(audio, vad))
        .flatten()
}

/// Sound tagging is intentionally best-effort.
pub(crate) fn tag_audio(audio: &[f32], journal_path: &Path) -> Option<Value> {
    solstone_core_sound_tags::tag_audio(audio, journal_path)
}

fn resolve_vad_binary() -> Result<PathBuf, TranscribeError> {
    let candidate = vad_binary_candidate_from(env::current_exe(), |name| env::var(name).ok())?;
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(TranscribeError::VadBinary {
            detail: format!("VAD helper binary is missing: {}", candidate.display()),
        })
    }
}

fn vad_binary_candidate_from<F>(
    current_executable: Result<PathBuf, io::Error>,
    lookup_env: F,
) -> Result<PathBuf, TranscribeError>
where
    F: Fn(&str) -> Option<String>,
{
    let executable = current_executable.map_err(|error| TranscribeError::VadBinary {
        detail: format!("could not determine current executable for VAD helper: {error}"),
    })?;
    let directory = executable
        .parent()
        .ok_or_else(|| TranscribeError::VadBinary {
            detail: "current executable has no parent directory for VAD helper lookup".to_owned(),
        })?;
    Ok(vad_binary_candidate(directory, lookup_env))
}

fn vad_binary_candidate<F>(base_directory: &Path, lookup_env: F) -> PathBuf
where
    F: Fn(&str) -> Option<String>,
{
    match lookup_env(VAD_BINARY_ENV) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => base_directory.join(VAD_BINARY_NAME),
    }
}

fn vad_request(
    audio_path: &Path,
    model_path: &Path,
    min_speech_seconds: f64,
) -> Result<Vec<u8>, TranscribeError> {
    serde_json::to_vec(&json!({
        "schema": REQUEST_SCHEMA,
        "audio_f32le_path": audio_path,
        "models": { "silero_vad_onnx_path": model_path },
        "min_speech_seconds": min_speech_seconds,
    }))
    .map_err(|error| TranscribeError::VadResponse {
        detail: format!("could not serialize VAD request: {error}"),
    })
}

fn invoke_vad_helper(binary: &Path, request: &[u8]) -> Result<VadResult, TranscribeError> {
    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| TranscribeError::VadTemporary {
            detail: format!("could not launch VAD helper {}: {error}", binary.display()),
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| TranscribeError::VadTemporary {
            detail: "VAD helper stdin was unavailable".to_owned(),
        })?
        .write_all(request)
        .map_err(|error| TranscribeError::VadTemporary {
            detail: format!("could not send VAD request: {error}"),
        })?;
    let output = child
        .wait_with_output()
        .map_err(|error| TranscribeError::VadTemporary {
            detail: format!("could not wait for VAD helper: {error}"),
        })?;

    if output.status.success() {
        parse_vad_response(json_line(&output.stdout, "VAD response")?)
    } else if let Some(exit_code) = output.status.code() {
        Err(parse_vad_error(
            exit_code,
            json_line(&output.stderr, "VAD error response")?,
        ))
    } else {
        Err(TranscribeError::VadTemporary {
            detail: "VAD helper terminated without an exit code".to_owned(),
        })
    }
}

fn json_line<'a>(bytes: &'a [u8], context: &str) -> Result<&'a str, TranscribeError> {
    let text = std::str::from_utf8(bytes).map_err(|error| TranscribeError::VadResponse {
        detail: format!("{context} was not UTF-8: {error}"),
    })?;
    let mut lines = text.lines();
    let line = lines
        .next()
        .filter(|line| !line.is_empty())
        .ok_or_else(|| TranscribeError::VadResponse {
            detail: format!("{context} was empty"),
        })?;
    if lines.next().is_some() {
        return Err(TranscribeError::VadResponse {
            detail: format!("{context} contained more than one line"),
        });
    }
    Ok(line)
}

fn parse_vad_response(line: &str) -> Result<VadResult, TranscribeError> {
    let value: Value =
        serde_json::from_str(line).map_err(|error| TranscribeError::VadResponse {
            detail: format!("VAD response was not JSON: {error}"),
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| TranscribeError::VadResponse {
            detail: "VAD response must be an object".to_owned(),
        })?;
    if required_string(object, "schema")? != RESPONSE_SCHEMA {
        return Err(TranscribeError::VadResponse {
            detail: "VAD response has an unknown schema".to_owned(),
        });
    }
    let duration_s = required_f64(object, "duration")?;
    let speech_duration_s = required_f64(object, "speech_duration")?;
    let has_speech = object
        .get("has_speech")
        .and_then(Value::as_bool)
        .ok_or_else(|| TranscribeError::VadResponse {
            detail: "VAD response has no boolean has_speech".to_owned(),
        })?;
    let speech = object
        .get("speech")
        .and_then(Value::as_array)
        .ok_or_else(|| TranscribeError::VadResponse {
            detail: "VAD response has no speech array".to_owned(),
        })?;
    let speech_segments = speech
        .iter()
        .map(parse_speech_segment)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(VadResult {
        duration_s,
        speech_duration_s,
        has_speech,
        speech_segments,
        noisy_rms: None,
        noisy_s: 0.0,
        loud_windows: 0,
        speech_loud_windows: 0,
    })
}

fn parse_speech_segment(value: &Value) -> Result<(f64, f64), TranscribeError> {
    let object = value
        .as_object()
        .ok_or_else(|| TranscribeError::VadResponse {
            detail: "VAD speech entry must be an object".to_owned(),
        })?;
    let start = required_sample_index(object, "start")?;
    let end = required_sample_index(object, "end")?;
    if end < start {
        return Err(TranscribeError::VadResponse {
            detail: "VAD speech entry ends before it starts".to_owned(),
        });
    }
    Ok((start as f64 / SAMPLE_RATE_HZ, end as f64 / SAMPLE_RATE_HZ))
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<&'a str, TranscribeError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| TranscribeError::VadResponse {
            detail: format!("VAD response has no non-empty {field}"),
        })
}

fn required_f64(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<f64, TranscribeError> {
    object
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| TranscribeError::VadResponse {
            detail: format!("VAD response has no non-negative finite {field}"),
        })
}

fn required_sample_index(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<usize, TranscribeError> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| TranscribeError::VadResponse {
            detail: format!("VAD speech entry has no valid {field} sample index"),
        })
}

fn parse_vad_error(exit_code: i32, line: &str) -> TranscribeError {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return TranscribeError::VadResponse {
                detail: format!("VAD error response was not JSON: {error}"),
            };
        }
    };
    let Some(object) = value.as_object() else {
        return TranscribeError::VadResponse {
            detail: "VAD error response must be an object".to_owned(),
        };
    };
    let (Some(schema), Some(reason), Some(detail)) = (
        object.get("schema").and_then(Value::as_str),
        object.get("reason").and_then(Value::as_str),
        object.get("detail").and_then(Value::as_str),
    ) else {
        return TranscribeError::VadResponse {
            detail: "VAD error response lacks schema, reason, or detail".to_owned(),
        };
    };
    if schema != ERROR_SCHEMA || reason.is_empty() || detail.is_empty() {
        return TranscribeError::VadResponse {
            detail: "VAD error response violates its schema".to_owned(),
        };
    }
    TranscribeError::VadHelper {
        helper_exit_code: exit_code,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use serde_json::json;
    use solstone_core_observe_audio::VadResult;

    use super::{
        ERROR_SCHEMA, RESPONSE_SCHEMA, VAD_BINARY_ENV, VAD_BINARY_NAME, parse_vad_error,
        parse_vad_response, should_skip_reduction, speech_ratio, vad_binary_candidate_from,
    };
    use crate::TranscribeError;

    fn vad(noisy: bool) -> VadResult {
        VadResult {
            duration_s: 1.0,
            speech_duration_s: 0.5,
            has_speech: true,
            speech_segments: vec![(0.0, 0.5)],
            noisy_rms: noisy.then_some(0.02),
            noisy_s: 0.5,
            loud_windows: 0,
            speech_loud_windows: 0,
        }
    }

    #[test]
    fn reduction_skips_for_noisy_audio_at_speech_ratio_threshold() {
        assert!(should_skip_reduction(&vad(true), 0.7));
    }

    #[test]
    fn reduction_does_not_skip_for_noisy_audio_below_speech_ratio_threshold() {
        assert!(!should_skip_reduction(&vad(true), 0.699));
    }

    #[test]
    fn reduction_does_not_skip_for_quiet_audio_at_speech_ratio_threshold() {
        assert!(!should_skip_reduction(&vad(false), 0.7));
    }

    #[test]
    fn reduction_does_not_skip_for_quiet_audio_below_speech_ratio_threshold() {
        assert!(!should_skip_reduction(&vad(false), 0.699));
    }

    #[test]
    fn speech_ratio_divides_speech_duration_by_duration() {
        let mut result = vad(false);
        result.duration_s = 4.0;
        result.speech_duration_s = 1.5;

        assert_eq!(speech_ratio(&result), 0.375);
    }

    #[test]
    fn speech_ratio_is_zero_for_zero_duration() {
        let mut result = vad(false);
        result.duration_s = 0.0;
        result.speech_duration_s = 1.0;

        assert_eq!(speech_ratio(&result), 0.0);
    }

    #[test]
    fn vad_binary_env_override_is_used_verbatim() {
        let candidate = vad_binary_candidate_from(
            Ok(PathBuf::from("/runtime/bin/solstone-transcribe")),
            |name| {
                assert_eq!(name, VAD_BINARY_ENV);
                Some("/custom/vad-helper".to_owned())
            },
        )
        .unwrap();

        assert_eq!(candidate, PathBuf::from("/custom/vad-helper"));
    }

    #[test]
    fn vad_binary_lookup_uses_current_executable_sibling_directory() {
        let candidate = vad_binary_candidate_from(
            Ok(PathBuf::from("/runtime/bin/solstone-transcribe")),
            |_| None,
        )
        .unwrap();

        assert_eq!(
            candidate,
            PathBuf::from("/runtime/bin").join(VAD_BINARY_NAME)
        );
    }

    #[test]
    fn vad_response_converts_sample_indices_to_seconds() {
        let result = parse_vad_response(
            &json!({
                "schema": RESPONSE_SCHEMA,
                "duration": 2.0,
                "speech_duration": 0.75,
                "has_speech": true,
                "speech": [{"start": 16000, "end": 28000}],
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result.duration_s, 2.0);
        assert_eq!(result.speech_duration_s, 0.75);
        assert!(result.has_speech);
        assert_eq!(result.speech_segments, vec![(1.0, 1.75)]);
        assert_eq!(result.noisy_rms, None);
    }

    #[test]
    fn vad_usage_error_is_a_driver_failure() {
        let error = helper_error(64);

        assert_eq!(error.exit_code(), 1);
    }

    #[test]
    fn vad_unavailable_error_defers() {
        let error = helper_error(69);

        assert_eq!(error.exit_code(), 69);
    }

    #[test]
    fn vad_tempfail_error_is_retryable() {
        let error = helper_error(75);

        assert_eq!(error.exit_code(), 75);
    }

    fn helper_error(exit_code: i32) -> TranscribeError {
        let error = parse_vad_error(
            exit_code,
            &json!({
                "schema": ERROR_SCHEMA,
                "reason": "injected",
                "detail": "injected helper failure",
            })
            .to_string(),
        );
        assert!(matches!(error, TranscribeError::VadHelper { .. }));
        error
    }

    #[test]
    fn vad_binary_current_executable_failure_is_typed() {
        let error =
            vad_binary_candidate_from(Err(io::Error::other("injected")), |_| None).unwrap_err();

        assert!(matches!(error, TranscribeError::VadBinary { .. }));
        assert_eq!(error.exit_code(), 78);
    }
}
