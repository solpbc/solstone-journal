// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded launch probe for the sibling VAD helper.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use solstone_core_system::process::{Disposition, LaunchError, launch};
use solstone_core_system_health::{
    BoundedStderr, classify_loader_failure, read_bounded_stderr,
    sanitize_os_bytes_for_terminal_bounded, unresolved_library,
};

use crate::audio::vad_binary_candidate_from;

const ERROR_SCHEMA: &str = "solstone-vad-error-v1";
const MALFORMED_REQUEST: &str = "malformed-request";
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Closed-stdin probe budget. Measured 2026-08-23 on this host:
/// source-checkout max 1.203ms, installed-package max 1.809ms (14 timed runs,
/// both exit 64 + solstone-vad-error-v1 malformed-request, no loader override).
/// `clamp(max * 4, 1s, 10s)` => 1s.
pub const VAD_RUNTIME_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Distinct launch outcomes for the VAD helper. None of these is a missing
/// provider, missing model, or unready Parakeet server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VadRuntimeStatus {
    Ready,
    Missing {
        path: PathBuf,
    },
    Loader {
        path: PathBuf,
        library: String,
        stderr: String,
    },
    Timeout {
        path: PathBuf,
        timeout: Duration,
        pid: u32,
    },
    Spawn {
        path: PathBuf,
        cause: String,
    },
    Contract {
        path: PathBuf,
        exit_code: Option<i32>,
        stderr: String,
    },
    /// The helper path could not be resolved (no executable, or no parent directory).
    Unresolved {
        cause: String,
    },
}

/// Probe using transcription's resolver from an executable path.
pub fn probe_from_executable(
    current_executable: Result<PathBuf, io::Error>,
    timeout: Duration,
) -> VadRuntimeStatus {
    match vad_binary_candidate_from(current_executable, |name| std::env::var(name).ok()) {
        Ok(path) => probe_vad_runtime(&path, timeout),
        Err(error) => VadRuntimeStatus::Unresolved {
            cause: error.to_string(),
        },
    }
}

/// Run the closed-stdin launch probe against one helper path.
pub fn probe_vad_runtime(binary: &Path, timeout: Duration) -> VadRuntimeStatus {
    let path = binary.to_path_buf();
    if !binary.is_file() {
        return VadRuntimeStatus::Missing { path };
    }
    let mut authority = match launch(
        Disposition::IndependentBoundedHelper { timeout },
        || {
            Command::new(binary)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        },
        Box::new(|child, _timeout| child.kill().map_err(LaunchError::Terminate)),
    ) {
        Ok(authority) => authority,
        Err(error) => {
            return VadRuntimeStatus::Spawn {
                path,
                cause: error.to_string(),
            };
        }
    };
    let pid = authority.pid();
    let stderr_reader = authority
        .take_stderr()
        .map(|pipe| thread::spawn(move || read_bounded_stderr(pipe)));
    let stdout_reader = authority
        .take_stdout()
        .map(|pipe| thread::spawn(move || read_bounded_stderr(pipe)));
    let start = Instant::now();
    let poll_code = loop {
        match authority.poll() {
            Ok(Some(code)) => break Some(code),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = authority.terminate(timeout);
                drop(join_bounded(stderr_reader));
                drop(join_bounded(stdout_reader));
                return VadRuntimeStatus::Timeout { path, timeout, pid };
            }
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(error) => {
                drop(join_bounded(stderr_reader));
                drop(join_bounded(stdout_reader));
                return VadRuntimeStatus::Spawn {
                    path,
                    cause: error.to_string(),
                };
            }
        }
    };
    let stderr = join_bounded(stderr_reader);
    let stdout = join_bounded(stdout_reader);
    let (exit_code, signal) = split_poll_code(poll_code);
    classify_finished(path, exit_code, signal, stdout, stderr)
}

/// Per-variant repair guidance. Distinct from Parakeet and speakers-analyze copy.
pub fn vad_runtime_repair_for(status: &VadRuntimeStatus) -> Option<&'static str> {
    match status {
        VadRuntimeStatus::Ready => None,
        VadRuntimeStatus::Missing { .. } => Some(
            "place solstone-core-vad-analyze beside solstone-core in the journal-host bindir, then rerun journal doctor",
        ),
        VadRuntimeStatus::Loader { .. } => Some(
            "restore the bundled ONNX runtime libraries for the VAD helper, then rerun journal doctor",
        ),
        VadRuntimeStatus::Timeout { .. } => Some(
            "stop any stuck solstone-core-vad-analyze process, reinstall the journal-host VAD helper, then rerun journal doctor",
        ),
        VadRuntimeStatus::Spawn { .. } => Some(
            "repair execute permission and format of solstone-core-vad-analyze, then rerun journal doctor",
        ),
        VadRuntimeStatus::Contract { .. } => Some(
            "reinstall the journal-host VAD helper so closed stdin reports solstone-vad-error-v1 malformed-request, then rerun journal doctor",
        ),
        VadRuntimeStatus::Unresolved { .. } => Some(
            "repair the journal-host install so the doctor can resolve solstone-core-vad-analyze beside solstone-core, then rerun journal doctor",
        ),
    }
}

pub fn status_detail(status: &VadRuntimeStatus) -> String {
    match status {
        VadRuntimeStatus::Ready => "VAD helper launchable (closed-stdin usage contract)".to_owned(),
        VadRuntimeStatus::Missing { path } => {
            format!("VAD helper binary is missing: {}", path.display())
        }
        VadRuntimeStatus::Loader { library, .. } => {
            format!("VAD helper could not load {library}")
        }
        VadRuntimeStatus::Timeout { timeout, .. } => {
            format!("VAD helper did not exit within {timeout:?}")
        }
        VadRuntimeStatus::Spawn { cause, .. } => {
            format!("VAD helper could not be executed: {cause}")
        }
        VadRuntimeStatus::Contract {
            exit_code, stderr, ..
        } => format!(
            "VAD helper ran but did not report closed-stdin usage contract (exit {exit_code:?}): {stderr}"
        ),
        VadRuntimeStatus::Unresolved { cause } => {
            format!("could not resolve the VAD helper path: {cause}")
        }
    }
}

fn classify_finished(
    path: PathBuf,
    exit_code: Option<i32>,
    signal: Option<i32>,
    stdout: BoundedStderr,
    stderr: BoundedStderr,
) -> VadRuntimeStatus {
    if closed_stdin_usage_contract(exit_code, &stdout.bytes, &stderr.bytes) {
        return VadRuntimeStatus::Ready;
    }
    let named = stderr
        .loader_library
        .clone()
        .or_else(|| stdout.loader_library.clone())
        .or_else(|| unresolved_library(&stderr.bytes))
        .or_else(|| unresolved_library(&stdout.bytes));
    if let Some(library) = classify_loader_failure(exit_code, signal, named.as_deref()) {
        return VadRuntimeStatus::Loader {
            path,
            library,
            stderr: sanitize_os_bytes_for_terminal_bounded(&stderr.bytes),
        };
    }
    VadRuntimeStatus::Contract {
        path,
        exit_code,
        stderr: sanitize_os_bytes_for_terminal_bounded(&stderr.bytes),
    }
}

fn closed_stdin_usage_contract(exit_code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> bool {
    exit_code == Some(64)
        && (reports_malformed_request(stderr) || reports_malformed_request(stdout))
}

fn reports_malformed_request(bytes: &[u8]) -> bool {
    let text = std::str::from_utf8(bytes).unwrap_or("");
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("schema").and_then(Value::as_str) == Some(ERROR_SCHEMA)
            && value.get("reason").and_then(Value::as_str) == Some(MALFORMED_REQUEST)
        {
            return true;
        }
    }
    text.contains(ERROR_SCHEMA) && text.contains(MALFORMED_REQUEST)
}

fn split_poll_code(code: Option<i32>) -> (Option<i32>, Option<i32>) {
    match code {
        Some(value) if value >= 0 => (Some(value), None),
        Some(value) => (None, Some(-value)),
        None => (None, None),
    }
}

fn join_bounded(reader: Option<thread::JoinHandle<io::Result<BoundedStderr>>>) -> BoundedStderr {
    reader
        .and_then(|handle| handle.join().ok())
        .and_then(Result::ok)
        .unwrap_or_default()
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use super::{
        VadRuntimeStatus, probe_from_executable, probe_vad_runtime, vad_runtime_repair_for,
    };

    fn write_stub(body: &str, mode: u32) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("vad-stub");
        let staging = root.path().join("vad-stub.staging");
        {
            let mut file = fs::File::create(&staging).unwrap();
            file.write_all(body.as_bytes()).unwrap();
            file.sync_all().unwrap();
        }
        let mut permissions = fs::metadata(&staging).unwrap().permissions();
        permissions.set_mode(mode);
        fs::set_permissions(&staging, permissions).unwrap();
        fs::rename(&staging, &path).unwrap();
        (root, path)
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }

    #[test]
    fn resolver_failure_is_unresolved_not_spawn() {
        let status = probe_from_executable(
            Err(std::io::Error::other("injected")),
            Duration::from_secs(1),
        );
        match &status {
            VadRuntimeStatus::Unresolved { cause } => {
                assert!(
                    cause.contains("could not determine current executable"),
                    "{cause}"
                );
                assert!(cause.contains("injected"), "{cause}");
            }
            other => panic!("expected Unresolved, got {other:?}"),
        }
        let repair = vad_runtime_repair_for(&status).expect("unresolved helper has repair text");
        assert!(repair.contains("solstone-core-vad-analyze"), "{repair}");
        assert!(repair.contains("journal doctor"), "{repair}");
        let rooted =
            probe_from_executable(Ok(std::path::PathBuf::from("/")), Duration::from_secs(1));
        match rooted {
            VadRuntimeStatus::Unresolved { cause } => {
                assert!(cause.contains("no parent directory"), "{cause}");
            }
            other => panic!("expected Unresolved for root path, got {other:?}"),
        }
    }

    #[test]
    fn missing_path_is_missing_not_spawn() {
        let path = std::path::PathBuf::from("/no/such/solstone-core-vad-analyze");
        let status = probe_vad_runtime(&path, Duration::from_secs(1));
        assert!(
            matches!(status, VadRuntimeStatus::Missing { .. }),
            "{status:?}"
        );
    }

    #[test]
    fn healthy_closed_stdin_contract_is_ready() {
        let (_root, path) = write_stub(
            "#!/bin/sh\nprintf '%s\\n' '{\"schema\":\"solstone-vad-error-v1\",\"reason\":\"malformed-request\",\"detail\":\"empty\"}'\nexit 64\n",
            0o755,
        );
        let status = probe_vad_runtime(&path, Duration::from_secs(2));
        assert!(matches!(status, VadRuntimeStatus::Ready), "{status:?}");
    }

    #[test]
    fn loader_stderr_is_loader() {
        let (_root, path) = write_stub(
            "#!/bin/sh\necho 'error while loading shared libraries: libonnxruntime.so.1: cannot open shared object file' >&2\nexit 127\n",
            0o755,
        );
        let status = probe_vad_runtime(&path, Duration::from_secs(2));
        match status {
            VadRuntimeStatus::Loader { library, .. } => {
                assert_eq!(library, "libonnxruntime.so.1");
            }
            other => panic!("expected Loader, got {other:?}"),
        }
    }

    #[test]
    fn eacces_is_spawn_with_the_real_cause() {
        let (_root, path) = write_stub("#!/bin/sh\nexit 64\n", 0o644);
        let status = probe_vad_runtime(&path, Duration::from_secs(2));
        match status {
            VadRuntimeStatus::Spawn { cause, .. } => {
                let lower = cause.to_lowercase();
                assert!(
                    lower.contains("permission")
                        || lower.contains("denied")
                        || cause.contains("13"),
                    "{cause}"
                );
            }
            other => panic!("expected Spawn, got {other:?}"),
        }
    }

    #[test]
    fn exec_format_is_spawn_with_the_real_cause() {
        let (_root, path) = write_stub("not-an-executable-format\x00\x01\x02", 0o755);
        let status = probe_vad_runtime(&path, Duration::from_secs(2));
        match status {
            VadRuntimeStatus::Spawn { cause, .. } => {
                let lower = cause.to_lowercase();
                assert!(
                    lower.contains("exec")
                        || lower.contains("format")
                        || lower.contains("invalid")
                        || cause.contains("8"),
                    "{cause}"
                );
            }
            other => panic!("expected Spawn, got {other:?}"),
        }
    }

    #[test]
    fn timeout_kills_the_helper_and_returns_within_budget() {
        let (_root, path) = write_stub("#!/bin/sh\nexec sleep 30\n", 0o755);
        let timeout = Duration::from_millis(80);
        let started = Instant::now();
        let status = probe_vad_runtime(&path, timeout);
        let elapsed = started.elapsed();
        match status {
            VadRuntimeStatus::Timeout { pid, .. } => {
                assert!(
                    !process_exists(pid),
                    "helper pid {pid} still exists after timeout"
                );
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
        assert!(
            elapsed <= timeout + Duration::from_millis(500),
            "probe took {elapsed:?}, budget {:?}",
            timeout + Duration::from_millis(500)
        );
    }

    #[test]
    fn unexpected_exit_is_contract() {
        let (_root, path) = write_stub("#!/bin/sh\necho ok\nexit 0\n", 0o755);
        let status = probe_vad_runtime(&path, Duration::from_secs(2));
        assert!(
            matches!(
                status,
                VadRuntimeStatus::Contract {
                    exit_code: Some(0),
                    ..
                }
            ),
            "{status:?}"
        );
    }
}
