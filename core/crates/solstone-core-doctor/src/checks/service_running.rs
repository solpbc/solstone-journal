// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::service_status;
use crate::{
    context::{CheckContext, WindowsServiceRegistration},
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
#[cfg(unix)]
use solstone_core_system::process::{Disposition, LaunchAuthority, LaunchError, launch};
#[cfg(unix)]
use std::{
    io,
    os::unix::process::CommandExt,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Bounded diagnostic child for a service-manager query.
#[cfg(unix)]
struct ProcessGroupChild {
    authority: LaunchAuthority,
    group: rustix::process::Pid,
}

/// The inspected journal service stays externally managed; only this bounded
/// probe child is claimed.
#[cfg(unix)]
fn diagnostic_disposition(timeout: Duration) -> Disposition {
    Disposition::IndependentBoundedHelper { timeout }
}

#[cfg(unix)]
impl ProcessGroupChild {
    fn spawn(command: &mut Command, timeout: Duration) -> io::Result<Self> {
        let authority = launch(
            diagnostic_disposition(timeout),
            || {
                command
                    .process_group(0)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
            },
            Box::new(|child, _timeout| {
                let Some(group) = i32::try_from(child.id())
                    .ok()
                    .and_then(rustix::process::Pid::from_raw)
                else {
                    return child.kill().map_err(LaunchError::Terminate);
                };
                if rustix::process::kill_process_group(group, rustix::process::Signal::KILL)
                    .is_err()
                {
                    return child.kill().map_err(LaunchError::Terminate);
                }
                Ok(())
            }),
        )
        .map_err(|error| match error {
            LaunchError::Spawn(inner) => inner,
            other => io::Error::other(other),
        })?;
        let Some(group) = i32::try_from(authority.pid())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                copy::text("journal.cli.doctor.service.running.pid.invalid"),
            ));
        };
        Ok(Self { authority, group })
    }

    fn exited_without_reaping(&self) -> io::Result<bool> {
        rustix::process::waitid(
            rustix::process::WaitId::Pid(self.group),
            rustix::process::WaitIdOptions::EXITED
                | rustix::process::WaitIdOptions::NOHANG
                | rustix::process::WaitIdOptions::NOWAIT,
        )
        .map(|status| status.is_some())
        .map_err(io::Error::from)
    }

    fn terminate_with_output(mut self) -> io::Result<Output> {
        let _ = self.authority.terminate(Duration::from_secs(2));
        self.authority.wait_with_output().map_err(io::Error::other)
    }
}

/// Whether a journal service is registered for this installation.
enum Installed {
    Yes,
    No,
    Unknown(String),
}

fn installed(context: &CheckContext) -> Installed {
    let present = match context.platform {
        crate::vocabulary::Platform::Darwin => context
            .home_dir
            .join("Library/LaunchAgents/org.solpbc.solstone.plist")
            .exists(),
        crate::vocabulary::Platform::Linux => context
            .home_dir
            .join(".config/systemd/user/solstone.service")
            .exists(),
        crate::vocabulary::Platform::Windows => {
            return match windows_registration(context) {
                WindowsServiceRegistration::Absent => Installed::No,
                WindowsServiceRegistration::Present { .. } => Installed::Yes,
                WindowsServiceRegistration::Unreadable(reason) => Installed::Unknown(reason),
            };
        }
    };
    if present {
        Installed::Yes
    } else {
        Installed::No
    }
}

/// The Windows service registration, read through the journal command's own
/// Task Scheduler client.
pub(crate) fn windows_registration(context: &CheckContext) -> WindowsServiceRegistration {
    match context.windows_service_probe {
        Some(probe) => probe(context),
        None => WindowsServiceRegistration::Unreadable(copy::text(
            "journal.cli.doctor.service.running.registration.no.reader",
        )),
    }
}

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    match installed(context) {
        Installed::Yes => {}
        Installed::No => {
            return Ok(make_result(
                check,
                Status::Skip,
                copy::text("journal.cli.doctor.service.absent"),
                None::<String>,
            ));
        }
        Installed::Unknown(reason) => {
            return Ok(make_result(
                check,
                Status::Skip,
                copy::display(
                    "journal.cli.doctor.service.registration.unreadable",
                    [("reason", &reason)],
                ),
                None::<String>,
            ));
        }
    }
    let status = match service_status::fetch(context) {
        Ok(status) => status,
        Err(cause) => {
            // A failed unit is a loud, established fact whatever the probe did,
            // so it keeps precedence over every no-status cause.
            if service_is_failed(context) {
                return Ok(make_result(
                    check,
                    Status::Fail,
                    copy::text("journal.cli.doctor.service.running.unit.failed"),
                    Some(copy::text("journal.cli.doctor.service.running.restart")),
                )
                .changing_service());
            }
            // ⛔ Only an absent socket establishes that nothing is running.
            // The other causes mean the probe could not tell, and saying "not
            // running" about a live service is worse than saying nothing.
            if cause == service_status::Unavailable::NoSocket {
                return Ok(make_result(
                    check,
                    Status::Warn,
                    copy::text("journal.cli.doctor.service.running.stopped"),
                    Some(copy::text("journal.cli.doctor.service.running.start")),
                )
                .changing_service());
            }
            return Ok(make_result(
                check,
                Status::Skip,
                // ⛔ "supervisor" is a backstage process name; the owner-facing
                // surfaces call this the system manager.
                copy::display(
                    "journal.cli.doctor.service.running.unreachable",
                    [("cause", &cause.key())],
                ),
                None::<String>,
            ));
        }
    };
    let crashed = status
        .get("crashed")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !crashed.is_empty() {
        let items = crashed
            .iter()
            .map(|value| {
                let name = value
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?");
                let attempts = value
                    .get("restart_attempts")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                context.report_form.counted(
                    "journal.cli.doctor.service.restart.attempts",
                    &[("quantity", attempts.max(0) as u64)],
                    &[("name", &name), ("attempts", &attempts)],
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(make_result(
            check,
            Status::Fail,
            copy::strings(
                "journal.cli.doctor.service.crash.list",
                [("services", &items)],
            ),
            Some(copy::text("journal.cli.doctor.service.running.logs")),
        ));
    }
    Ok(make_result(
        check,
        Status::Ok,
        copy::text("journal.cli.doctor.service.running.running"),
        None::<String>,
    ))
}

/// A Windows task has no "failed" state distinct from not running: the
/// Scheduler reports only its last result, which a clean stop also sets.
#[cfg(not(unix))]
fn service_is_failed(_context: &CheckContext) -> bool {
    false
}

#[cfg(unix)]
fn service_is_failed(context: &CheckContext) -> bool {
    let mut command = if let Some((program, args)) = &context.service_status_command_override {
        let mut command = Command::new(program);
        command.args(args);
        command
    } else {
        match context.platform {
            crate::vocabulary::Platform::Darwin => {
                let mut command = Command::new("launchctl");
                command.args([
                    "print",
                    &format!("gui/{}/org.solpbc.solstone", nix::unistd::Uid::effective()),
                ]);
                command
            }
            crate::vocabulary::Platform::Linux => {
                let mut command = Command::new("systemctl");
                command.args(["--user", "is-failed", "solstone"]);
                command
            }
            crate::vocabulary::Platform::Windows => return false,
        }
    };
    let Some(output) = run_with_timeout(&mut command, Duration::from_secs(2)) else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    match context.platform {
        crate::vocabulary::Platform::Darwin => {
            stdout.contains("\n\tstate = crashed\n") || stdout.contains("state = crashed")
        }
        crate::vocabulary::Platform::Linux => stdout.trim() == "failed",
        crate::vocabulary::Platform::Windows => false,
    }
}

#[cfg(unix)]
fn run_with_timeout(command: &mut Command, timeout: Duration) -> Option<Output> {
    let child = ProcessGroupChild::spawn(command, timeout).ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.exited_without_reaping().ok()? || Instant::now() >= deadline {
            return child.terminate_with_output().ok();
        }
        thread::sleep(Duration::from_millis(10));
    }
}
