// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
#[cfg(unix)]
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::thread;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use solstone_core_cogitate::{AccessTierError, classify_command};
#[cfg(unix)]
use solstone_core_system::lifecycle::{RunId, hosted_child_launch_provenance};
#[cfg(unix)]
use solstone_core_system::process::{
    BoxedTerminateFn, CommandLaunchRequest, Disposition, LaunchAuthority, LaunchError,
    launch_command, launch_command_hosted,
};

use crate::{BudgetExhaustedEvent, SlotLease, SlotReacquireError, SolCallBudget};

pub const SHELL_STDOUT_CAP: usize = 6000;
pub const SHELL_STDERR_CAP: usize = 6000;
pub const SHELL_TIMEOUT_SECONDS: u64 = 30;
const TRUNCATION_MARKER: &str = "\n... [truncated]";
const BUDGET_EXHAUSTED_TEXT: &str = "tool_budget_exhausted: read-call budget exceeded";
const REACQUIRE_CANCELLED_TEXT: &str =
    "local_admission_cancelled: cogitate run interrupted before reacquiring local inference";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolObservation {
    pub text: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolToolResult {
    pub observation: SolObservation,
    pub budget_exhausted_event: Option<BudgetExhaustedEvent>,
}

/// Run a command already accepted by cogitate policy directly, without a shell.
pub fn run_sol_command(
    command: &str,
    access_tier: &str,
    outbound_approval: Option<&str>,
    journal_root: &Path,
    budget: &mut SolCallBudget,
    slot: &mut dyn SlotLease,
) -> Result<SolToolResult, AccessTierError> {
    let decision = classify_command(command, access_tier, outbound_approval)?;
    if !decision.allowed {
        return Ok(result(decision.reason, true, None));
    }

    let budget_exhausted_event = budget.charge();
    if budget.exhausted() {
        return Ok(result(
            BUDGET_EXHAUSTED_TEXT.to_owned(),
            true,
            budget_exhausted_event,
        ));
    }

    let argv = decision.argv.expect("allowed decisions retain argv");
    Ok(orchestrate_slot_cycle(slot, || {
        run_command(&argv, journal_root)
    }))
}

pub(crate) fn orchestrate_slot_cycle(
    slot: &mut dyn SlotLease,
    run: impl FnOnce() -> Result<SolObservation, String>,
) -> SolToolResult {
    slot.yield_slot();
    let command_result = run();
    match slot.reacquire() {
        Err(SlotReacquireError::Cancelled) => match command_result {
            Ok(observation) => SolToolResult {
                observation,
                budget_exhausted_event: None,
            },
            Err(_) => result(REACQUIRE_CANCELLED_TEXT.to_owned(), true, None),
        },
        Err(SlotReacquireError::Other(error)) => {
            // The runtime wave owns interrupting the active conversation here.
            result(error, true, None)
        }
        Ok(()) => match command_result {
            Ok(observation) => SolToolResult {
                observation,
                budget_exhausted_event: None,
            },
            Err(error) => result(error, true, None),
        },
    }
}

fn result(
    text: String,
    is_error: bool,
    budget_exhausted_event: Option<BudgetExhaustedEvent>,
) -> SolToolResult {
    SolToolResult {
        observation: SolObservation { text, is_error },
        budget_exhausted_event,
    }
}

/// Execute argv with the same lookup and observation behavior as the Python sol tool.
pub fn run_command(argv: &[String], journal_root: &Path) -> Result<SolObservation, String> {
    run_command_with_timeout(
        argv,
        journal_root,
        Duration::from_secs(SHELL_TIMEOUT_SECONDS),
    )
}

#[cfg(unix)]
fn run_command_with_timeout(
    argv: &[String],
    journal_root: &Path,
    timeout: Duration,
) -> Result<SolObservation, String> {
    let Some(name) = argv.first() else {
        return Err("command_not_found: ".to_owned());
    };
    let Some(executable) = resolve_executable(name) else {
        return Ok(SolObservation {
            text: format!("command_not_found: {name}"),
            is_error: true,
        });
    };
    let command = CommandLaunchRequest {
        program: executable.into_os_string(),
        arguments: argv[1..].iter().map(Into::into).collect(),
        environment: Default::default(),
        current_dir: Some(journal_root.to_path_buf()),
        process_group: true,
        stdin_piped: false,
        stdout_piped: true,
        stderr_piped: true,
    };
    let child = ProcessGroupChild::spawn(command, timeout);
    let mut child = match child {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SolObservation {
                text: format!("command_not_found: {name}"),
                is_error: true,
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Ok(SolObservation {
                text: format!("permission_denied: {error}"),
                is_error: true,
            });
        }
        Err(error) => {
            return Ok(SolObservation {
                text: error.to_string(),
                is_error: true,
            });
        }
    };

    let stdout = child.take_stdout().expect("stdout was piped");
    let stderr = child.take_stderr().expect("stderr was piped");
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));
    let deadline = Instant::now() + timeout;
    let outcome = loop {
        match child.exited_without_reaping() {
            Ok(true) => break child.finish().map(|status| (status, false)),
            Ok(false) if Instant::now() >= deadline => {
                break child.finish().map(|status| (status, true));
            }
            Ok(false) => thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                let cleanup = child.finish();
                let cleanup = cleanup.err().map(|cleanup| cleanup.to_string());
                break Err(std::io::Error::other(match cleanup {
                    Some(cleanup) => format!("{error}; cleanup failed: {cleanup}"),
                    None => error.to_string(),
                }));
            }
        }
    };
    let stdout = String::from_utf8_lossy(&stdout_reader.join().expect("stdout reader panicked"))
        .into_owned();
    let stderr = String::from_utf8_lossy(&stderr_reader.join().expect("stderr reader panicked"))
        .into_owned();
    let (status, timed_out) = outcome.map_err(|error| error.to_string())?;
    // signal_aware_exit_code is non-negative iff ExitStatus::code() was Some.
    let text = format_shell_output(&stdout, &stderr, (status >= 0).then_some(status), timed_out);
    Ok(SolObservation {
        is_error: timed_out || status != 0,
        text,
    })
}

#[cfg(not(unix))]
fn run_command_with_timeout(
    _argv: &[String],
    _journal_root: &Path,
    _timeout: Duration,
) -> Result<SolObservation, String> {
    Err("process capability unavailable on this platform".to_owned())
}

#[cfg(unix)]
struct ProcessGroupChild {
    authority: LaunchAuthority,
    group: rustix::process::Pid,
    timeout: Duration,
}

#[cfg(unix)]
impl ProcessGroupChild {
    fn spawn(command: CommandLaunchRequest, timeout: Duration) -> std::io::Result<Self> {
        // Journal owns native same-device operations and acknowledges admission
        // at entry. Give it a distinct descendant identity rather than letting
        // it inherit this talent's acknowledgement. Other allowed tools do not
        // implement the hosted-child protocol and retain the bounded group path.
        let provenance = if Path::new(&command.program)
            .file_name()
            .is_some_and(|name| name == "journal")
        {
            let id = RunId::generate().map_err(std::io::Error::other)?;
            hosted_child_launch_provenance(
                format!("journal-tool-{id}"),
                timeout.min(Duration::from_secs(3)),
            )
            .map_err(std::io::Error::other)?
        } else {
            None
        };
        let terminate: BoxedTerminateFn = Box::new(|child, _timeout| {
            let Some(group) = i32::try_from(child.id())
                .ok()
                .and_then(rustix::process::Pid::from_raw)
            else {
                return child.kill().map_err(LaunchError::Terminate);
            };
            match rustix::process::kill_process_group(group, rustix::process::Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
                Err(error) => Err(LaunchError::Terminate(std::io::Error::from(error))),
            }
        });
        let disposition = Disposition::IndependentBoundedHelper { timeout };
        let authority = match provenance {
            Some(provenance) => launch_command_hosted(disposition, command, provenance, terminate),
            None => launch_command(disposition, command, terminate),
        }
        .map_err(|error| match error {
            LaunchError::Spawn(inner) => inner,
            other => std::io::Error::other(other),
        })?;
        let Some(group) = i32::try_from(authority.pid())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "command child PID does not fit a process-group ID",
            ));
        };
        Ok(Self {
            authority,
            group,
            timeout,
        })
    }

    fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.authority.take_stdout()
    }

    fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.authority.take_stderr()
    }

    fn exited_without_reaping(&self) -> std::io::Result<bool> {
        rustix::process::waitid(
            rustix::process::WaitId::Pid(self.group),
            rustix::process::WaitIdOptions::EXITED
                | rustix::process::WaitIdOptions::NOHANG
                | rustix::process::WaitIdOptions::NOWAIT,
        )
        .map(|status| status.is_some())
        .map_err(std::io::Error::from)
    }

    fn finish(mut self) -> std::io::Result<i32> {
        let _ = self.authority.terminate(self.timeout);
        self.authority.wait()
    }
}

#[cfg(unix)]
fn read_all(mut stream: impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = stream.read_to_end(&mut bytes);
    bytes
}

fn resolve_executable(name: &str) -> Option<PathBuf> {
    let executable_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let paths = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    resolve_executable_in(name, executable_dir.as_deref(), &paths)
}

// Python checks only existence beside its executable; PATH fallback remains a
// simple existence walk because no oracle vector constrains executable-bit fidelity.
fn resolve_executable_in(
    name: &str,
    executable_dir: Option<&Path>,
    paths: &[PathBuf],
) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.is_absolute() && candidate.exists() {
        return Some(candidate.to_path_buf());
    }
    if let Some(path) = executable_dir
        .map(|directory| directory.join(name))
        .filter(|path| path.exists())
    {
        return Some(path);
    }
    paths
        .iter()
        .map(|directory| directory.join(name))
        .find(|path| path.exists())
}

pub fn format_shell_output(
    stdout: &str,
    stderr: &str,
    returncode: Option<i32>,
    timed_out: bool,
) -> String {
    let mut parts = Vec::new();
    if !stdout.is_empty() {
        parts.push(format!("stdout:\n{}", format_stdout(stdout)));
    }
    if !stderr.is_empty() {
        parts.push(format!(
            "stderr:\n{}",
            truncate_output(stderr, SHELL_STDERR_CAP)
        ));
    }
    if timed_out {
        parts.push(format!(
            "timeout: command exceeded {SHELL_TIMEOUT_SECONDS}s"
        ));
    } else if let Some(code) = returncode.filter(|code| *code != 0) {
        parts.push(format!("exit_code: {code}"));
    }
    if parts.is_empty() {
        "ok".to_owned()
    } else {
        parts.join("\n\n")
    }
}

pub fn truncate_output(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_owned();
    }
    format!(
        "{}{TRUNCATION_MARKER}",
        text.chars().take(cap).collect::<String>()
    )
}

fn format_stdout(text: &str) -> String {
    if text.chars().count() <= SHELL_STDOUT_CAP {
        return text.to_owned();
    }
    let Ok(raw) = serde_json::from_str::<&serde_json::value::RawValue>(text) else {
        return truncate_output(text, SHELL_STDOUT_CAP);
    };
    // Strip only JSON whitespace outside strings. Preserve number spellings,
    // duplicate keys, escapes and ordering instead of decoding through Value.
    let mut compact = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut count = 0;
    for ch in raw.get().chars() {
        if !quoted && matches!(ch, ' ' | '\n' | '\r' | '\t') {
            continue;
        }
        count += 1;
        if count > SHELL_STDOUT_CAP {
            return "output_omitted: the complete JSON response exceeds the tool output limit. No response data is shown. For a read, narrow the query or date range and request fewer results. Do not repeat a write because its output was omitted.".to_owned();
        }
        compact.push(ch);
        if escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        }
    }
    compact
}

#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub mod test_hooks {
    use std::path::Path;
    use std::time::Duration;

    use super::{SolObservation, run_command_with_timeout};

    pub fn run_with_timeout(
        argv: &[String],
        journal_root: &Path,
        timeout: Duration,
    ) -> Result<SolObservation, String> {
        run_command_with_timeout(argv, journal_root, timeout)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn executable_beside_current_binary_wins_over_path() {
        let root = unique_temp_dir("resolution");
        let sibling = root.path().join("sibling");
        let path = root.path().join("path");
        fs::create_dir_all(&sibling).expect("sibling directory");
        fs::create_dir_all(&path).expect("path directory");
        fs::write(sibling.join("solstone"), "sibling").expect("sibling fixture");
        fs::write(path.join("solstone"), "path").expect("path fixture");
        assert_eq!(
            resolve_executable_in("solstone", Some(&sibling), &[path]),
            Some(sibling.join("solstone"))
        );
    }

    #[test]
    fn stale_sol_executable_name_is_not_resolved() {
        let root = unique_temp_dir("stale-sol");
        let sibling = root.path().join("sibling");
        let path = root.path().join("path");
        fs::create_dir_all(&sibling).expect("sibling directory");
        fs::create_dir_all(&path).expect("path directory");
        fs::write(sibling.join("solstone"), "sibling").expect("current sibling");
        fs::write(path.join("sol"), "stale-path-sol").expect("stale path sol");
        assert_eq!(
            resolve_executable_in("solstone", Some(&sibling), std::slice::from_ref(&path)),
            Some(sibling.join("solstone"))
        );
        assert_eq!(
            resolve_executable_in("sol", Some(&sibling), &[]),
            None,
            "resolving sol as an executable name is command_not_found equivalent"
        );
    }

    #[test]
    fn shell_output_truncates_each_stream_independently_at_rust_chars() {
        let stdout = "x".repeat(SHELL_STDOUT_CAP + 1);
        let stderr = "é".repeat(SHELL_STDERR_CAP + 1);
        let actual = format_shell_output(&stdout, &stderr, Some(7), false);
        assert_eq!(
            actual,
            format!(
                "stdout:\n{}\n... [truncated]\n\nstderr:\n{}\n... [truncated]\n\nexit_code: 7",
                "x".repeat(SHELL_STDOUT_CAP),
                "é".repeat(SHELL_STDERR_CAP)
            )
        );
    }

    #[test]
    fn structured_output_keeps_complete_records_when_compaction_fits() {
        let value = serde_json::json!({
            "days": [{"results": (0..80).map(|id| serde_json::json!({
                "entry_id": id, "text": "dated evidence", "idx": id
            })).collect::<Vec<_>>() }],
            "tail": "last field must survive"
        });
        let pretty = serde_json::to_string_pretty(&value).unwrap();
        assert!(pretty.chars().count() > SHELL_STDOUT_CAP);
        let actual = format_shell_output(&pretty, "", Some(0), false);
        let body = actual.strip_prefix("stdout:\n").unwrap();
        assert!(body.chars().count() <= SHELL_STDOUT_CAP);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            value
        );
    }

    #[test]
    fn oversized_structured_output_is_omitted_without_partial_evidence() {
        let raw = serde_json::json!({"claim": "unsupported-prefix", "body": "x".repeat(7000)})
            .to_string();
        let actual = format_shell_output(&raw, "native failure", Some(7), false);
        assert!(actual.contains("output_omitted"));
        assert!(!actual.contains("unsupported-prefix"));
        assert!(!actual.contains("[truncated]"));
        assert!(actual.ends_with("stderr:\nnative failure\n\nexit_code: 7"));
    }

    #[test]
    fn structured_compaction_preserves_literals_escapes_and_duplicate_keys() {
        let raw = r#"{ "n": 1e999, "n": 900719925474099312345, "s": "é  \"quoted\" \\ \n" }"#;
        let padded = format!("{}{raw}", "\n".repeat(SHELL_STDOUT_CAP));
        assert_eq!(
            format_stdout(&padded),
            r#"{"n":1e999,"n":900719925474099312345,"s":"é  \"quoted\" \\ \n"}"#
        );
        let exact = serde_json::json!({"a": "é".repeat(SHELL_STDOUT_CAP - 8)}).to_string();
        assert_eq!(exact.chars().count(), SHELL_STDOUT_CAP);
        assert_eq!(format_stdout(&format!("\n{exact}")), exact);
    }

    fn unique_temp_dir(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("solstone-cogitate-tools-{name}-"))
            .tempdir()
            .expect("tempdir")
    }
}
