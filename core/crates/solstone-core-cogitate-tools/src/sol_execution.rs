// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_cogitate::{AccessTierError, classify_command};

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
    let child = Command::new(executable)
        .args(&argv[1..])
        .current_dir(journal_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
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

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));
    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                break (child.wait().map_err(|error| error.to_string())?, true);
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(error.to_string()),
        }
    };
    let stdout = String::from_utf8_lossy(&stdout_reader.join().expect("stdout reader panicked"))
        .into_owned();
    let stderr = String::from_utf8_lossy(&stderr_reader.join().expect("stderr reader panicked"))
        .into_owned();
    let text = format_shell_output(&stdout, &stderr, status.code(), timed_out);
    Ok(SolObservation {
        is_error: timed_out || !status.success(),
        text,
    })
}

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
        parts.push(format!(
            "stdout:\n{}",
            truncate_output(stdout, SHELL_STDOUT_CAP)
        ));
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn executable_beside_current_binary_wins_over_path() {
        let root = unique_temp_dir("resolution");
        let sibling = root.join("sibling");
        let path = root.join("path");
        fs::create_dir_all(&sibling).expect("sibling directory");
        fs::create_dir_all(&path).expect("path directory");
        fs::write(sibling.join("sol"), "sibling").expect("sibling fixture");
        fs::write(path.join("sol"), "path").expect("path fixture");
        assert_eq!(
            resolve_executable_in("sol", Some(&sibling), &[path]),
            Some(sibling.join("sol"))
        );
        fs::remove_dir_all(root).expect("remove fixtures");
    }

    #[test]
    fn timeout_preserves_partial_output() {
        let argv = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "printf partial; printf error >&2; sleep 1".to_owned(),
        ];
        let actual = run_command_with_timeout(&argv, Path::new("."), Duration::from_millis(50))
            .expect("command handling");
        assert!(actual.is_error);
        assert_eq!(
            actual.text,
            "stdout:\npartial\n\nstderr:\nerror\n\ntimeout: command exceeded 30s"
        );
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        env::temp_dir().join(format!("solstone-cogitate-tools-{name}-{stamp}"))
    }
}
