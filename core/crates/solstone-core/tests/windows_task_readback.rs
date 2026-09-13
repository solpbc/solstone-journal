// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use base64::Engine as _;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

#[path = "../src/service_windows/task_scheduler/unowned_control.rs"]
mod unowned_control;

const SCRIPT: &str = include_str!("../src/service_windows/task_scheduler.ps1");

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
