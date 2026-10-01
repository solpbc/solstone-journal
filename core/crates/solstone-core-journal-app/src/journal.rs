// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The app drives the journal through the same `journal` command an owner
//! types, from the same install. Each call is its own process with no
//! console window, so the app holds no Task Scheduler state of its own.

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::setup_events::{SetupProgress, parse_line};
use crate::status::ServiceStatus;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `<root>\current\bin`, where this program and `journal.exe` both live.
pub fn bin_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

pub fn journal_exe() -> PathBuf {
    bin_dir().join("journal.exe")
}

fn command(args: &[&str]) -> Command {
    let mut command = Command::new(journal_exe());
    command
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    command
}

/// Run `journal <args>` to the end. `Err` carries the command's own last
/// line of explanation, which is written for the owner.
pub fn run(args: &[&str]) -> Result<String, String> {
    let output = command(args)
        .output()
        .map_err(|error| format!("couldn't run journal: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .rev()
        .chain(stdout.lines().rev())
        .map(|line| line.trim().trim_start_matches("journal: "))
        .find(|line| !line.is_empty())
        .unwrap_or("journal didn't say why")
        .to_owned();
    Err(reason)
}

pub fn status() -> Result<ServiceStatus, String> {
    ServiceStatus::parse(&run(&["service", "__app-status"])?)
}

pub fn start() -> Result<(), String> {
    run(&["service", "start"]).map(drop)
}

pub fn stop() -> Result<(), String> {
    run(&["service", "stop"]).map(drop)
}

pub fn restart() -> Result<(), String> {
    run(&["service", "restart"]).map(drop)
}

/// Download the models setup could not, as `journal install-models` does.
pub fn install_models() -> Result<(), String> {
    run(&["install-models"]).map(drop)
}

pub fn set_starts_at_sign_in(on: bool) -> Result<(), String> {
    run(&["service", "__sign-in", if on { "on" } else { "off" }]).map(drop)
}

/// Setup's exit when the only step that failed was downloading the models:
/// everything else is in place and the journal is running.
const MODELS_ONLY_FAILED: i32 = 80;

/// `journal setup` for a journal at `journal`, new or already there, with
/// its progress handed to `progress` line by line. Setup registers the
/// journal with Windows and starts it, exactly as it does from a terminal.
/// `Ok(Some(reason))` means setup finished but the models did not download.
pub fn setup(
    journal: &Path,
    mut progress: impl FnMut(SetupProgress),
) -> Result<Option<String>, String> {
    let mut child = command(&[
        "setup",
        "--jsonl",
        "--yes",
        "--accept-existing-journal",
        "--journal",
    ])
    .arg(journal)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|error| format!("couldn't start setup: {error}"))?;
    let stderr = child.stderr.take();
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
        }
        text
    });
    let mut failure = None;
    let mut models_failure = None;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(event) = parse_line(&line) {
                if let SetupProgress::StepFailed { step, message } = &event {
                    if step == "install_models" {
                        models_failure = Some(message.clone());
                    } else {
                        failure = Some(message.clone());
                    }
                }
                progress(event);
            }
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("setup didn't finish: {error}"))?;
    let stderr = stderr_reader.join().unwrap_or_default();
    if status.success() {
        return Ok(None);
    }
    if status.code() == Some(MODELS_ONLY_FAILED) {
        return Ok(Some(models_failure.unwrap_or_default()));
    }
    Err(failure
        .filter(|message| !message.is_empty())
        .or_else(|| {
            stderr
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "setup didn't finish".to_owned()))
}

/// Whether `folder` already holds a journal, so setup adopts it in place
/// rather than creating one. A journal keeps its settings under `config`,
/// the same test the Mac app applies to a journal it finds on disk.
pub fn holds_a_journal(folder: &Path) -> bool {
    folder.join("config").is_dir()
}
