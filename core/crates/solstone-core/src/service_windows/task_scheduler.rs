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
    Create { xml: &'a str },
    Update { before: &'a Snapshot, xml: &'a str },
    Run { before: &'a Snapshot },
    Delete { before: &'a Snapshot },
}

/// Encode the fixed script for PowerShell's UTF-16LE EncodedCommand boundary.
/// No request data enters the script or its command-line arguments.
fn encoded_script() -> String {
    let bytes: Vec<_> = SCRIPT.encode_utf16().flat_map(u16::to_le_bytes).collect();
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
        return Err(format!(
            "task operation failed; scheduler state must be re-inspected: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
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

#[cfg(test)]
mod readback_tests {
    use super::*;
    use solstone_core_installation_identity::{
        Generation, GuardFields, InstallationId, NamespaceName, journal_token_from_path,
    };
    use solstone_core_service_unit::{
        WindowsServiceAction, WindowsTaskInput, parse_windows_task_xml, render_windows_task_xml,
    };

    #[test]
    fn normalizes_only_private_embedded_task_security() {
        let sid = "S-1-5-21-1-2-3-1001";
        let journal = r"C:\Users\Owner\Journal";
        let action = WindowsServiceAction {
            port: 6123,
            journal: journal.into(),
            guard: GuardFields {
                namespace: NamespaceName::parse(&"a".repeat(64)).unwrap(),
                id: InstallationId::parse(&"b".repeat(32)).unwrap(),
                generation: Generation::new(1).unwrap(),
                journal_token: journal_token_from_path(std::path::Path::new(journal)).unwrap(),
            },
        };
        let xml = render_windows_task_xml(&WindowsTaskInput {
            principal_sid: sid,
            command: r"C:\Program Files\Journal\journal.exe",
            working_directory: journal,
            action: &action,
        })
        .unwrap();
        let sddl = format!("O:{sid}G:{sid}D:PAI(A;;FA;;;{sid})(A;;FA;;;SY)(A;;FA;;;BA)");
        let metadata = format!("<SecurityDescriptor>{sddl}</SecurityDescriptor>");
        let with =
            |value: &str| xml.replace("<RegistrationInfo>", &format!("<RegistrationInfo>{value}"));
        let mut cases = vec![xml.clone(), with(&metadata)];
        for invalid in [
            metadata.replace(&format!("O:{sid}"), "O:SY"),
            metadata.replace("D:PAI", "D:AI"),
            metadata.replace("(A;;FA;;;BA)", "(A;;FA;;;WD)"),
            metadata.replace("(A;;FA;;;BA)", ""),
            metadata.replace("(A;;FA;;;BA)", "(A;;FR;;;BA)"),
            metadata.replace("D:PAI", "D:PNO_ACCESS_CONTROL"),
            metadata.replace("<SecurityDescriptor>", "<SecurityDescriptor extra=\"x\">"),
            format!("{metadata}{metadata}"),
            format!("<SecurityDescriptor><Value>{sddl}</Value></SecurityDescriptor>"),
        ] {
            cases.push(with(&invalid));
        }
        // Exercise the exact production normalization and native ACL parser,
        // without entering the script's scheduler-operation dispatch.
        let definitions = SCRIPT.split_once("\ntry {\n    $body =").unwrap().0;
        let script = format!(
            r#"{definitions}
$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$results = @(foreach ($xml in $request.cases) {{
    try {{ @{{ xml = (Get-ValidationXml $xml $request.sid); error = $null }} }}
    catch {{ @{{ xml = $null; error = $_.Exception.Message }} }}
}})
ConvertTo-Json -InputObject $results -Compress -Depth 4
"#
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(
            script
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let root = PathBuf::from(std::env::var_os("SystemRoot").expect("native Windows root"));
        let mut command = Command::new(root.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
        command
            .current_dir(&root)
            .env_clear()
            .env("SystemRoot", &root)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
            ])
            .arg(encoded);
        let output = unowned_control::run(
            command,
            serde_json::to_vec(&json!({"sid": sid, "cases": cases})).unwrap(),
            std::time::Instant::now() + std::time::Duration::from_secs(15),
        )
        .unwrap();
        assert_eq!(
            output.code,
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let results: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(results.len(), cases.len());
        let expected = parse_windows_task_xml(&xml).unwrap();
        for (index, result) in results.iter().enumerate() {
            if index < 2 {
                assert!(result["error"].is_null(), "{result}");
                assert_eq!(
                    parse_windows_task_xml(result["xml"].as_str().unwrap()).unwrap(),
                    expected
                );
            } else {
                assert!(
                    result["xml"].is_null() && result["error"].is_string(),
                    "accepted case {index}: {result}"
                );
            }
        }
    }
}
