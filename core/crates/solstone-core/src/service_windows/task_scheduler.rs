// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded Task Scheduler COM transport. Profile/binding policy stays in the caller.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::json;
use std::process::Command;

#[path = "task_scheduler/unowned_control.rs"]
mod unowned_control;

const SCHEMA: &str = "solstone-windows-task-operation-v1";
const SCRIPT: &str = include_str!("task_scheduler.ps1");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskInstance {
    pub guid: String,
    pub engine_pid: u32,
    pub current_action: String,
}

impl TaskInstance {
    fn valid_guid(&self) -> bool {
        let bytes = self.guid.as_bytes();
        bytes.len() == 38
            && bytes[0] == b'{'
            && bytes[37] == b'}'
            && bytes[1..37].iter().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    *byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
            && self.guid != "{00000000-0000-0000-0000-000000000000}"
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    schema: String,
    pub present: bool,
    pub folder_sddl: Option<String>,
    pub task_sddl: Option<String>,
    pub xml: Option<String>,
    pub validation_xml: Option<String>,
    pub state: Option<u32>,
    pub last_run: Option<String>,
    pub last_result: Option<i64>,
    pub instances: Vec<TaskInstance>,
    pub run_instance: Option<TaskInstance>,
}

pub(super) enum Operation<'a> {
    Inspect,
    Create {
        xml: &'a str,
    },
    Update {
        before: &'a Snapshot,
        xml: &'a str,
    },
    Run {
        before: &'a Snapshot,
    },
    Delete {
        before: &'a Snapshot,
    },
    /// Velopack kills the install-root process tree after this hook returns.
    DeleteBeforeUninstall {
        before: &'a Snapshot,
    },
    /// Record the owner's run intent on the registration itself.
    SetEnabled {
        before: &'a Snapshot,
        enabled: bool,
    },
}

/// Encode the fixed script for PowerShell's UTF-16LE EncodedCommand boundary.
/// No request data enters the script or its command-line arguments.
fn encoded_script() -> String {
    let wire = solstone_core_service_unit::powershell_wire_script(SCRIPT);
    let bytes: Vec<_> = wire.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(super) fn execute(
    owner_sid: &str,
    installation_id: &str,
    operation: Operation<'_>,
) -> Result<Snapshot, String> {
    execute_until(
        owner_sid,
        installation_id,
        operation,
        std::time::Instant::now() + std::time::Duration::from_secs(15),
    )
}

pub(super) fn execute_until(
    owner_sid: &str,
    installation_id: &str,
    operation: Operation<'_>,
    deadline: std::time::Instant,
) -> Result<Snapshot, String> {
    let (name, before, xml) = match operation {
        Operation::Inspect => ("inspect", None, None),
        Operation::Create { xml } => ("create", None, Some(xml)),
        Operation::Update { before, xml } => ("update", Some(before), Some(xml)),
        Operation::Run { before } => ("run", Some(before), None),
        Operation::Delete { before } => ("delete", Some(before), None),
        Operation::DeleteBeforeUninstall { before } => ("uninstall-delete", Some(before), None),
        Operation::SetEnabled {
            before,
            enabled: true,
        } => ("enable", Some(before), None),
        Operation::SetEnabled {
            before,
            enabled: false,
        } => ("disable", Some(before), None),
    };
    let mut directory = vec![0u16; 32768];
    #[allow(unsafe_code)]
    // SAFETY: the initialized UTF-16 buffer has the declared capacity; the API
    // writes at most that capacity and retains no caller memory.
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW(
            directory.as_mut_ptr(),
            directory.len() as u32,
        )
    } as usize;
    if length == 0 || length >= directory.len() {
        return Err("native Windows directory is unavailable".to_owned());
    }
    let system_root = OsString::from_wide(&directory[..length]);
    let system_root_path = PathBuf::from(&system_root);
    let executable = system_root_path.join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let request = json!({
        "schema": SCHEMA,
        "owner_sid": owner_sid,
        "installation_id": installation_id,
        "operation": name,
        "xml": xml,
        "expected_xml": before.and_then(|snapshot| snapshot.xml.as_ref()),
        "expected_task_sddl": before.and_then(|snapshot| snapshot.task_sddl.as_ref()),
        "expected_folder_sddl": before.and_then(|snapshot| snapshot.folder_sddl.as_ref()),
    });
    // Unowned OS-manager control worker: its exit is never a task-tree receipt.
    let mut command = Command::new(executable);
    command
        .current_dir(system_root_path)
        .env_clear()
        .env("SystemRoot", system_root)
        .env("PATH", "")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
        ])
        .arg(encoded_script());
    let output = unowned_control::run(
        command,
        serde_json::to_vec(&request).map_err(|error| error.to_string())?,
        deadline,
    )?;
    if output.code != Some(0) {
        // The reason is Windows' text, not ours: the fallback arm joins whatever
        // the worker wrote to stderr once the CLIXML envelope is removed, so it
        // can carry control bytes, escape sequences and any length PowerShell
        // felt like emitting -- straight into a line an owner reads in a
        // terminal. Bound and sanitize it here rather than in the extractor,
        // which stays a pure text function; the raw streams remain in the
        // operation's own captured output for support.
        let reason = solstone_core_system_health::sanitize_str_for_terminal_bounded(
            &solstone_core_service_unit::windows_task_failure_reason(
                &output.stdout,
                &output.stderr,
            ),
        );
        return Err(render_operation_failure(name, &reason));
    }
    let snapshot: Snapshot = serde_json::from_slice(&output.stdout)
        .map_err(|_| "task operation returned invalid JSON")?;
    if snapshot.schema != SCHEMA
        || snapshot
            .instances
            .iter()
            .any(|instance| !instance.valid_guid())
        || snapshot
            .run_instance
            .as_ref()
            .is_some_and(|instance| !instance.valid_guid())
        || (name == "run") != snapshot.run_instance.is_some()
        || (snapshot.present
            && (snapshot.xml.is_none()
                || snapshot.validation_xml.is_none()
                || snapshot.task_sddl.is_none()
                || snapshot.folder_sddl.is_none()
                || snapshot.state.is_none()
                || snapshot.last_run.is_none()
                || snapshot.last_result.is_none()))
        || (!snapshot.present
            && (snapshot.xml.is_some()
                || snapshot.validation_xml.is_some()
                || snapshot.task_sddl.is_some()
                || snapshot.state.is_some()
                || snapshot.last_run.is_some()
                || snapshot.last_result.is_some()
                || !snapshot.instances.is_empty()
                || snapshot.run_instance.is_some()))
    {
        return Err("task operation returned inconsistent evidence".to_owned());
    }
    Ok(snapshot)
}

fn render_operation_failure(operation: &str, reason: &str) -> String {
    let reason = if reason == "windows gave no reason" {
        "windows gave no reason."
    } else {
        reason
    };
    if operation == "inspect" {
        format!(
            "background support for your journal couldn't be checked.\n\
             try again in a moment. if it keeps failing, include the details below in a support request.\n\
             details: {reason}"
        )
    } else {
        format!(
            "background support for your journal couldn't be changed.\n\
             run `journal service status` to check it.\n\
             details: {reason}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::render_operation_failure;

    #[test]
    fn a_change_failure_names_the_status_recovery_and_keeps_details_separate() {
        assert_eq!(
            render_operation_failure("update", "powershell.exe: access is denied"),
            "background support for your journal couldn't be changed.\n\
             run `journal service status` to check it.\n\
             details: powershell.exe: access is denied"
        );
    }

    #[test]
    fn an_inspect_failure_does_not_send_the_owner_back_into_the_failed_read() {
        assert_eq!(
            render_operation_failure("inspect", "task-read-refused"),
            "background support for your journal couldn't be checked.\n\
             try again in a moment. if it keeps failing, include the details below in a support request.\n\
             details: task-read-refused"
        );
    }

    #[test]
    fn an_empty_worker_reason_keeps_the_locked_honest_fallback() {
        assert_eq!(
            render_operation_failure("run", "windows gave no reason"),
            "background support for your journal couldn't be changed.\n\
             run `journal service status` to check it.\n\
             details: windows gave no reason."
        );
    }
}
