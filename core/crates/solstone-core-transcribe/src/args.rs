// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CLI syntax, host preflight, and input guards without stage-machine wiring.

use std::env;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{TranscribeError, backend::KNOWN_BACKENDS};

const SUPERVISOR_MESSAGE: &str = "journal isn't running. start it with 'journal up' and retry.";
const SUPERVISOR_TIMEOUT: Duration = Duration::from_millis(200);
const SUPPORTED_AUDIO_FORMATS: [&str; 6] = [".flac", ".m4a", ".mp3", ".ogg", ".opus", ".wav"];

/// Arguments accepted by the standalone transcription seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedArgs {
    pub audio_path: Option<PathBuf>,
    pub all: bool,
    pub redo: bool,
    pub backend: Option<String>,
}

/// A CLI-preflight outcome with an intentional process status and optional stderr text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliError {
    /// argparse-compatible semantic or syntax rejection.
    Usage { message: String },
    /// The supervised child must retry without emitting a duplicate parent-facing message.
    SupervisorSpawnedUnavailable,
    /// An interactive invocation needs a concrete instruction.
    SupervisorUnavailable,
    /// The sibling speaker-analysis runtime is missing an executable or model asset.
    SpeakersInstallation { message: String },
}

impl CliError {
    /// The process status established by the Python CLI contract.
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Usage { .. } => 2,
            Self::SupervisorSpawnedUnavailable => 75,
            Self::SupervisorUnavailable => 1,
            Self::SpeakersInstallation { .. } => 78,
        }
    }

    /// Stderr text, deliberately absent for supervisor-spawned temporary failure.
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Usage { message } | Self::SpeakersInstallation { message } => Some(message),
            Self::SupervisorUnavailable => Some(SUPERVISOR_MESSAGE),
            Self::SupervisorSpawnedUnavailable => None,
        }
    }
}

/// Parse command-line syntax without performing semantic file validation.
pub fn parse_arguments(
    arguments: impl IntoIterator<Item = String>,
) -> Result<ParsedArgs, CliError> {
    let mut parsed = ParsedArgs {
        audio_path: None,
        all: false,
        redo: false,
        backend: None,
    };
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--all" => parsed.all = true,
            "--redo" => parsed.redo = true,
            "--backend" => {
                let backend = arguments
                    .next()
                    .ok_or_else(|| usage("argument --backend: expected one argument"))?;
                if !KNOWN_BACKENDS.contains(&backend.as_str()) {
                    return Err(usage(&format!(
                        "argument --backend: invalid choice: '{backend}' (choose from {})",
                        KNOWN_BACKENDS.join(", ")
                    )));
                }
                if parsed.backend.replace(backend).is_some() {
                    return Err(usage("argument --backend: may only be specified once"));
                }
            }
            option if option.starts_with('-') => {
                return Err(usage(&format!("unrecognized arguments: {option}")));
            }
            path => {
                if parsed.audio_path.replace(PathBuf::from(path)).is_some() {
                    return Err(usage(&format!("unrecognized arguments: {path}")));
                }
            }
        }
    }
    Ok(parsed)
}

/// Preserve Python's observable ordering: supervisor preflight precedes selection errors.
pub(crate) fn preflight_arguments_with<E, C>(
    arguments: impl IntoIterator<Item = String>,
    lookup_env: E,
    connectivity: C,
) -> Result<ParsedArgs, CliError>
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    let parsed = parse_arguments(arguments)?;
    require_solstone_with(lookup_env, connectivity)?;
    validate_selection(&parsed)?;
    Ok(parsed)
}

/// Perform the current process's supervisor check against `journal/health/convey.port`.
///
/// This is shared host-preflight while native journal verbs still lack a common
/// owner; callers must reuse this single owner-facing contract.
pub fn require_solstone(journal_path: &Path) -> Result<(), CliError> {
    require_solstone_with(|name| env::var(name).ok(), || is_solstone_up(journal_path))
}

/// Testable three-branch supervisor preflight.
fn require_solstone_with<E, C>(lookup_env: E, connectivity: C) -> Result<(), CliError>
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    if lookup_env("SOL_SKIP_SUPERVISOR_CHECK").as_deref() == Some("1") || connectivity() {
        return Ok(());
    }
    if lookup_env("SOL_SUPERVISOR_SPAWNED").as_deref() == Some("1") {
        return Err(CliError::SupervisorSpawnedUnavailable);
    }
    Err(CliError::SupervisorUnavailable)
}

/// True only when the recorded local Convey port accepts a 200 ms TCP connection.
pub(crate) fn is_solstone_up(journal_path: &Path) -> bool {
    let Some(port) = read_convey_port(journal_path) else {
        return false;
    };
    TcpStream::connect_timeout(
        &SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        SUPERVISOR_TIMEOUT,
    )
    .is_ok()
}

fn read_convey_port(journal_path: &Path) -> Option<u16> {
    fs::read_to_string(journal_path.join("health/convey.port"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn check_speakers_analyze_installation_with<B, M>(binary: B, model: M) -> Result<(), CliError>
where
    B: FnOnce() -> Result<PathBuf, String>,
    M: Fn(&str) -> Result<PathBuf, TranscribeError>,
{
    let helper = binary().map_err(installation_error)?;
    if !is_executable(&helper) {
        return Err(installation_error(format!(
            "helper-not-executable: {}",
            helper.display()
        )));
    }
    for asset in [
        "wespeaker-resnet34-256.onnx",
        "pyannote-segmentation-3.0.onnx",
    ] {
        model(asset).map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    }
    Ok(())
}

/// Check the native helper and pinned model assets for host diagnostics.
pub fn check_speakers_analyze_installation() -> Result<(), CliError> {
    crate::speakers_installation::validate_speakers_analyze_runtime().map(|_| ())
}

/// The repair guidance paired with speakers-analyze installation failures.
pub fn speakers_analyze_repair_text() -> &'static str {
    "reinstall the journal host stack and restart the journal"
}

/// Probe an explicit VAD helper path with an explicit deadline.
pub fn check_vad_runtime_with(
    binary: impl AsRef<Path>,
    timeout: Duration,
) -> crate::VadRuntimeStatus {
    crate::vad_runtime::probe_vad_runtime(binary.as_ref(), timeout)
}

/// Per-variant VAD runtime repair text.
pub fn vad_runtime_repair_for(status: &crate::VadRuntimeStatus) -> Option<&'static str> {
    crate::vad_runtime::vad_runtime_repair_for(status)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Validate the two `--all` / positional-input semantic rules after supervisor preflight.
pub(crate) fn validate_selection(args: &ParsedArgs) -> Result<(), CliError> {
    if args.all && args.audio_path.is_some() {
        return Err(usage("--all and audio_path are mutually exclusive"));
    }
    if !args.all && args.audio_path.is_none() {
        return Err(usage("provide audio_path or --all"));
    }
    Ok(())
}

/// Resolve and validate one input using the journal-relative convention from Python.
pub(crate) fn resolve_single_audio_path(
    audio_path: &Path,
    journal_path: &Path,
) -> Result<PathBuf, CliError> {
    resolve_single_audio_path_with(audio_path, journal_path, Path::exists)
}

fn resolve_single_audio_path_with<F>(
    audio_path: &Path,
    journal_path: &Path,
    exists: F,
) -> Result<PathBuf, CliError>
where
    F: Fn(&Path) -> bool,
{
    let resolved = if exists(audio_path) {
        audio_path.to_path_buf()
    } else {
        let journal_relative = journal_relative_candidate(audio_path, journal_path);
        if exists(&journal_relative) {
            journal_relative
        } else {
            return Err(usage(&format!(
                "Audio file not found.\n  Tried absolute:         {}\n  Tried journal-relative: {}",
                audio_path.display(),
                journal_relative.display()
            )));
        }
    };
    let extension = resolved
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default();
    if !SUPPORTED_AUDIO_FORMATS.contains(&extension.as_str()) {
        return Err(usage(&format!(
            "Unsupported audio format: {extension}. Supported formats: {}",
            SUPPORTED_AUDIO_FORMATS.join(", ")
        )));
    }
    let parent = resolved
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !is_segment_directory(parent) {
        return Err(usage(&format!(
            "Audio file must be in a segment directory (HHMMSS_LEN/), but parent is: {parent}"
        )));
    }
    Ok(resolved)
}

/// Independent guards for batch discovery, `_process_one`, and `process_audio`.
pub(crate) fn should_skip_batch_processed(audio_path: &Path, redo: bool) -> bool {
    already_processed(audio_path, redo)
}

pub(crate) fn should_skip_process_one_processed(audio_path: &Path, redo: bool) -> bool {
    already_processed(audio_path, redo)
}

pub(crate) fn should_skip_process_audio_processed(audio_path: &Path, redo: bool) -> bool {
    already_processed(audio_path, redo)
}

fn already_processed(audio_path: &Path, redo: bool) -> bool {
    !redo && audio_path.with_extension("jsonl").exists()
}

fn journal_relative_candidate(audio_path: &Path, journal_path: &Path) -> PathBuf {
    if audio_path.is_absolute() {
        let stripped = audio_path
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(part) => Some(part),
                _ => None,
            })
            .collect::<PathBuf>();
        return journal_path.join(stripped);
    }
    let first = audio_path
        .components()
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(part) => part.to_str(),
            _ => None,
        });
    if first.is_some_and(is_day_name) {
        journal_path.join("chronicle").join(audio_path)
    } else {
        journal_path.join(audio_path)
    }
}

fn is_day_name(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_segment_directory(value: &str) -> bool {
    let Some((time, length)) = value.split_once('_') else {
        return false;
    };
    if time.len() != 6
        || length.is_empty()
        || !time.bytes().all(|byte| byte.is_ascii_digit())
        || !length.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let hour = time[0..2].parse::<u8>().ok();
    let minute = time[2..4].parse::<u8>().ok();
    let second = time[4..6].parse::<u8>().ok();
    matches!(
        (hour, minute, second),
        (Some(0..=23), Some(0..=59), Some(0..=59))
    )
}

pub(crate) fn installation_error(detail: impl Into<String>) -> CliError {
    CliError::SpeakersInstallation {
        message: format!(
            "Speakers-analyze installation is incomplete ({}). Repair: {}.",
            detail.into(),
            speakers_analyze_repair_text()
        ),
    }
}

fn usage(message: &str) -> CliError {
    CliError::Usage {
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        CliError, check_speakers_analyze_installation_with, parse_arguments,
        preflight_arguments_with, require_solstone_with, resolve_single_audio_path_with,
        should_skip_batch_processed, should_skip_process_audio_processed,
        should_skip_process_one_processed, validate_selection,
    };
    use crate::TranscribeError;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn empty_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn supervisor_skip_override_bypasses_connectivity() {
        let result = require_solstone_with(
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn supervisor_spawned_without_listener_is_silent_tempfail() {
        let error = require_solstone_with(
            |name| (name == "SOL_SUPERVISOR_SPAWNED").then(|| "1".to_owned()),
            || false,
        )
        .unwrap_err();
        assert_eq!(error, CliError::SupervisorSpawnedUnavailable);
        assert_eq!(error.exit_code(), 75);
        assert_eq!(error.message(), None);
    }

    #[test]
    fn unavailable_interactive_supervisor_has_exact_message() {
        let error = require_solstone_with(empty_env, || false).unwrap_err();
        assert_eq!(error, CliError::SupervisorUnavailable);
        assert_eq!(error.exit_code(), 1);
        assert_eq!(
            error.message(),
            Some("journal isn't running. start it with 'journal up' and retry.")
        );
    }

    #[test]
    fn supervisor_preflight_precedes_selection_validation() {
        let error = preflight_arguments_with(args(&["--all", "clip.wav"]), empty_env, || false)
            .unwrap_err();
        assert_eq!(error, CliError::SupervisorUnavailable);
    }

    #[test]
    fn mutually_exclusive_all_and_audio_path_is_usage_error() {
        let parsed = parse_arguments(args(&["--all", "clip.wav"])).unwrap();
        assert_eq!(
            validate_selection(&parsed).unwrap_err(),
            CliError::Usage {
                message: "--all and audio_path are mutually exclusive".to_owned()
            }
        );
    }

    #[test]
    fn missing_all_and_audio_path_is_usage_error() {
        let parsed = parse_arguments(Vec::<String>::new()).unwrap();
        assert_eq!(
            validate_selection(&parsed).unwrap_err(),
            CliError::Usage {
                message: "provide audio_path or --all".to_owned()
            }
        );
    }

    #[test]
    fn invalid_backend_is_usage_error() {
        assert_eq!(
            parse_arguments(args(&["--backend", "not-a-backend", "--all"])).unwrap_err(),
            CliError::Usage {
                message: "argument --backend: invalid choice: 'not-a-backend' (choose from parakeet, parakeet-cpp, confidential)".to_owned()
            }
        );
    }

    #[test]
    fn unsupported_audio_format_is_usage_error() {
        let audio = Path::new("/journal/chronicle/20260810/stream/120000_60/clip.txt");
        let error =
            resolve_single_audio_path_with(audio, Path::new("/journal"), |path| path == audio)
                .unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert_eq!(
            error.message(),
            Some(
                "Unsupported audio format: .txt. Supported formats: .flac, .m4a, .mp3, .ogg, .opus, .wav"
            )
        );
    }

    #[test]
    fn wrong_parent_directory_is_usage_error() {
        let audio = Path::new("/journal/chronicle/20260810/stream/not-a-segment/clip.wav");
        let error =
            resolve_single_audio_path_with(audio, Path::new("/journal"), |path| path == audio)
                .unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert_eq!(
            error.message(),
            Some(
                "Audio file must be in a segment directory (HHMMSS_LEN/), but parent is: not-a-segment"
            )
        );
    }

    #[test]
    fn missing_audio_path_has_exact_absolute_and_journal_relative_message() {
        let audio = Path::new("clip.wav");
        let journal = Path::new("/journal");
        let error = resolve_single_audio_path_with(audio, journal, |_| false).unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert_eq!(
            error.message(),
            Some(
                "Audio file not found.\n  Tried absolute:         clip.wav\n  Tried journal-relative: /journal/clip.wav"
            )
        );
    }

    #[test]
    fn batch_and_per_file_processed_guards_are_independent() {
        let temporary = tempfile::tempdir().unwrap();
        let audio = temporary.path().join("120000_60.wav");
        fs::write(audio.with_extension("jsonl"), b"done").unwrap();
        assert!(should_skip_batch_processed(&audio, false));
        assert!(should_skip_process_one_processed(&audio, false));
        assert!(should_skip_process_audio_processed(&audio, false));
        assert!(!should_skip_batch_processed(&audio, true));
        assert!(!should_skip_process_one_processed(&audio, true));
        assert!(!should_skip_process_audio_processed(&audio, true));
    }

    #[test]
    fn installation_failure_is_exit_78() {
        let error = check_speakers_analyze_installation_with(
            || Err("helper-missing: /bin/helper".to_owned()),
            |_| -> Result<PathBuf, TranscribeError> { unreachable!() },
        )
        .unwrap_err();
        assert_eq!(error.exit_code(), 78);
        assert!(error.message().unwrap().contains("helper-missing"));
    }
}
