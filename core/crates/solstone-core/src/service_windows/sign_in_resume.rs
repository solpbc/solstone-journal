// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A pending Windows stop is resumed by the owner's next interactive sign-in.
//! HKCU Run works for standard users; RunOnce does not. The entry exists only
//! while stopped, and wscript runs the Scheduler controls without a console.

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn value_name(id: &str) -> String {
    format!("SolstoneJournalResume-{id}")
}

fn script_path(owner_base: &Path, id: &str) -> Result<PathBuf, String> {
    let solstone = owner_base
        .ancestors()
        .nth(2)
        .ok_or("installation owner base is unavailable")?;
    Ok(solstone.join(format!("journal-resume-{id}.vbs")))
}

fn script(task_path: &str, value_name: &str) -> String {
    let quote = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
    [
        "' SPDX-License-Identifier: AGPL-3.0-only".to_owned(),
        "' Copyright (c) 2026 sol pbc".to_owned(),
        "On Error Resume Next".to_owned(),
        "Set sh = CreateObject(\"WScript.Shell\")".to_owned(),
        "Set fso = CreateObject(\"Scripting.FileSystemObject\")".to_owned(),
        format!("task = {}", quote(task_path)),
        format!(
            "entry = {}",
            quote(&format!("HKCU\\{RUN_KEY}\\{value_name}"))
        ),
        "exe = sh.ExpandEnvironmentStrings(\"%SystemRoot%\") & \"\\System32\\schtasks.exe\""
            .to_owned(),
        "Function Quoted(value)".to_owned(),
        "  Quoted = Chr(34) & value & Chr(34)".to_owned(),
        "End Function".to_owned(),
        "Function RunHidden(command)".to_owned(),
        "  On Error Resume Next".to_owned(),
        "  RunHidden = 1".to_owned(),
        "  Err.Clear".to_owned(),
        "  RunHidden = sh.Run(command, 0, True)".to_owned(),
        "  If Err.Number <> 0 Then RunHidden = 1".to_owned(),
        "End Function".to_owned(),
        "enable = RunHidden(Quoted(exe) & \" /change /tn \" & Quoted(task) & \" /enable\")"
            .to_owned(),
        "If enable = 0 Then".to_owned(),
        "  started = RunHidden(Quoted(exe) & \" /run /tn \" & Quoted(task))".to_owned(),
        "  If started = 0 Then".to_owned(),
        "    Err.Clear".to_owned(),
        "    sh.RegDelete entry".to_owned(),
        "    If Err.Number = 0 Then fso.DeleteFile WScript.ScriptFullName, True".to_owned(),
        "  End If".to_owned(),
        "Else".to_owned(),
        "  found = RunHidden(Quoted(exe) & \" /query /tn \" & Quoted(task))".to_owned(),
        "  If found <> 0 Then".to_owned(),
        "    Err.Clear".to_owned(),
        "    sh.RegDelete entry".to_owned(),
        "    If Err.Number = 0 Then fso.DeleteFile WScript.ScriptFullName, True".to_owned(),
        "  End If".to_owned(),
        "End If".to_owned(),
    ]
    .join("\r\n")
}

fn registry_command(action: &str, id: &str, value: Option<&str>) -> Result<(), String> {
    let windows = std::env::var_os("SystemRoot").ok_or("SystemRoot is unavailable")?;
    let powershell = Path::new(&windows).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    // The payloads are UTF-8/base64 literals, so owner paths never become
    // PowerShell source text. The .NET registry API provides a safe readback.
    let name = STANDARD.encode(value_name(id).as_bytes());
    let value = STANDARD.encode(value.unwrap_or_default().as_bytes());
    let operation = if action == "write" {
        format!(
            r#"$key=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('{RUN_KEY}', $true)
if($null -eq $key){{throw 'Run key unavailable'}}
try {{
  $key.SetValue($name,$value,[Microsoft.Win32.RegistryValueKind]::String)
  if($key.GetValueKind($name) -ne [Microsoft.Win32.RegistryValueKind]::String -or
     ![string]::Equals([string]$key.GetValue($name),$value,[StringComparison]::Ordinal)){{throw 'Run value readback differed'}}
}} finally {{$key.Close()}}"#
        )
    } else {
        format!(
            r#"$key=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('{RUN_KEY}', $true)
if($null -ne $key){{
  try {{
    $key.DeleteValue($name,$false)
    if($null -ne $key.GetValue($name,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)){{throw 'Run value remained'}}
  }} finally {{$key.Close()}}
}}"#
        )
    };
    let script = format!(
        r#"$ErrorActionPreference='Stop'
try {{
  $name=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{name}'))
  $value=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{value}'))
  {operation}
}} catch {{
  [Console]::Error.WriteLine($_.Exception.Message)
  exit 1
}}"#
    );
    let encoded = STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let output = Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("Windows sign-in resume {action} could not start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Windows sign-in resume {action} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn set_entry(id: &str, command: &str) -> Result<(), String> {
    registry_command("write", id, Some(command))
}

fn delete_entry(id: &str) -> Result<(), String> {
    registry_command("delete", id, None)
}

pub(super) fn arm(owner_base: &Path, id: &str, task_path: &str) -> Result<(), String> {
    let script_path = script_path(owner_base, id)?;
    let windows = std::env::var_os("SystemRoot").ok_or("SystemRoot is unavailable")?;
    let wscript = Path::new(&windows).join("System32/wscript.exe");
    let command = format!(
        "\"{}\" //B //Nologo \"{}\"",
        wscript.display(),
        script_path.display()
    );
    if command.encode_utf16().count() > 260
        || [wscript.as_os_str(), script_path.as_os_str()]
            .iter()
            .any(|path| path.to_string_lossy().contains(['"', '\r', '\n']))
    {
        return Err(
            "Windows sign-in resume command exceeds the Run key limit or has an invalid path"
                .to_owned(),
        );
    }
    fs::write(&script_path, script(task_path, &value_name(id)))
        .map_err(|error| format!("could not prepare Windows sign-in resume: {error}"))?;
    set_entry(id, &command)
}

pub(super) fn clear(owner_base: &Path, id: &str) -> Result<(), String> {
    delete_entry(id)?;
    let path = script_path(owner_base, id)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not clear Windows sign-in resume: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_resume_uses_only_the_owner_task_and_clears_pending_state() {
        let body = script(
            r"\solstone-S-1-5-21-123\00112233445566778899aabbccddeeff",
            "SolstoneJournalResume-00112233445566778899aabbccddeeff",
        );
        assert!(body.contains("sh.Run(command, 0, True)"));
        assert!(body.contains("Function RunHidden(command)\r\n  On Error Resume Next"));
        assert!(body.contains(" /change /tn "));
        assert!(body.contains(" /run /tn "));
        assert!(body.contains("sh.RegDelete entry"));
        assert!(!body.contains("powershell"));
    }
}
