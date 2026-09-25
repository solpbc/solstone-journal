// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A pending Windows stop is resumed by the owner's next interactive sign-in.
//! HKCU Run works for standard users; RunOnce does not. The entry exists only
//! while stopped, and wscript runs the Scheduler controls without a console.

use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

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

fn registry_error(action: &str, code: u32) -> String {
    format!("Windows sign-in resume {action} failed (Windows error {code})")
}

#[allow(unsafe_code)]
fn open_run_key(create: bool) -> Result<Option<HKEY>, String> {
    let mut key = ptr::null_mut();
    let path = wide(OsStr::new(RUN_KEY));
    let code = if create {
        // SAFETY: all pointers refer to live, terminated buffers or output storage.
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                ptr::null(),
                &mut key,
                ptr::null_mut(),
            )
        }
    } else {
        // SAFETY: the path is terminated and key points to output storage.
        unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                &mut key,
            )
        }
    };
    if code == ERROR_FILE_NOT_FOUND && !create {
        return Ok(None);
    }
    if code != 0 {
        return Err(registry_error("open", code));
    }
    Ok(Some(key))
}

#[allow(unsafe_code)]
fn close_key(key: HKEY) {
    // SAFETY: key was returned by RegOpenKeyExW or RegCreateKeyExW above.
    unsafe { RegCloseKey(key) };
}

#[allow(unsafe_code)]
fn set_entry(id: &str, command: &str) -> Result<(), String> {
    let key = open_run_key(true)?.ok_or("Windows Run key was not opened")?;
    let name = wide(OsStr::new(&value_name(id)));
    let value = wide(OsStr::new(command));
    let bytes: Vec<u8> = value.into_iter().flat_map(u16::to_le_bytes).collect();
    // SAFETY: pointers refer to live name and value buffers for this call.
    let code = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    if code != 0 {
        close_key(key);
        return Err(registry_error("write", code));
    }
    let mut kind = 0;
    let mut readback = vec![0_u8; bytes.len() + 2];
    let mut length = readback.len() as u32;
    // SAFETY: the readback buffer has length bytes and length points to its size.
    let code = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            readback.as_mut_ptr(),
            &mut length,
        )
    };
    close_key(key);
    if code != 0 || kind != REG_SZ || readback[..length as usize] != bytes {
        return Err("Windows sign-in resume registry readback differed".to_owned());
    }
    Ok(())
}

#[allow(unsafe_code)]
fn delete_entry(id: &str) -> Result<(), String> {
    let Some(key) = open_run_key(false)? else {
        return Ok(());
    };
    let name = wide(OsStr::new(&value_name(id)));
    // SAFETY: key is open and name is a terminated buffer.
    let code = unsafe { RegDeleteValueW(key, name.as_ptr()) };
    close_key(key);
    if code != 0 && code != ERROR_FILE_NOT_FOUND {
        return Err(registry_error("delete", code));
    }
    Ok(())
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
