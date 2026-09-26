// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The Windows package puts its commands on the owner's own `PATH`.
//!
//! `journal setup` skips the command-wrapper step on Windows because the
//! package owns the commands there. The package keeps that promise: the
//! Velopack install hook adds `<root>\current\bin` to the owner's user `Path`
//! (`HKCU\Environment`), and the uninstall hook removes exactly that entry.
//! Only the per-user value is touched, so no elevation is involved, and every
//! other entry keeps its text, order and registry type.

/// The owner's `Path` with `dir` appended, or `None` when an entry already
/// names it (compared the way Windows resolves paths: case-insensitive, with
/// a trailing separator ignored).
#[cfg(any(windows, test))]
fn with_entry(path: &str, dir: &str) -> Option<String> {
    if path.split(';').any(|entry| same_dir(entry, dir)) {
        return None;
    }
    let kept = path.trim_end_matches(';');
    Some(if kept.is_empty() {
        dir.to_owned()
    } else {
        format!("{kept};{dir}")
    })
}

/// The owner's `Path` without any entry naming `dir`, or `None` when no entry
/// does. Every other entry is kept as written.
#[cfg(any(windows, test))]
fn without_entry(path: &str, dir: &str) -> Option<String> {
    let entries: Vec<&str> = path.split(';').collect();
    if !entries.iter().any(|entry| same_dir(entry, dir)) {
        return None;
    }
    Some(
        entries
            .into_iter()
            .filter(|entry| !same_dir(entry, dir))
            .collect::<Vec<_>>()
            .join(";"),
    )
}

#[cfg(any(windows, test))]
fn same_dir(entry: &str, dir: &str) -> bool {
    let normal = |value: &str| value.trim().trim_end_matches(['\\', '/']).to_lowercase();
    let entry = normal(entry);
    !entry.is_empty() && entry == normal(dir)
}

/// `<root>\current\bin`, the directory this installed `journal.exe` runs from,
/// or `None` outside a Velopack install layout.
#[cfg(windows)]
fn installed_commands_dir() -> Option<String> {
    let journal = std::env::current_exe().ok()?;
    let bin = journal.parent()?;
    let current = bin.parent()?;
    let is = |path: &std::path::Path, name: &str| {
        path.file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(name))
    };
    if !(is(bin, "bin") && is(current, "current")) {
        return None;
    }
    bin.to_str().map(str::to_owned)
}

/// Add the installed commands to the owner's `Path` (the install hook).
#[cfg(windows)]
pub fn add_commands_to_owner_path() {
    if let Some(dir) = installed_commands_dir() {
        if let Err(error) = registry::update(|path| with_entry(path, &dir)) {
            eprintln!("journal commands were not added to PATH: {error}");
        }
    }
}

/// Remove the installed commands from the owner's `Path` (the uninstall hook).
#[cfg(windows)]
pub fn remove_commands_from_owner_path() {
    if let Some(dir) = installed_commands_dir() {
        if let Err(error) = registry::update(|path| without_entry(path, &dir)) {
            eprintln!("journal commands were not removed from PATH: {error}");
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod registry {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, LPARAM, WPARAM};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ,
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the key was returned open by RegCreateKeyExW.
            unsafe { RegCloseKey(self.0) };
        }
    }

    /// Read `HKCU\Environment\Path`, apply `change`, and write the result
    /// back with the value's own type (a new value is `REG_EXPAND_SZ`, as
    /// Windows' own editor writes it). `None` from `change` writes nothing.
    pub(super) fn update(change: impl FnOnce(&str) -> Option<String>) -> Result<(), String> {
        let mut raw = ptr::null_mut();
        let subkey = wide("Environment");
        // SAFETY: the subkey is terminated and raw points to output storage.
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                ptr::null(),
                &mut raw,
                ptr::null_mut(),
            )
        };
        if code != 0 {
            return Err(format!(
                "could not open the user environment (Windows error {code})"
            ));
        }
        let key = Key(raw);
        let name = wide("Path");
        let (kind, current) = read(&key, &name)?;
        let Some(next) = change(&current) else {
            return Ok(());
        };
        let code = if next.is_empty() {
            // SAFETY: the key is open and the name is terminated.
            unsafe { RegDeleteValueW(key.0, name.as_ptr()) }
        } else {
            let bytes: Vec<u8> = wide(&next).into_iter().flat_map(u16::to_le_bytes).collect();
            // SAFETY: the pointers refer to live name and value buffers.
            unsafe {
                RegSetValueExW(
                    key.0,
                    name.as_ptr(),
                    0,
                    kind,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                )
            }
        };
        if code != 0 {
            return Err(format!(
                "could not write the user Path (Windows error {code})"
            ));
        }
        if !next.is_empty() && read(&key, &name)?.1 != next {
            return Err("the user Path read back differently".to_owned());
        }
        drop(key);
        // New terminals inherit Explorer's environment; tell it to reload.
        let environment = wide("Environment");
        let mut result = 0;
        // SAFETY: the parameter string outlives this bounded, synchronous call.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0 as WPARAM,
                environment.as_ptr() as LPARAM,
                SMTO_ABORTIFHUNG,
                2000,
                &mut result,
            )
        };
        Ok(())
    }

    fn read(key: &Key, name: &[u16]) -> Result<(u32, String), String> {
        let mut kind = 0;
        let mut length = 0_u32;
        // SAFETY: a null data pointer asks only for the value's type and size.
        let code = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                ptr::null(),
                &mut kind,
                ptr::null_mut(),
                &mut length,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok((REG_EXPAND_SZ, String::new()));
        }
        if code != 0 {
            return Err(format!(
                "could not read the user Path (Windows error {code})"
            ));
        }
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return Err(format!("the user Path has unexpected registry type {kind}"));
        }
        let mut data = vec![0_u16; (length as usize).div_ceil(2) + 1];
        let mut bytes = (data.len() * 2) as u32;
        // SAFETY: data holds bytes bytes and bytes points to its size.
        let code = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                ptr::null(),
                &mut kind,
                data.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if code != 0 {
            return Err(format!(
                "could not read the user Path (Windows error {code})"
            ));
        }
        data.truncate(bytes as usize / 2);
        while data.last() == Some(&0) {
            data.pop();
        }
        String::from_utf16(&data)
            .map(|value| (kind, value))
            .map_err(|_| "the user Path is not valid text".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Users\owner\AppData\Local\SolstoneJournal\current\bin";

    #[test]
    fn install_appends_once_and_keeps_every_other_entry() {
        let path = r"%USERPROFILE%\AppData\Local\Microsoft\WindowsApps;C:\tools;";
        let added = with_entry(path, DIR).expect("appended");
        assert_eq!(
            added,
            format!(r"%USERPROFILE%\AppData\Local\Microsoft\WindowsApps;C:\tools;{DIR}")
        );
        assert_eq!(with_entry(&added, DIR), None);
        assert_eq!(with_entry("", DIR).as_deref(), Some(DIR));
        let spelled = DIR.to_uppercase() + "\\";
        assert_eq!(with_entry(&format!("C:\\tools;{spelled}"), DIR), None);
    }

    #[test]
    fn uninstall_removes_only_the_package_entry() {
        let path = format!(r"C:\tools;{DIR};%USERPROFILE%\bin;{}\", DIR.to_lowercase());
        assert_eq!(
            without_entry(&path, DIR).as_deref(),
            Some(r"C:\tools;%USERPROFILE%\bin")
        );
        assert_eq!(without_entry(r"C:\tools;;D:\x", DIR), None);
        assert_eq!(without_entry(DIR, DIR).as_deref(), Some(""));
    }
}
