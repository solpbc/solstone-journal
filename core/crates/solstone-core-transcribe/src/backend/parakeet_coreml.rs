// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! FluidAudio CoreML Parakeet helper client.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use solstone_core_journal_config::{JournalConfigRead, parakeet_coreml::parakeet_coreml_cache_dir};
use solstone_core_observe_audio::{SAMPLE_RATE, audio_to_wav_bytes};
use solstone_core_system::process::{Disposition, LaunchAuthority, LaunchError, launch};

use crate::TranscribeError;
use crate::backend::parakeet_cpp::{ModelInfo, TranscriptionResponse, TranscriptionWord};
use crate::config::{parakeet_coreml_model_version, parakeet_coreml_timeout};

const HELPER_ENV_KEY: &str = "SOLSTONE_PARAKEET_HELPER";
const HELPER_RELATIVE: &str = "parakeet-helper";
const HELPER_NAME: &str = "parakeet-helper";
const HELPER_SPAWN_RETRIES: usize = 3;
const HELPER_SPAWN_RETRY_DELAY: Duration = Duration::from_millis(10);
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Invoke CoreML transcription. Model metadata is deliberately not probed here.
pub(crate) fn transcribe(
    audio: &[f32],
    config: &JournalConfigRead,
) -> Result<TranscriptionResponse, TranscribeError> {
    let helper = resolve_helper_path();
    let home = std::env::home_dir().unwrap_or_default();
    transcribe_with_helper(
        audio,
        &helper,
        &parakeet_coreml_cache_dir(config, &home),
        &parakeet_coreml_model_version(config),
        parakeet_coreml_timeout(config),
    )
}

/// Probe metadata only after the caller has completed a successful transcription.
pub(crate) fn get_model_info(config: &JournalConfigRead) -> Result<ModelInfo, TranscribeError> {
    let helper = resolve_helper_path();
    get_model_info_with_helper(
        &helper,
        &parakeet_coreml_model_version(config),
        VERSION_TIMEOUT,
    )
}

pub(crate) fn transcribe_with_helper(
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

    // The helper's hardcoded fallback is unreachable here because this call
    // always passes --cache-dir; a future invocation change could make it live.
    let arguments = transcribe_helper_arguments(cache_dir, model_version, temporary.path());
    let output = run_helper(helper, &arguments, timeout)
        .map_err(|error| map_helper_run_error(error, timeout))?;
    let output = map_helper_exit(output)?;
    parse_coreml_response(&output.stdout).map_err(|error| match error {
        CoremlResponseError::InvalidJson(detail) => failure("coreml_invalid_json", detail),
        CoremlResponseError::Contract(detail) => failure("coreml_contract_violation", detail),
    })
}

pub(crate) fn get_model_info_with_helper(
    helper: &Path,
    model_version: &str,
    timeout: Duration,
) -> Result<ModelInfo, TranscribeError> {
    let output = run_helper(helper, &["--version".to_owned()], timeout)
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
            failure("coreml_helper_launch_failed", error.to_string())
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
    if let Some(path) = installed_helper_binaries()
        .into_iter()
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

fn installed_helper_binaries() -> Vec<PathBuf> {
    let Ok(executable) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(parent) = executable.parent() else {
        return Vec::new();
    };
    let mut paths = vec![parent.join(HELPER_NAME)];
    for ancestor in parent.ancestors() {
        paths.push(ancestor.join("bin").join(HELPER_NAME));
        paths.push(ancestor.join("lib").join(HELPER_RELATIVE).join(HELPER_NAME));
    }
    paths
}

fn transcribe_helper_arguments(
    cache_dir: &Path,
    model_version: &str,
    wav_path: &Path,
) -> [String; 5] {
    [
        "--cache-dir".to_owned(),
        cache_dir.display().to_string(),
        "--model".to_owned(),
        model_version.to_owned(),
        wav_path.display().to_string(),
    ]
}

fn map_helper_run_error(error: HelperRunError, timeout: Duration) -> TranscribeError {
    match error {
        HelperRunError::TimedOut => deferred(
            "coreml_helper_timeout",
            format!(
                "Parakeet helper timed out after {:.1}s",
                timeout.as_secs_f64()
            ),
        ),
        error => failure("coreml_helper_launch_failed", error.to_string()),
    }
}

fn map_helper_exit(output: HelperOutput) -> Result<HelperOutput, TranscribeError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(failure(
            "coreml_helper_exit_failed",
            helper_failure_detail(&output),
        ))
    }
}

fn retry_busy_spawn<T>(
    mut spawn: impl FnMut() -> io::Result<T>,
    mut sleep: impl FnMut(Duration),
) -> io::Result<T> {
    let mut retries = 0;
    loop {
        match spawn() {
            Ok(value) => return Ok(value),
            Err(error)
                if error.kind() == io::ErrorKind::ExecutableFileBusy
                    && retries < HELPER_SPAWN_RETRIES =>
            {
                retries += 1;
                sleep(HELPER_SPAWN_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
}

fn spawn_helper(helper: &Path, arguments: &[String]) -> io::Result<Child> {
    retry_busy_spawn(
        || {
            Command::new(helper)
                .args(arguments)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .spawn()
        },
        thread::sleep,
    )
}

fn kill_helper_group(pgid: rustix::process::Pid) -> io::Result<()> {
    match rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(io::Error::from(error)),
    }
}

fn terminate_helper_child(child: &mut Child, _timeout: Duration) -> Result<(), LaunchError> {
    match i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        Some(pgid) => kill_helper_group(pgid).map_err(LaunchError::Terminate),
        None => child.kill().map_err(LaunchError::Terminate),
    }
}

// Inverts signal_aware_exit_code: non-negative = normal exit, negative = -signal,
// back into the raw Unix wait-status encoding ExitStatus::from_raw expects.
fn exit_status_from_code(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;

    if (0..=255).contains(&code) {
        ExitStatus::from_raw(code << 8)
    } else if code < 0 {
        ExitStatus::from_raw(-code)
    } else {
        ExitStatus::from_raw(code)
    }
}

trait HelperSupervisor {
    fn observe_without_reap(&mut self) -> io::Result<bool>;
    fn kill_group(&mut self) -> io::Result<()>;
    fn close_owned_pipes(&mut self);
    fn reap_root(&mut self) -> io::Result<ExitStatus>;
    fn join_readers(&mut self) -> Result<(String, String), HelperRunError>;
}

struct LiveHelper {
    authority: LaunchAuthority,
    reaped_exit: Option<i32>,
    timeout: Duration,
    stdout_reader: Option<thread::JoinHandle<Result<String, HelperRunError>>>,
    stderr_reader: Option<thread::JoinHandle<Result<String, HelperRunError>>>,
}

impl HelperSupervisor for LiveHelper {
    fn observe_without_reap(&mut self) -> io::Result<bool> {
        if self.reaped_exit.is_none() {
            self.reaped_exit = self.authority.poll()?;
        }
        Ok(self.reaped_exit.is_some())
    }

    fn kill_group(&mut self) -> io::Result<()> {
        self.authority
            .terminate(self.timeout)
            .map_err(io::Error::other)
    }

    fn close_owned_pipes(&mut self) {
        drop(self.authority.take_stdout());
        drop(self.authority.take_stderr());
    }

    fn reap_root(&mut self) -> io::Result<ExitStatus> {
        if let Some(exit_code) = self.reaped_exit.take() {
            return Ok(exit_status_from_code(exit_code));
        }
        retry_interrupted(|| self.authority.wait().map(exit_status_from_code))
    }

    fn join_readers(&mut self) -> Result<(String, String), HelperRunError> {
        let stdout = match self.stdout_reader.take() {
            Some(reader) => join_reader(reader, "stdout")?,
            None => String::new(),
        };
        let stderr = match self.stderr_reader.take() {
            Some(reader) => join_reader(reader, "stderr")?,
            None => String::new(),
        };
        Ok((stdout, stderr))
    }
}

fn conclude_helper<S: HelperSupervisor>(
    session: &mut S,
) -> Result<(ExitStatus, String, String), HelperRunError> {
    session
        .kill_group()
        .map_err(|error| HelperRunError::Wait(error.to_string()))?;
    session.close_owned_pipes();
    let reaped = session
        .reap_root()
        .map_err(|error| HelperRunError::Wait(error.to_string()))?;
    let (stdout, stderr) = session.join_readers()?;
    Ok((reaped, stdout, stderr))
}

fn run_helper(
    helper: &Path,
    arguments: &[String],
    timeout: Duration,
) -> Result<HelperOutput, HelperRunError> {
    let authority = launch(
        Disposition::IndependentBoundedHelper { timeout },
        || spawn_helper(helper, arguments),
        Box::new(terminate_helper_child),
    )
    .map_err(|error| HelperRunError::Spawn(error.to_string()))?;
    let mut session = LiveHelper {
        authority,
        reaped_exit: None,
        timeout,
        stdout_reader: None,
        stderr_reader: None,
    };
    let stdout = match session.authority.take_stdout() {
        Some(stdout) => stdout,
        None => {
            let _ = conclude_helper(&mut session);
            return Err(HelperRunError::Read("missing stdout pipe".to_owned()));
        }
    };
    let stderr = match session.authority.take_stderr() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            let _ = conclude_helper(&mut session);
            return Err(HelperRunError::Read("missing stderr pipe".to_owned()));
        }
    };
    session.stdout_reader = Some(spawn_reader(stdout));
    session.stderr_reader = Some(spawn_reader(stderr));
    let started = Instant::now();
    loop {
        match session.observe_without_reap() {
            Err(error) => {
                let _ = conclude_helper(&mut session);
                return Err(HelperRunError::Wait(error.to_string()));
            }
            Ok(false) if started.elapsed() >= timeout => {
                return match conclude_helper(&mut session) {
                    Ok(_) => Err(HelperRunError::TimedOut),
                    Err(error) => Err(error),
                };
            }
            Ok(false) => thread::sleep(Duration::from_millis(10)),
            Ok(true) => break,
        }
    }
    let (status, stdout, stderr) = conclude_helper(&mut session)?;
    Ok(HelperOutput {
        status,
        stdout,
        stderr,
    })
}

fn spawn_reader<R>(stream: R) -> thread::JoinHandle<Result<String, HelperRunError>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_child_stream(stream))
}

fn join_reader(
    reader: thread::JoinHandle<Result<String, HelperRunError>>,
    name: &str,
) -> Result<String, HelperRunError> {
    reader
        .join()
        .map_err(|_| HelperRunError::Read(format!("{name} reader panicked")))?
}

fn read_child_stream(mut stream: impl Read) -> Result<String, HelperRunError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = retry_interrupted(|| stream.read(&mut buffer))
            .map_err(|error| HelperRunError::Read(error.to_string()))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn retry_interrupted<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
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

#[derive(Debug)]
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

#[derive(Debug)]
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
    use std::io::{self, Cursor, Read};
    use std::os::unix::process::ExitStatusExt;
    use std::path::Path;
    use std::process::ExitStatus;
    use std::time::Duration;

    use super::{
        CoremlResponseError, HELPER_RELATIVE, HELPER_SPAWN_RETRY_DELAY, HelperOutput,
        HelperRunError, HelperSupervisor, conclude_helper, exit_status_from_code,
        kill_helper_group, map_helper_exit, map_helper_run_error, parse_coreml_response,
        parse_version_response, read_child_stream, retry_busy_spawn, transcribe_helper_arguments,
        transcribe_with_helper,
    };
    use crate::TranscribeError;

    const SUCCESS: &str = r#"{"transcript":"hello world!","audio_sec":1.0,"transcribe_ms":2,"rtfx":3.0,"token_timings":[{"token":"▁hel","token_id":1,"start":0.0,"end":0.1,"confidence":0.9},{"token":"lo","token_id":2,"start":0.1,"end":0.2,"confidence":0.6},{"token":"▁world","token_id":3,"start":0.2,"end":0.4,"confidence":0.8},{"token":"!","token_id":4,"start":0.4,"end":0.5,"confidence":0.7}]}"#;
    const VERSION: &str = r#"{"fluidaudio_version":"0.14.0","model_version_default":"v3","swift_version":"Swift","hardware":"M4","macos_version":"26"}"#;

    #[test]
    fn exit_status_from_code_round_trips_signal_aware_codes() {
        let zero = exit_status_from_code(0);
        assert!(zero.success());
        assert_eq!(zero.code(), Some(0));

        let one = exit_status_from_code(1);
        assert!(!one.success());
        assert_eq!(one.code(), Some(1));

        let max_byte = exit_status_from_code(255);
        assert!(!max_byte.success());
        assert_eq!(max_byte.code(), Some(255));

        let sigkill = exit_status_from_code(-9);
        assert!(!sigkill.success());
        assert_eq!(sigkill.code(), None);
        assert_eq!(sigkill.signal(), Some(9));

        let sigterm = exit_status_from_code(-15);
        assert!(!sigterm.success());
        assert_eq!(sigterm.code(), None);
        assert_eq!(sigterm.signal(), Some(15));
    }

    #[test]
    fn helper_package_lives_next_to_this_crate() {
        let package = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(HELPER_RELATIVE)
            .join("Package.swift");
        assert!(
            package.is_file(),
            "helper Package.swift must sit at {package}",
            package = package.display()
        );
    }

    #[test]
    fn missing_helper_defers() {
        let error = transcribe_with_helper(
            &[0.0],
            Path::new("/definitely/missing/parakeet-helper"),
            Path::new("/var/tmp/cache"),
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

        let error = transcribe_with_helper(
            &[0.0],
            &helper,
            temporary.path(),
            "v3",
            Duration::from_secs(1),
        )
        .unwrap_err();

        assert_deferred_reason(error, "coreml_helper_not_executable");
    }

    #[test]
    fn child_pipe_reader_retries_interrupted_reads() {
        let output = read_child_stream(InterruptedReader {
            interrupted: true,
            bytes: b"helper output".to_vec(),
        })
        .unwrap();

        assert_eq!(output, "helper output");
    }

    #[test]
    fn read_child_stream_drains_more_than_seventy_thousand_bytes_without_a_cap() {
        let bytes = vec![b'x'; 70_001];
        let output = read_child_stream(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(output.len(), 70_001);
        assert_eq!(output.as_bytes(), bytes);
    }

    #[test]
    fn coreml_response_parse_table() {
        assert!(matches!(
            parse_coreml_response("not-json"),
            Err(CoremlResponseError::InvalidJson(_))
        ));
        assert!(matches!(
            parse_coreml_response(
                r#"{"transcript":"hello","audio_sec":1.0,"transcribe_ms":2,"rtfx":3.0,"token_timings":[]}"#,
            ),
            Err(CoremlResponseError::Contract(_))
        ));

        let response = parse_coreml_response(SUCCESS).unwrap();
        assert_eq!(response.text, "hello world!");
        assert_eq!(response.words.len(), 2);
        assert_eq!(response.words[0].word, " hello");
        assert_eq!(response.words[0].probability, 0.6);
        assert_eq!(response.words[1].word, " world!");
        assert_eq!(response.words[1].probability, 0.7);

        let padded = parse_coreml_response(
            r#"{"transcript":"","audio_sec":1.0,"transcribe_ms":2,"rtfx":3.0,"token_timings":[],"padding":"xxx"}"#,
        )
        .unwrap();
        assert!(padded.words.is_empty());
        assert!(padded.text.is_empty());
    }

    #[test]
    fn version_probe_parse_table() {
        parse_version_response(VERSION).unwrap();
        assert!(parse_version_response("not-json").is_err());
        assert!(parse_version_response("{}").is_err());
        assert_failure_reason(
            super::failure(
                "coreml_version_probe_failed",
                HelperRunError::TimedOut.to_string(),
            ),
            "coreml_version_probe_failed",
        );
    }

    #[test]
    fn helper_run_errors_map_to_transcribe_reasons() {
        let timeout = Duration::from_millis(25);
        assert_deferred_reason(
            map_helper_run_error(HelperRunError::TimedOut, timeout),
            "coreml_helper_timeout",
        );
        for error in [
            HelperRunError::Spawn("boom".into()),
            HelperRunError::Wait("wait".into()),
            HelperRunError::Read("read".into()),
        ] {
            assert_failure_reason(
                map_helper_run_error(error, timeout),
                "coreml_helper_launch_failed",
            );
        }
        assert_failure_reason(
            map_helper_exit(HelperOutput {
                status: ExitStatus::from_raw(5 << 8),
                stdout: String::new(),
                stderr: "helper-error".into(),
            })
            .unwrap_err(),
            "coreml_helper_exit_failed",
        );
    }

    #[test]
    fn transcribe_helper_argv_is_cache_model_wav() {
        let arguments = transcribe_helper_arguments(
            Path::new("/var/tmp/cache"),
            "v2",
            Path::new("/var/tmp/clip.wav"),
        );
        assert_eq!(
            arguments,
            [
                "--cache-dir",
                "/var/tmp/cache",
                "--model",
                "v2",
                "/var/tmp/clip.wav"
            ]
        );
    }

    #[test]
    fn kill_helper_group_treats_missing_group_as_success() {
        kill_helper_group(rustix::process::Pid::from_raw(i32::MAX - 1).unwrap()).unwrap();
    }

    #[test]
    fn conclude_helper_surfaces_kill_group_failure_without_later_steps() {
        let mut session = RecordingSupervisor {
            steps: Vec::new(),
            kill_error: Some(io::ErrorKind::PermissionDenied),
        };
        let error = conclude_helper(&mut session).unwrap_err();
        assert!(matches!(error, HelperRunError::Wait(_)));
        assert_eq!(session.steps, ["kill-group"]);
    }

    #[test]
    fn helper_cleanup_records_kill_close_reap_join_in_order() {
        let mut session = RecordingSupervisor {
            steps: Vec::new(),
            kill_error: None,
        };
        conclude_helper(&mut session).unwrap();
        assert_eq!(
            session.steps,
            [
                "kill-group",
                "close-owned-pipes",
                "reap-root",
                "join-readers",
            ]
        );
    }

    #[test]
    fn busy_spawn_retries_then_succeeds_or_exhausts() {
        for retries_before_success in [1_usize, 2, 3] {
            let mut attempts = 0;
            let mut sleeps = Vec::new();
            retry_busy_spawn(
                || {
                    attempts += 1;
                    if attempts <= retries_before_success {
                        Err::<(), _>(io::Error::from(io::ErrorKind::ExecutableFileBusy))
                    } else {
                        Ok(())
                    }
                },
                |delay| sleeps.push(delay),
            )
            .unwrap();
            assert_eq!(attempts, retries_before_success + 1);
            assert_eq!(sleeps.len(), retries_before_success);
            assert!(
                sleeps
                    .iter()
                    .all(|delay| *delay == HELPER_SPAWN_RETRY_DELAY)
            );
        }

        let mut attempts = 0;
        let mut sleeps = Vec::new();
        let error = retry_busy_spawn(
            || {
                attempts += 1;
                Err::<(), _>(io::Error::from(io::ErrorKind::ExecutableFileBusy))
            },
            |delay| sleeps.push(delay),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ExecutableFileBusy);
        assert_eq!(attempts, 4);
        assert_eq!(sleeps, [HELPER_SPAWN_RETRY_DELAY; 3]);
    }

    #[test]
    fn busy_spawn_does_not_retry_a_non_busy_error() {
        let mut attempts = 0;
        let mut sleeps = Vec::new();
        let error = retry_busy_spawn(
            || {
                attempts += 1;
                if attempts == 1 {
                    Err::<(), _>(io::Error::from(io::ErrorKind::ExecutableFileBusy))
                } else {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                }
            },
            |delay| sleeps.push(delay),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(attempts, 2);
        assert_eq!(sleeps, [HELPER_SPAWN_RETRY_DELAY]);
    }

    struct RecordingSupervisor {
        steps: Vec<&'static str>,
        kill_error: Option<io::ErrorKind>,
    }

    impl HelperSupervisor for RecordingSupervisor {
        fn observe_without_reap(&mut self) -> io::Result<bool> {
            self.steps.push("observe-without-reap");
            Ok(false)
        }

        fn kill_group(&mut self) -> io::Result<()> {
            self.steps.push("kill-group");
            match self.kill_error {
                Some(kind) => Err(io::Error::from(kind)),
                None => Ok(()),
            }
        }

        fn close_owned_pipes(&mut self) {
            self.steps.push("close-owned-pipes");
        }

        fn reap_root(&mut self) -> io::Result<ExitStatus> {
            self.steps.push("reap-root");
            Ok(ExitStatus::from_raw(0))
        }

        fn join_readers(&mut self) -> Result<(String, String), HelperRunError> {
            self.steps.push("join-readers");
            Ok((String::new(), String::new()))
        }
    }

    struct InterruptedReader {
        interrupted: bool,
        bytes: Vec<u8>,
    }

    impl Read for InterruptedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.interrupted {
                self.interrupted = false;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let count = buffer.len().min(self.bytes.len());
            buffer[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes.drain(..count);
            Ok(count)
        }
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
