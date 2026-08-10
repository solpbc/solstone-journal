// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! FluidAudio CoreML Parakeet helper client.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use solstone_core_journal_config::JournalConfigRead;
use solstone_core_observe_audio::{SAMPLE_RATE, audio_to_wav_bytes};

use crate::TranscribeError;
use crate::backend::parakeet_cpp::{ModelInfo, TranscriptionResponse, TranscriptionWord};
use crate::config::{
    parakeet_coreml_cache_dir, parakeet_coreml_model_version, parakeet_coreml_timeout,
};

const HELPER_ENV_KEY: &str = "SOLSTONE_PARAKEET_HELPER";
const HELPER_RELATIVE: &str = "solstone/observe/transcribe/parakeet_helper";
const HELPER_NAME: &str = "parakeet-helper";
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Invoke CoreML transcription. Model metadata is deliberately not probed here.
pub(crate) fn transcribe(
    audio: &[f32],
    config: &JournalConfigRead,
) -> Result<TranscriptionResponse, TranscribeError> {
    let helper = resolve_helper_path();
    transcribe_with_helper(
        audio,
        &helper,
        &parakeet_coreml_cache_dir(config),
        &parakeet_coreml_model_version(config),
        parakeet_coreml_timeout(config),
    )
}

/// Probe metadata only after the caller has completed a successful transcription.
pub(crate) fn get_model_info(config: &JournalConfigRead) -> Result<ModelInfo, TranscribeError> {
    let helper = resolve_helper_path();
    get_model_info_with_helper(&helper, &parakeet_coreml_model_version(config))
}

fn transcribe_with_helper(
    audio: &[f32],
    helper: &Path,
    cache_dir: &Path,
    model_version: &str,
    timeout: Duration,
) -> Result<TranscriptionResponse, TranscribeError> {
    require_ready_helper(helper)?;
    let wav = audio_to_wav_bytes(audio, SAMPLE_RATE)
        .map_err(|error| failure("coreml_tempfile_failed", error.to_string()))?;
    let mut temporary = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile()
        .map_err(|error| failure("coreml_tempfile_failed", error.to_string()))?;
    temporary
        .write_all(&wav)
        .map_err(|error| failure("coreml_tempfile_failed", error.to_string()))?;

    let arguments = [
        "--cache-dir".to_owned(),
        cache_dir.display().to_string(),
        "--model".to_owned(),
        model_version.to_owned(),
        temporary.path().display().to_string(),
    ];
    let output = run_helper(helper, &arguments, timeout).map_err(|error| match error {
        HelperRunError::TimedOut => deferred(
            "coreml_helper_timeout",
            format!(
                "Parakeet helper timed out after {:.1}s",
                timeout.as_secs_f64()
            ),
        ),
        error => failure("coreml_helper_exit_failed", error.to_string()),
    })?;
    if !output.status.success() {
        return Err(failure(
            "coreml_helper_exit_failed",
            helper_failure_detail(&output),
        ));
    }
    parse_coreml_response(&output.stdout).map_err(|error| match error {
        CoremlResponseError::InvalidJson(detail) => failure("coreml_invalid_json", detail),
        CoremlResponseError::Contract(detail) => failure("coreml_contract_violation", detail),
    })
}

fn get_model_info_with_helper(
    helper: &Path,
    model_version: &str,
) -> Result<ModelInfo, TranscribeError> {
    let output = run_helper(helper, &["--version".to_owned()], VERSION_TIMEOUT)
        .map_err(|error| failure("coreml_version_probe_failed", error.to_string()))?;
    if !output.status.success() {
        return Err(failure(
            "coreml_version_probe_failed",
            helper_failure_detail(&output),
        ));
    }
    parse_version_response(&output.stdout)
        .map_err(|error| failure("coreml_version_probe_failed", error))?;
    Ok(ModelInfo {
        model: format!("parakeet-tdt-0.6b-{model_version}"),
        device: "ane".to_owned(),
        compute_type: "coreml_fp16".to_owned(),
    })
}

fn require_ready_helper(helper: &Path) -> Result<(), TranscribeError> {
    let metadata = fs::metadata(helper).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            deferred(
                "coreml_helper_missing",
                format!("Parakeet helper is missing: {}", helper.display()),
            )
        } else {
            failure("coreml_helper_exit_failed", error.to_string())
        }
    })?;
    if !metadata.is_file() {
        return Err(deferred(
            "coreml_helper_missing",
            format!("Parakeet helper is not a file: {}", helper.display()),
        ));
    }
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(deferred(
            "coreml_helper_not_executable",
            format!("Parakeet helper is not executable: {}", helper.display()),
        ));
    }
    Ok(())
}

fn resolve_helper_path() -> PathBuf {
    if let Some(value) = std::env::var_os(HELPER_ENV_KEY) {
        return resolve_override_path(PathBuf::from(value));
    }

    let source_directories = source_helper_directories(Path::new(env!("CARGO_MANIFEST_DIR")));
    if let Some(path) = source_directories
        .iter()
        .map(|directory| directory.join("_bin").join(HELPER_NAME))
        .find(|path| path.exists())
    {
        return path;
    }
    if let Some(path) = installed_helper_paths()
        .into_iter()
        .map(|directory| directory.join("_bin").join(HELPER_NAME))
        .find(|path| path.exists())
    {
        return path;
    }

    source_directories
        .into_iter()
        .find(|directory| directory.is_dir())
        .unwrap_or_else(|| PathBuf::from(HELPER_RELATIVE))
        .join(".build")
        .join("release")
        .join(HELPER_NAME)
}

fn resolve_override_path(path: PathBuf) -> PathBuf {
    let expanded = match path.to_str().and_then(|value| value.strip_prefix("~/")) {
        Some(remainder) => std::env::home_dir()
            .map(|home| home.join(remainder))
            .unwrap_or(path),
        None => path,
    };
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(&expanded))
            .unwrap_or(expanded)
    }
}

fn source_helper_directories(manifest_directory: &Path) -> Vec<PathBuf> {
    manifest_directory
        .ancestors()
        .map(|ancestor| ancestor.join(HELPER_RELATIVE))
        .collect()
}

fn installed_helper_paths() -> Vec<PathBuf> {
    let Ok(executable) = std::env::current_exe() else {
        return Vec::new();
    };
    executable
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .flat_map(|root| python_helper_directories(&root.join("lib")))
        .collect()
}

fn python_helper_directories(library_directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(library_directory) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().ok().is_some_and(|kind| kind.is_dir())
                && entry.file_name().to_string_lossy().starts_with("python3.")
        })
        .map(|entry| {
            entry
                .path()
                .join("site-packages/solstone/observe/transcribe/parakeet_helper")
        })
        .collect()
}

fn run_helper(
    helper: &Path,
    arguments: &[String],
    timeout: Duration,
) -> Result<HelperOutput, HelperRunError> {
    let mut child = Command::new(helper)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| HelperRunError::Spawn(error.to_string()))?;
    let started = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| HelperRunError::Wait(error.to_string()))?
        {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HelperRunError::TimedOut);
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    let stdout = read_child_stream(child.stdout.take(), "stdout")?;
    let stderr = read_child_stream(child.stderr.take(), "stderr")?;
    Ok(HelperOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_child_stream(stream: Option<impl Read>, name: &str) -> Result<String, HelperRunError> {
    let mut stream = stream.ok_or_else(|| HelperRunError::Read(format!("missing {name} pipe")))?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|error| HelperRunError::Read(error.to_string()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_coreml_response(text: &str) -> Result<TranscriptionResponse, CoremlResponseError> {
    let payload: Value = serde_json::from_str(text)
        .map_err(|error| CoremlResponseError::InvalidJson(error.to_string()))?;
    let object = payload.as_object().ok_or_else(|| {
        CoremlResponseError::InvalidJson("helper JSON was not an object".to_owned())
    })?;
    let transcript = required_string(object, "transcript")?.trim().to_owned();
    required_finite_number(object, "audio_sec")?;
    required_u64(object, "transcribe_ms")?;
    required_finite_number(object, "rtfx")?;
    let token_timings = object
        .get("token_timings")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CoremlResponseError::Contract("helper response missing token_timings[]".to_owned())
        })?;
    if token_timings.is_empty() {
        return transcript
            .is_empty()
            .then_some(TranscriptionResponse {
                words: Vec::new(),
                text: transcript,
            })
            .ok_or_else(|| {
                CoremlResponseError::Contract(
                    "helper returned transcript text without token timings".to_owned(),
                )
            });
    }

    let tokens = token_timings
        .iter()
        .map(parse_token_timing)
        .collect::<Result<Vec<_>, _>>()?;
    let words = collapse_subwords_to_words(&tokens);
    if words.is_empty() && !transcript.is_empty() {
        return Err(CoremlResponseError::Contract(
            "helper token timings collapsed to no words".to_owned(),
        ));
    }
    Ok(TranscriptionResponse {
        words,
        text: transcript,
    })
}

fn parse_version_response(text: &str) -> Result<(), String> {
    let payload: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let object = payload
        .as_object()
        .ok_or_else(|| "helper version JSON was not an object".to_owned())?;
    for field in [
        "fluidaudio_version",
        "model_version_default",
        "swift_version",
        "hardware",
        "macos_version",
    ] {
        object
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("helper version response missing {field}"))?;
    }
    Ok(())
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, CoremlResponseError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoremlResponseError::Contract(format!("helper response missing {key}")))
}

fn required_finite_number(
    object: &Map<String, Value>,
    key: &str,
) -> Result<f64, CoremlResponseError> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| CoremlResponseError::Contract(format!("helper response has invalid {key}")))
}

fn required_u64(object: &Map<String, Value>, key: &str) -> Result<u64, CoremlResponseError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| CoremlResponseError::Contract(format!("helper response has invalid {key}")))
}

fn parse_token_timing(value: &Value) -> Result<TokenTiming, CoremlResponseError> {
    let object = value.as_object().ok_or_else(|| {
        CoremlResponseError::Contract("token timing must be an object".to_owned())
    })?;
    let token = required_string(object, "token")?.to_owned();
    required_u64(object, "token_id")?;
    Ok(TokenTiming {
        token,
        start: required_finite_number(object, "start")?,
        end: required_finite_number(object, "end")?,
        confidence: required_finite_number(object, "confidence")?,
    })
}

fn collapse_subwords_to_words(tokens: &[TokenTiming]) -> Vec<TranscriptionWord> {
    let mut words = Vec::new();
    let mut current_parts = Vec::new();
    let mut confidences = Vec::new();
    let mut start = None;
    let mut end = None;

    let flush = |words: &mut Vec<TranscriptionWord>,
                 current_parts: &mut Vec<String>,
                 confidences: &mut Vec<f64>,
                 start: &mut Option<f64>,
                 end: &mut Option<f64>| {
        let (Some(word_start), Some(word_end)) = (*start, *end) else {
            current_parts.clear();
            confidences.clear();
            *start = None;
            *end = None;
            return;
        };
        if !current_parts.is_empty() {
            words.push(TranscriptionWord {
                word: format!(" {}", current_parts.concat().trim_start()),
                start: word_start,
                end: word_end,
                probability: confidences.iter().copied().fold(f64::INFINITY, f64::min),
            });
        }
        current_parts.clear();
        confidences.clear();
        *start = None;
        *end = None;
    };

    for token in tokens {
        let punctuation = is_punctuation(&token.token);
        let starts_new = token.token.starts_with('▁') || token.token.starts_with(' ');
        if starts_new && !current_parts.is_empty() && !punctuation {
            flush(
                &mut words,
                &mut current_parts,
                &mut confidences,
                &mut start,
                &mut end,
            );
        }
        let mut cleaned = token.token.trim_start_matches('▁');
        if starts_new {
            cleaned = cleaned.trim_start_matches(' ');
        }
        let cleaned = cleaned.to_owned();
        current_parts.push(cleaned);
        confidences.push(token.confidence);
        if !punctuation && start.is_none() {
            start = Some(token.start);
        }
        if !punctuation {
            end = Some(token.end);
        }
    }
    flush(
        &mut words,
        &mut current_parts,
        &mut confidences,
        &mut start,
        &mut end,
    );
    words
}

fn is_punctuation(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|character| {
            character.is_ascii_punctuation()
                || matches!(character, '—' | '…' | '’' | '‘' | '“' | '”')
        })
}

fn helper_failure_detail(output: &HelperOutput) -> String {
    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        format!("Parakeet helper exited with {}", output.status)
    } else {
        stderr.to_owned()
    }
}

fn deferred(reason: impl Into<String>, detail: impl Into<String>) -> TranscribeError {
    TranscribeError::ParakeetCoremlDeferred {
        reason: reason.into(),
        detail: detail.into(),
    }
}

fn failure(reason: impl Into<String>, detail: impl Into<String>) -> TranscribeError {
    TranscribeError::ParakeetCoremlFailure {
        reason: reason.into(),
        detail: detail.into(),
    }
}

struct HelperOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
enum HelperRunError {
    Spawn(String),
    Wait(String),
    Read(String),
    TimedOut,
}

impl std::fmt::Display for HelperRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(detail) | Self::Wait(detail) | Self::Read(detail) => {
                formatter.write_str(detail)
            }
            Self::TimedOut => formatter.write_str("Parakeet helper timed out"),
        }
    }
}

enum CoremlResponseError {
    InvalidJson(String),
    Contract(String),
}

struct TokenTiming {
    token: String,
    start: f64,
    end: f64,
    confidence: f64,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::{get_model_info_with_helper, transcribe_with_helper};
    use crate::TranscribeError;

    const SUCCESS: &str = r#"{"transcript":"hello world!","audio_sec":1.0,"transcribe_ms":2,"rtfx":3.0,"token_timings":[{"token":"▁hel","token_id":1,"start":0.0,"end":0.1,"confidence":0.9},{"token":"lo","token_id":2,"start":0.1,"end":0.2,"confidence":0.6},{"token":"▁world","token_id":3,"start":0.2,"end":0.4,"confidence":0.8},{"token":"!","token_id":4,"start":0.4,"end":0.5,"confidence":0.7}]}"#;
    const VERSION: &str = r#"{"fluidaudio_version":"0.14.0","model_version_default":"v3","swift_version":"Swift","hardware":"M4","macos_version":"26"}"#;

    #[test]
    fn missing_helper_defers() {
        let error = transcribe_with_helper(
            &[0.0],
            Path::new("/definitely/missing/parakeet-helper"),
            Path::new("/tmp/cache"),
            "v3",
            Duration::from_secs(1),
        )
        .unwrap_err();

        assert_deferred_reason(error, "coreml_helper_missing");
    }

    #[test]
    fn non_executable_helper_defers() {
        let temporary = tempfile::tempdir().unwrap();
        let helper = temporary.path().join("parakeet-helper");
        fs::write(&helper, "#!/bin/sh\nexit 0\n").unwrap();

        let error = transcribe(&helper).unwrap_err();

        assert_deferred_reason(error, "coreml_helper_not_executable");
    }

    #[test]
    fn timed_out_helper_defers() {
        let helper = helper("sleep 1");

        let error = transcribe_with_timeout(&helper, Duration::from_millis(25)).unwrap_err();

        assert_deferred_reason(error, "coreml_helper_timeout");
    }

    #[test]
    fn nonzero_helper_fails() {
        let helper = helper("echo helper-error >&2\nexit 5");

        assert_failure_reason(
            transcribe(&helper).unwrap_err(),
            "coreml_helper_exit_failed",
        );
    }

    #[test]
    fn malformed_helper_json_fails() {
        let helper = helper("printf '%s\\n' not-json");

        assert_failure_reason(transcribe(&helper).unwrap_err(), "coreml_invalid_json");
    }

    #[test]
    fn contract_violating_helper_json_fails() {
        let helper = helper(
            "printf '%s\\n' '{\"transcript\":\"hello\",\"audio_sec\":1.0,\"transcribe_ms\":2,\"rtfx\":3.0,\"token_timings\":[]}'",
        );

        assert_failure_reason(
            transcribe(&helper).unwrap_err(),
            "coreml_contract_violation",
        );
    }

    #[test]
    fn successful_helper_collapses_subwords_and_punctuation() {
        let helper = helper(&format!("printf '%s\\n' '{SUCCESS}'"));

        let response = transcribe(&helper).unwrap();

        assert_eq!(response.text, "hello world!");
        assert_eq!(response.words.len(), 2);
        assert_eq!(response.words[0].word, " hello");
        assert_eq!(response.words[0].probability, 0.6);
        assert_eq!(response.words[1].word, " world!");
        assert_eq!(response.words[1].probability, 0.7);
    }

    #[test]
    fn version_probe_succeeds_only_with_a_valid_version_envelope() {
        let helper = helper(&format!(
            "if [ \"$1\" = \"--version\" ]; then printf '%s\\n' '{VERSION}'; else printf '%s\\n' '{SUCCESS}'; fi"
        ));

        let info = get_model_info_with_helper(&helper, "v2").unwrap();

        assert_eq!(info.model, "parakeet-tdt-0.6b-v2");
        assert_eq!(info.device, "ane");
        assert_eq!(info.compute_type, "coreml_fp16");
    }

    #[test]
    fn version_probe_failure_is_hard_not_deferred() {
        let helper = helper("exit 5");

        assert_failure_reason(
            get_model_info_with_helper(&helper, "v3").unwrap_err(),
            "coreml_version_probe_failed",
        );
    }

    #[test]
    fn helper_receives_direct_coreml_argv() {
        let temporary = tempfile::tempdir().unwrap();
        let arguments = temporary.path().join("arguments");
        let helper = write_helper(
            temporary.path(),
            &format!(
                "printf '%s\\n' \"$@\" > {}\nprintf '%s\\n' '{SUCCESS}'",
                shell_quote(&arguments)
            ),
        );
        let cache_dir = temporary.path().join("cache");

        transcribe_with_helper(&[0.0], &helper, &cache_dir, "v2", Duration::from_secs(1)).unwrap();

        let arguments = fs::read_to_string(arguments).unwrap();
        let arguments: Vec<_> = arguments.lines().collect();
        assert_eq!(
            arguments[..4],
            ["--cache-dir", cache_dir.to_str().unwrap(), "--model", "v2"]
        );
        assert!(arguments[4].ends_with(".wav"));
    }

    fn transcribe(helper: &Path) -> Result<super::TranscriptionResponse, TranscribeError> {
        transcribe_with_timeout(helper, Duration::from_secs(1))
    }

    fn transcribe_with_timeout(
        helper: &Path,
        timeout: Duration,
    ) -> Result<super::TranscriptionResponse, TranscribeError> {
        transcribe_with_helper(&[0.0], helper, Path::new("/tmp/cache"), "v3", timeout)
    }

    fn helper(body: &str) -> PathBuf {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.keep();
        write_helper(&path, body)
    }

    fn write_helper(directory: &Path, body: &str) -> PathBuf {
        let helper = directory.join("parakeet-helper");
        fs::write(&helper, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&helper, permissions).unwrap();
        helper
    }

    fn shell_quote(path: &Path) -> String {
        format!(
            "'{}'",
            path.display().to_string().replace('\'', "'\\\"'\\\"'")
        )
    }

    fn assert_deferred_reason(error: TranscribeError, expected_reason: &str) {
        assert_eq!(error.exit_code(), 69);
        let TranscribeError::ParakeetCoremlDeferred { reason, .. } = error else {
            panic!("expected deferred CoreML error");
        };
        assert_eq!(reason, expected_reason);
    }

    fn assert_failure_reason(error: TranscribeError, expected_reason: &str) {
        assert_eq!(error.exit_code(), 1);
        let TranscribeError::ParakeetCoremlFailure { reason, .. } = error else {
            panic!("expected hard CoreML error");
        };
        assert_eq!(reason, expected_reason);
    }
}
