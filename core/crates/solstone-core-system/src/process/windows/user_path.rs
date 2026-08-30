// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows user-path and long-path preparation for application names.

use thiserror::Error;

const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;
const QUESTION: u16 = b'?' as u16;
const COLON: u16 = b':' as u16;
const DOT: u16 = b'.' as u16;
const NUL: u16 = 0;
const VERBATIM_PREFIX: &[u16] = &[BACKSLASH, BACKSLASH, QUESTION, BACKSLASH];
const NT_PREFIX: &[u16] = &[BACKSLASH, QUESTION, QUESTION, BACKSLASH];
const UNC_PREFIX: &[u16] = &[
    BACKSLASH,
    BACKSLASH,
    QUESTION,
    BACKSLASH,
    b'U' as u16,
    b'N' as u16,
    b'C' as u16,
    BACKSLASH,
];

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(super) enum WindowsFullPathError {
    #[error("Windows full-path lookup failed: {0}")]
    Lookup(String),
    #[error("Windows full-path lookup returned a malformed UTF-16 buffer")]
    MalformedResult,
    #[error("Windows paths cannot contain an interior NUL")]
    InteriorNul,
}

pub(super) type WindowsFullPathResult<T> = Result<T, WindowsFullPathError>;

/// Raw `GetFullPathNameW`-style path normalization.
///
/// Inputs and successful outputs are NUL-terminated UTF-16 buffers.
pub(super) trait WindowsFullPathName {
    fn get_full_path_name(&self, path: &[u16]) -> WindowsFullPathResult<Vec<u16>>;
}

/// Return whether `path` has no Windows directory separator.
///
/// Derived from Rust 1.97.1 `sys/path/windows.rs::is_file_name`.
pub(super) fn is_file_name(path: &[u16]) -> bool {
    !path.iter().any(|&unit| matches!(unit, BACKSLASH | SLASH))
}

/// Return whether `path` ends in a separator under the Windows namespace rules.
///
/// Derived from Rust 1.97.1 `sys/path/windows.rs::has_trailing_slash`.
pub(super) fn has_trailing_slash(path: &[u16]) -> bool {
    let Some(&last) = path.last() else {
        return false;
    };
    if path.starts_with(VERBATIM_PREFIX) {
        last == BACKSLASH
    } else {
        matches!(last, BACKSLASH | SLASH)
    }
}

/// Append a suffix without replacing an existing extension.
///
/// Derived from Rust 1.97.1 `sys/path/windows.rs::append_suffix`.
pub(super) fn append_suffix(path: &[u16], suffix: &[u16]) -> Vec<u16> {
    let mut result = Vec::with_capacity(path.len() + suffix.len());
    result.extend_from_slice(path);
    result.extend_from_slice(suffix);
    result
}

/// Convert a potentially verbatim path into a user-facing application path.
///
/// Derived from Rust 1.97.1 `sys/args/windows.rs::to_user_path`. This is a
/// derived adapter-based form so Linux-hosted tests do not call Win32 directly.
pub(super) fn to_user_path(
    path: &[u16],
    full_path: &impl WindowsFullPathName,
) -> WindowsFullPathResult<Vec<u16>> {
    if path.contains(&NUL) {
        return Err(WindowsFullPathError::InteriorNul);
    }
    let mut terminated = path.to_vec();
    terminated.push(NUL);
    from_wide_to_user_path(terminated, full_path)
}

/// Convert a NUL-terminated Windows path away from a verbatim prefix when safe.
///
/// Derived from Rust 1.97.1 `sys/args/windows.rs::from_wide_to_user_path`.
pub(super) fn from_wide_to_user_path(
    mut path: Vec<u16>,
    full_path: &impl WindowsFullPathName,
) -> WindowsFullPathResult<Vec<u16>> {
    validate_terminated(&path)?;
    if path.len() > 260 {
        return Ok(path);
    }

    if path.starts_with(UNC_PREFIX) {
        path[6] = BACKSLASH;
        let original_without_prefix = path[6..path.len() - 1].to_vec();
        let normalized = full_path.get_full_path_name(&path[6..])?;
        validate_terminated(&normalized)?;
        if normalized[..normalized.len() - 1] == original_without_prefix {
            return Ok(normalized);
        }
        path[6] = b'C' as u16;
        return Ok(path);
    }

    if matches!(
        path.as_slice(),
        [
            BACKSLASH,
            BACKSLASH,
            QUESTION,
            BACKSLASH,
            _,
            COLON,
            BACKSLASH,
            ..
        ]
    ) {
        let original_without_prefix = path[4..path.len() - 1].to_vec();
        let normalized = full_path.get_full_path_name(&path[4..])?;
        validate_terminated(&normalized)?;
        if normalized[..normalized.len() - 1] == original_without_prefix {
            return Ok(normalized);
        }
        return Ok(path);
    }

    get_long_path(path, false, full_path)
}

/// Normalize a path and add a verbatim prefix when Windows length rules require it.
///
/// Derived from Rust 1.97.1 `sys/path/windows.rs::get_long_path`.
pub(super) fn get_long_path(
    path: Vec<u16>,
    prefer_verbatim: bool,
    full_path: &impl WindowsFullPathName,
) -> WindowsFullPathResult<Vec<u16>> {
    validate_terminated(&path)?;
    if path.starts_with(VERBATIM_PREFIX) || path.starts_with(NT_PREFIX) || path == [NUL] {
        return Ok(path);
    }

    if path.len() < 248 && is_short_exact_path(&path) {
        return Ok(path);
    }

    let mut absolute = full_path.get_full_path_name(&path)?;
    validate_terminated(&absolute)?;
    let body = &absolute[..absolute.len() - 1];
    if prefer_verbatim || absolute.len() >= 248 {
        let prefix = match body {
            [_, COLON, BACKSLASH, ..] => VERBATIM_PREFIX,
            [BACKSLASH, BACKSLASH, DOT, BACKSLASH, ..] => {
                absolute.drain(..4);
                VERBATIM_PREFIX
            }
            [BACKSLASH, BACKSLASH, QUESTION, BACKSLASH, ..]
            | [BACKSLASH, QUESTION, QUESTION, BACKSLASH, ..] => &[],
            [BACKSLASH, BACKSLASH, ..] => {
                absolute.drain(..2);
                UNC_PREFIX
            }
            _ => &[],
        };
        if !prefix.is_empty() {
            let mut prefixed = Vec::with_capacity(prefix.len() + absolute.len());
            prefixed.extend_from_slice(prefix);
            prefixed.extend_from_slice(&absolute);
            return Ok(prefixed);
        }
    }
    Ok(absolute)
}

fn is_short_exact_path(path: &[u16]) -> bool {
    match path {
        [drive, COLON, NUL] | [drive, COLON, BACKSLASH | SLASH, ..]
            if *drive != BACKSLASH && *drive != SLASH =>
        {
            true
        }
        [BACKSLASH | SLASH, BACKSLASH | SLASH, ..] => true,
        _ => false,
    }
}

fn validate_terminated(path: &[u16]) -> WindowsFullPathResult<()> {
    if path.last() != Some(&NUL) || path[..path.len().saturating_sub(1)].contains(&NUL) {
        return Err(WindowsFullPathError::MalformedResult);
    }
    Ok(())
}

#[cfg(windows)]
pub(super) struct SystemWindowsFullPathName;

#[cfg(windows)]
impl WindowsFullPathName for SystemWindowsFullPathName {
    fn get_full_path_name(&self, path: &[u16]) -> WindowsFullPathResult<Vec<u16>> {
        validate_terminated(path)?;
        let mut buffer = vec![0; 260];
        loop {
            #[allow(unsafe_code)]
            // SAFETY: `path` is NUL-terminated, while `buffer` is writable for exactly the
            // advertised count and remains alive for the duration of this synchronous call.
            let written = unsafe {
                windows_sys::Win32::Storage::FileSystem::GetFullPathNameW(
                    path.as_ptr(),
                    buffer.len() as u32,
                    buffer.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            };
            if written == 0 {
                return Err(WindowsFullPathError::Lookup(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            let written = written as usize;
            if written >= buffer.len() {
                buffer.resize(written + 1, NUL);
                continue;
            }
            buffer.truncate(written);
            buffer.push(NUL);
            return Ok(buffer);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    struct FakeFullPathName {
        outputs: RefCell<BTreeMap<Vec<u16>, Vec<u16>>>,
        failures: RefCell<BTreeSet<Vec<u16>>>,
        calls: RefCell<Vec<Vec<u16>>>,
        current_directory: RefCell<Vec<u16>>,
    }

    impl FakeFullPathName {
        fn with_current_directory(directory: &str) -> Self {
            Self {
                outputs: RefCell::new(BTreeMap::new()),
                failures: RefCell::new(BTreeSet::new()),
                calls: RefCell::new(Vec::new()),
                current_directory: RefCell::new(wide(directory)),
            }
        }

        fn map(&self, input: &str, output: &str) {
            self.outputs.borrow_mut().insert(wide(input), wide(output));
        }
    }

    impl WindowsFullPathName for FakeFullPathName {
        fn get_full_path_name(&self, path: &[u16]) -> WindowsFullPathResult<Vec<u16>> {
            self.calls.borrow_mut().push(path.to_vec());
            if self.failures.borrow().contains(path) {
                return Err(WindowsFullPathError::Lookup(
                    "configured failure".to_owned(),
                ));
            }
            if let Some(output) = self.outputs.borrow().get(path) {
                return Ok(output.clone());
            }
            if path == wide(r".\tool.exe") {
                let mut bound = self.current_directory.borrow().clone();
                if bound.last() == Some(&NUL) {
                    bound.pop();
                }
                if !bound.ends_with(&[BACKSLASH]) {
                    bound.push(BACKSLASH);
                }
                bound.extend(r"tool.exe".encode_utf16());
                bound.push(NUL);
                return Ok(bound);
            }
            Ok(path.to_vec())
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain([NUL]).collect()
    }

    fn drive_path_with_nul_length(length: usize) -> Vec<u16> {
        assert!(length >= 4);
        let mut path = r"C:\".encode_utf16().collect::<Vec<_>>();
        path.extend(std::iter::repeat_n(b'a' as u16, length - path.len() - 1));
        path.push(NUL);
        path
    }

    #[test]
    fn separator_relative_input_is_bound_at_preparation_time() {
        let fake = FakeFullPathName::with_current_directory(r"C:\first");
        let prepared = to_user_path(&r".\tool.exe".encode_utf16().collect::<Vec<_>>(), &fake)
            .expect("relative path binds");
        *fake.current_directory.borrow_mut() = wide(r"C:\later");
        assert_eq!(prepared, wide(r"C:\first\tool.exe"));
    }

    #[test]
    fn short_verbatim_drive_and_unc_paths_deprefix_only_when_unchanged() {
        let fake = FakeFullPathName::with_current_directory(r"C:\unused");
        fake.map(r"C:\tool.exe", r"C:\tool.exe");
        fake.map(r"\\server\share\tool.exe", r"\\server\share\tool.exe");
        assert_eq!(
            to_user_path(
                &r"\\?\C:\tool.exe".encode_utf16().collect::<Vec<_>>(),
                &fake
            )
            .unwrap(),
            wide(r"C:\tool.exe")
        );
        assert_eq!(
            to_user_path(
                &r"\\?\UNC\server\share\tool.exe"
                    .encode_utf16()
                    .collect::<Vec<_>>(),
                &fake,
            )
            .unwrap(),
            wide(r"\\server\share\tool.exe")
        );
    }

    #[test]
    fn short_verbatim_path_stays_verbatim_when_deprefixing_normalizes() {
        let fake = FakeFullPathName::with_current_directory(r"C:\unused");
        fake.map(r"C:\one\..\tool.exe", r"C:\tool.exe");
        let original = r"\\?\C:\one\..\tool.exe".encode_utf16().collect::<Vec<_>>();
        assert_eq!(
            to_user_path(&original, &fake).unwrap(),
            wide(r"\\?\C:\one\..\tool.exe")
        );
    }

    #[test]
    fn long_path_boundaries_follow_nul_inclusive_std_thresholds() {
        let fake = FakeFullPathName::with_current_directory(r"C:\unused");
        for length in [247, 248, 260, 261] {
            let path = drive_path_with_nul_length(length);
            fake.map(
                &String::from_utf16_lossy(&path[..path.len() - 1]),
                &String::from_utf16_lossy(&path[..path.len() - 1]),
            );
            let result = from_wide_to_user_path(path.clone(), &fake).unwrap();
            match length {
                247 => assert_eq!(result, path),
                248 | 260 => assert!(result.starts_with(VERBATIM_PREFIX)),
                261 => assert_eq!(result, path),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn trailing_separator_polarity_matches_windows_namespaces() {
        assert!(has_trailing_slash(
            &r"C:\tool\".encode_utf16().collect::<Vec<_>>()
        ));
        assert!(has_trailing_slash(
            &r"C:\tool/".encode_utf16().collect::<Vec<_>>()
        ));
        assert!(has_trailing_slash(
            &r"\??\C:\tool/".encode_utf16().collect::<Vec<_>>()
        ));
        assert!(has_trailing_slash(
            &r"\\?\C:\tool\".encode_utf16().collect::<Vec<_>>()
        ));
        assert!(!has_trailing_slash(
            &r"\\?\C:\tool/".encode_utf16().collect::<Vec<_>>()
        ));
    }

    #[test]
    fn nontrailing_nt_namespace_path_is_unchanged() {
        let fake = FakeFullPathName::with_current_directory(r"C:\unused");
        let raw = r"\??\C:\tool.exe".encode_utf16().collect::<Vec<_>>();
        assert_eq!(to_user_path(&raw, &fake).unwrap(), wide(r"\??\C:\tool.exe"));
        assert!(fake.calls.borrow().is_empty());
    }
}
