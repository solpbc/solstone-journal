// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use super::service_status;
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use std::{
    io,
    os::unix::process::CommandExt,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct ProcessGroupChild {
    child: Option<Child>,
    group: rustix::process::Pid,
}

impl ProcessGroupChild {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        command.process_group(0);
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let group = match i32::try_from(child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            Some(pid) => pid,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "probe child PID does not fit a process-group ID",
                ));
            }
        };
        Ok(Self {
            child: Some(child),
            group,
        })
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
        self.terminate_group();
        self.child
            .take()
            .expect("owned probe child")
            .wait_with_output()
    }

    fn terminate_group(&mut self) {
        if rustix::process::kill_process_group(self.group, rustix::process::Signal::KILL).is_err()
            && let Some(child) = self.child.as_mut()
        {
            let _ = child.kill();
        }
    }
}

impl Drop for ProcessGroupChild {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.terminate_group();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }
}

fn installed(context: &CheckContext) -> bool {
    match context.platform {
        crate::vocabulary::Platform::Darwin => context
            .home_dir
            .join("Library/LaunchAgents/org.solpbc.solstone.plist")
            .exists(),
        crate::vocabulary::Platform::Linux => context
            .home_dir
            .join(".config/systemd/user/solstone.service")
            .exists(),
    }
}
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if !installed(context) {
        return Ok(make_result(
            check,
            Status::Skip,
            "no local journal service",
            None::<String>,
        ));
    }
    let Some(status) = service_status::fetch(context) else {
        if service_is_failed(context) {
            return Ok(make_result(
                check,
                Status::Fail,
                "journal service unit is failed",
                Some("run journal service restart; if it persists, run journal service logs"),
            ));
        }
        return Ok(make_result(
            check,
            Status::Warn,
            "service installed but not running",
            Some("run journal service start"),
        ));
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
                format!("{name} ({attempts} restart attempts)")
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(make_result(
            check,
            Status::Fail,
            format!("crash-loop: {items}"),
            Some("run journal service logs"),
        ));
    }
    Ok(make_result(
        check,
        Status::Ok,
        "journal service is running",
        None::<String>,
    ))
}

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
    }
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> Option<Output> {
    let child = ProcessGroupChild::spawn(command).ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.exited_without_reaping().ok()? || Instant::now() >= deadline {
            return child.terminate_with_output().ok();
        }
        thread::sleep(Duration::from_millis(10));
    }
}
