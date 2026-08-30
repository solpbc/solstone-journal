// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows executable resolution for prepared launch specifications.

use thiserror::Error;

use super::path_list::split_windows_paths;
use super::user_path::{
    WindowsFullPathError, WindowsFullPathName, append_suffix, has_trailing_slash, is_file_name,
    to_user_path,
};

const EXE_SUFFIX: &[u16] = &[b'.' as u16, b'e' as u16, b'x' as u16, b'e' as u16];

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(super) enum WindowsProbeError {
    #[error("Windows candidate probe failed: {0}")]
    Probe(String),
}

pub(super) type WindowsProbeResult<T> = Result<T, WindowsProbeError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(super) enum WindowsDirectoryError {
    #[error("Windows executable directory lookup failed: {0}")]
    Lookup(String),
}

pub(super) type WindowsDirectoryResult<T> = Result<T, WindowsDirectoryError>;

#[derive(Debug, Error, PartialEq, Eq)]
pub(super) enum WindowsResolveError {
    #[error("program path has no file name")]
    InvalidProgram,
    #[error("program not found")]
    NotFound,
    #[error("resolved batch files are not supported")]
    BatchFileRefused,
    #[error(transparent)]
    FullPath(#[from] WindowsFullPathError),
}

/// Check whether a normalized candidate exists without following it.
pub(super) trait WindowsCandidateProbe {
    fn program_exists(&self, candidate: &[u16]) -> WindowsProbeResult<bool>;
}

/// Lookup the three non-PATH executable search directories.
pub(super) trait WindowsDirectoryLookup {
    fn current_exe_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>>;
    fn system_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>>;
    fn windows_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>>;
}

/// Resolve an original program spelling to a NUL-terminated application-name path.
///
/// Derived from Rust 1.97.1 `sys/process/windows.rs::{resolve_exe, search_paths,
/// program_exists}`. Refusing `.bat` and `.cmd` is a project deviation.
pub(super) fn resolve_executable(
    program: &[u16],
    probe: &dyn WindowsCandidateProbe,
    directories: &dyn WindowsDirectoryLookup,
    full_path: &impl WindowsFullPathName,
    child_path: Option<&[u16]>,
    parent_path: Option<&[u16]>,
) -> Result<Vec<u16>, WindowsResolveError> {
    if program.is_empty() || has_trailing_slash(program) || program.contains(&0) {
        return Err(WindowsResolveError::InvalidProgram);
    }

    if !is_file_name(program) {
        if has_exe_suffix(program) {
            return reject_batch(to_user_path(program, full_path)?);
        }
        let with_exe = append_suffix(program, EXE_SUFFIX);
        if let Some(found) = program_exists(&with_exe, probe, full_path) {
            return reject_batch(found);
        }
        return reject_batch(to_user_path(program, full_path)?);
    }

    let has_extension = program.contains(&(b'.' as u16));
    if let Some(found) = search_paths(
        program,
        has_extension,
        probe,
        directories,
        full_path,
        child_path,
        parent_path,
    ) {
        return reject_batch(found);
    }
    Err(WindowsResolveError::NotFound)
}

fn search_paths(
    program: &[u16],
    has_extension: bool,
    probe: &dyn WindowsCandidateProbe,
    directories: &dyn WindowsDirectoryLookup,
    full_path: &impl WindowsFullPathName,
    child_path: Option<&[u16]>,
    parent_path: Option<&[u16]>,
) -> Option<Vec<u16>> {
    if let Some(paths) = child_path
        && let Some(found) = search_path_list(paths, program, has_extension, probe, full_path)
    {
        return Some(found);
    }

    for directory in [
        directories.current_exe_directory().ok().flatten(),
        directories.system_directory().ok().flatten(),
        directories.windows_directory().ok().flatten(),
    ] {
        if let Some(directory) = directory
            && let Some(found) =
                search_directory(&directory, program, has_extension, probe, full_path)
        {
            return Some(found);
        }
    }

    if let Some(paths) = parent_path {
        return search_path_list(paths, program, has_extension, probe, full_path);
    }
    None
}

fn search_path_list(
    paths: &[u16],
    program: &[u16],
    has_extension: bool,
    probe: &dyn WindowsCandidateProbe,
    full_path: &impl WindowsFullPathName,
) -> Option<Vec<u16>> {
    split_windows_paths(paths)
        .into_iter()
        .filter(|directory| !directory.is_empty())
        .find_map(|directory| {
            search_directory(&directory, program, has_extension, probe, full_path)
        })
}

fn search_directory(
    directory: &[u16],
    program: &[u16],
    has_extension: bool,
    probe: &dyn WindowsCandidateProbe,
    full_path: &impl WindowsFullPathName,
) -> Option<Vec<u16>> {
    let mut candidate = join_directory_and_program(directory, program);
    if !has_extension {
        candidate = append_suffix(&candidate, EXE_SUFFIX);
    }
    program_exists(&candidate, probe, full_path)
}

fn join_directory_and_program(directory: &[u16], program: &[u16]) -> Vec<u16> {
    // A drive-relative program such as `C:tool.exe` must not become
    // `D:\bin\C:tool.exe`; `C:.\tool.exe` was already classified as a direct path.
    if is_drive_relative(program) {
        return program.to_vec();
    }
    let mut candidate = directory.to_vec();
    if !candidate.ends_with(&[b'\\' as u16]) && !candidate.ends_with(&[b'/' as u16]) {
        candidate.push(b'\\' as u16);
    }
    candidate.extend_from_slice(program);
    candidate
}

fn is_drive_relative(path: &[u16]) -> bool {
    matches!(path, [drive, COLON, ..] if *drive != BACKSLASH && *drive != SLASH)
}

fn program_exists(
    candidate: &[u16],
    probe: &dyn WindowsCandidateProbe,
    full_path: &impl WindowsFullPathName,
) -> Option<Vec<u16>> {
    let candidate = to_user_path(candidate, full_path).ok()?;
    probe
        .program_exists(&candidate)
        .ok()
        .filter(|exists| *exists)
        .map(|_| candidate)
}

fn has_exe_suffix(path: &[u16]) -> bool {
    path.len() >= EXE_SUFFIX.len()
        && path[path.len() - EXE_SUFFIX.len()..]
            .iter()
            .zip(EXE_SUFFIX)
            .all(|(actual, expected)| ascii_lower(*actual) == *expected)
}

fn reject_batch(path: Vec<u16>) -> Result<Vec<u16>, WindowsResolveError> {
    let path_without_nul = path.strip_suffix(&[0]).unwrap_or(&path);
    if has_ascii_suffix(path_without_nul, b".bat") || has_ascii_suffix(path_without_nul, b".cmd") {
        Err(WindowsResolveError::BatchFileRefused)
    } else {
        Ok(path)
    }
}

fn has_ascii_suffix(path: &[u16], suffix: &[u8]) -> bool {
    path.len() >= suffix.len()
        && path[path.len() - suffix.len()..]
            .iter()
            .zip(suffix)
            .all(|(actual, expected)| ascii_lower(*actual) == u16::from(*expected))
}

const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;
const COLON: u16 = b':' as u16;

fn ascii_lower(unit: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&unit) {
        unit + (b'a' as u16 - b'A' as u16)
    } else {
        unit
    }
}

#[cfg(windows)]
pub(super) struct SystemWindowsCandidateProbe;

#[cfg(windows)]
impl WindowsCandidateProbe for SystemWindowsCandidateProbe {
    fn program_exists(&self, candidate: &[u16]) -> WindowsProbeResult<bool> {
        #[allow(unsafe_code)]
        // SAFETY: `candidate` is a NUL-terminated owned UTF-16 buffer and the synchronous query
        // only reads it for the duration of this call.
        let attributes = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetFileAttributesW(candidate.as_ptr())
        };
        Ok(attributes != windows_sys::Win32::Storage::FileSystem::INVALID_FILE_ATTRIBUTES)
    }
}

#[cfg(windows)]
pub(super) struct SystemWindowsDirectoryLookup;

#[cfg(windows)]
impl WindowsDirectoryLookup for SystemWindowsDirectoryLookup {
    fn current_exe_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>> {
        use std::os::windows::ffi::OsStrExt;

        let Ok(mut path) = std::env::current_exe() else {
            return Ok(None);
        };
        if !path.pop() {
            return Ok(None);
        }
        Ok(Some(path.as_os_str().encode_wide().collect()))
    }

    fn system_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>> {
        get_directory_from_windows_api(
            windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW,
        )
    }

    fn windows_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>> {
        get_directory_from_windows_api(
            windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW,
        )
    }
}

#[cfg(windows)]
fn get_directory_from_windows_api(
    get_directory: unsafe extern "system" fn(*mut u16, u32) -> u32,
) -> WindowsDirectoryResult<Option<Vec<u16>>> {
    let mut buffer = vec![0; 260];
    loop {
        #[allow(unsafe_code)]
        // SAFETY: `buffer` is writable for its stated element count and the API does not retain
        // the pointer after it returns.
        let written = unsafe { get_directory(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        if written == 0 {
            return Ok(None);
        }
        if written >= buffer.len() {
            buffer.resize(written + 1, 0);
            continue;
        }
        buffer.truncate(written);
        return Ok(Some(buffer));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    struct FakeCandidateProbe {
        found: RefCell<BTreeSet<Vec<u16>>>,
        failures: RefCell<BTreeSet<Vec<u16>>>,
        calls: RefCell<Vec<Vec<u16>>>,
    }

    impl FakeCandidateProbe {
        fn new() -> Self {
            Self {
                found: RefCell::new(BTreeSet::new()),
                failures: RefCell::new(BTreeSet::new()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn present(&self, path: &str) {
            self.found.borrow_mut().insert(wide_terminated(path));
        }
    }

    impl WindowsCandidateProbe for FakeCandidateProbe {
        fn program_exists(&self, candidate: &[u16]) -> WindowsProbeResult<bool> {
            self.calls.borrow_mut().push(candidate.to_vec());
            if self.failures.borrow().contains(candidate) {
                return Err(WindowsProbeError::Probe("configured failure".to_owned()));
            }
            Ok(self.found.borrow().contains(candidate))
        }
    }

    struct FakeDirectories {
        current: WindowsDirectoryResult<Option<Vec<u16>>>,
        system: WindowsDirectoryResult<Option<Vec<u16>>>,
        windows: WindowsDirectoryResult<Option<Vec<u16>>>,
    }

    impl FakeDirectories {
        fn none() -> Self {
            Self {
                current: Ok(None),
                system: Ok(None),
                windows: Ok(None),
            }
        }
    }

    impl WindowsDirectoryLookup for FakeDirectories {
        fn current_exe_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>> {
            self.current.clone()
        }

        fn system_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>> {
            self.system.clone()
        }

        fn windows_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>> {
            self.windows.clone()
        }
    }

    struct FakeFullPathName {
        outputs: RefCell<BTreeMap<Vec<u16>, Vec<u16>>>,
        failures: RefCell<BTreeSet<Vec<u16>>>,
        calls: RefCell<Vec<Vec<u16>>>,
    }

    impl FakeFullPathName {
        fn new() -> Self {
            Self {
                outputs: RefCell::new(BTreeMap::new()),
                failures: RefCell::new(BTreeSet::new()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl WindowsFullPathName for FakeFullPathName {
        fn get_full_path_name(&self, path: &[u16]) -> Result<Vec<u16>, WindowsFullPathError> {
            self.calls.borrow_mut().push(path.to_vec());
            if self.failures.borrow().contains(path) {
                return Err(WindowsFullPathError::Lookup(
                    "configured failure".to_owned(),
                ));
            }
            Ok(self
                .outputs
                .borrow()
                .get(path)
                .cloned()
                .unwrap_or_else(|| path.to_vec()))
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    fn wide_terminated(value: &str) -> Vec<u16> {
        value.encode_utf16().chain([0]).collect()
    }

    fn resolve(
        program: &str,
        probe: &FakeCandidateProbe,
        directories: &FakeDirectories,
        full_path: &FakeFullPathName,
        child_path: Option<&str>,
        parent_path: Option<&str>,
    ) -> Result<Vec<u16>, WindowsResolveError> {
        resolve_executable(
            &wide(program),
            probe,
            directories,
            full_path,
            child_path.map(wide).as_deref(),
            parent_path.map(wide).as_deref(),
        )
    }

    #[test]
    fn empty_program_and_trailing_separators_are_invalid() {
        let probe = FakeCandidateProbe::new();
        let directories = FakeDirectories::none();
        let full_path = FakeFullPathName::new();
        for program in ["", r"C:\tool\", r"C:\tool/"] {
            assert_eq!(
                resolve(program, &probe, &directories, &full_path, None, None).unwrap_err(),
                WindowsResolveError::InvalidProgram
            );
        }
    }

    #[test]
    fn nonfilename_exe_is_transformed_without_an_existence_probe() {
        let probe = FakeCandidateProbe::new();
        let directories = FakeDirectories::none();
        let full_path = FakeFullPathName::new();
        assert_eq!(
            resolve(r"C:\tool.EXE", &probe, &directories, &full_path, None, None).unwrap(),
            wide_terminated(r"C:\tool.EXE")
        );
        assert!(probe.calls.borrow().is_empty());
    }

    #[test]
    fn nonfilename_extensionless_path_probes_exe_then_falls_back_to_original() {
        let probe = FakeCandidateProbe::new();
        let directories = FakeDirectories::none();
        let full_path = FakeFullPathName::new();
        assert_eq!(
            resolve(r"C:\tool", &probe, &directories, &full_path, None, None).unwrap(),
            wide_terminated(r"C:\tool")
        );
        assert_eq!(
            probe.calls.borrow().as_slice(),
            [wide_terminated(r"C:\tool.exe")]
        );
        probe.present(r"C:\tool.exe");
        assert_eq!(
            resolve(r"C:\tool", &probe, &directories, &full_path, None, None).unwrap(),
            wide_terminated(r"C:\tool.exe")
        );
    }

    #[test]
    fn search_order_visits_child_current_system_windows_then_parent() {
        let full_path = FakeFullPathName::new();
        for (winner, expected) in [
            (0, r"C:\child\tool.exe"),
            (1, r"C:\current\tool.exe"),
            (2, r"C:\system\tool.exe"),
            (3, r"C:\windows\tool.exe"),
            (4, r"C:\parent\tool.exe"),
        ] {
            let probe = FakeCandidateProbe::new();
            probe.present(expected);
            let directories = FakeDirectories {
                current: Ok(Some(wide(r"C:\current"))),
                system: Ok(Some(wide(r"C:\system"))),
                windows: Ok(Some(wide(r"C:\windows"))),
            };
            let result = resolve(
                "tool",
                &probe,
                &directories,
                &full_path,
                Some(r"C:\child"),
                Some(r"C:\parent"),
            )
            .unwrap();
            assert_eq!(result, wide_terminated(expected), "winner {winner}");
        }
    }

    #[test]
    fn bare_dotted_name_skips_exe_suffix_and_extensionless_name_appends_it() {
        let probe = FakeCandidateProbe::new();
        probe.present(r"C:\child\tool.cmd");
        let directories = FakeDirectories::none();
        let full_path = FakeFullPathName::new();
        assert_eq!(
            resolve(
                "tool.cmd",
                &probe,
                &directories,
                &full_path,
                Some(r"C:\child"),
                None
            )
            .unwrap_err(),
            WindowsResolveError::BatchFileRefused
        );
        assert_eq!(
            probe.calls.borrow()[0],
            wide_terminated(r"C:\child\tool.cmd")
        );
        assert!(!probe.calls.borrow()[0].ends_with(&".exe\0".encode_utf16().collect::<Vec<_>>()));
    }

    #[test]
    fn separator_paths_bypass_catalog_search_and_drive_relative_join_is_prefix_aware() {
        let probe = FakeCandidateProbe::new();
        probe.present(r"C:tool.exe");
        let directories = FakeDirectories {
            current: Ok(Some(wide(r"D:\current"))),
            system: Ok(None),
            windows: Ok(None),
        };
        let full_path = FakeFullPathName::new();
        assert_eq!(
            resolve(
                r"C:\direct.EXE",
                &probe,
                &directories,
                &full_path,
                Some(r"D:\child"),
                None
            )
            .unwrap(),
            wide_terminated(r"C:\direct.EXE")
        );
        assert!(probe.calls.borrow().is_empty());
        assert_eq!(
            resolve(
                "C:tool.exe",
                &probe,
                &directories,
                &full_path,
                Some(r"D:\child"),
                None
            )
            .unwrap(),
            wide_terminated(r"C:tool.exe")
        );
        assert_eq!(
            probe.calls.borrow().last(),
            Some(&wide_terminated(r"C:tool.exe"))
        );
        assert_eq!(
            resolve(
                r"C:.\tool.exe",
                &probe,
                &directories,
                &full_path,
                None,
                None
            )
            .unwrap(),
            wide_terminated(r"C:.\tool.exe")
        );
    }

    #[test]
    fn unavailable_directories_and_candidate_conversion_failures_are_absent() {
        let probe = FakeCandidateProbe::new();
        probe.present(r"C:\system\tool.exe");
        let directories = FakeDirectories {
            current: Err(WindowsDirectoryError::Lookup("unavailable".to_owned())),
            system: Ok(Some(wide(r"C:\system"))),
            windows: Ok(None),
        };
        let full_path = FakeFullPathName::new();
        full_path
            .failures
            .borrow_mut()
            .insert(wide_terminated(r"C:\child\tool.exe"));
        assert_eq!(
            resolve(
                "tool",
                &probe,
                &directories,
                &full_path,
                Some(r"C:\child"),
                None
            )
            .unwrap(),
            wide_terminated(r"C:\system\tool.exe")
        );
    }

    #[test]
    fn valid_nonfollowing_probe_result_wins_before_later_real_candidate() {
        let probe = FakeCandidateProbe::new();
        // The fake's first present response represents any valid attributes, including a
        // directory or broken symlink; the resolver must not classify it before returning it.
        probe.present(r"C:\current\tool.exe");
        probe.present(r"C:\parent\tool.exe");
        let directories = FakeDirectories {
            current: Ok(Some(wide(r"C:\current"))),
            system: Ok(None),
            windows: Ok(None),
        };
        let full_path = FakeFullPathName::new();
        assert_eq!(
            resolve(
                "tool",
                &probe,
                &directories,
                &full_path,
                None,
                Some(r"C:\parent")
            )
            .unwrap(),
            wide_terminated(r"C:\current\tool.exe")
        );
    }

    #[test]
    fn direct_and_fallback_user_path_failures_are_hard_errors_and_exhaustion_is_not_found() {
        let probe = FakeCandidateProbe::new();
        let directories = FakeDirectories::none();
        let full_path = FakeFullPathName::new();
        full_path
            .failures
            .borrow_mut()
            .insert(wide_terminated(r".\direct.EXE"));
        assert!(matches!(
            resolve(
                r".\direct.EXE",
                &probe,
                &directories,
                &full_path,
                None,
                None
            ),
            Err(WindowsResolveError::FullPath(_))
        ));

        let fallback = FakeFullPathName::new();
        fallback
            .failures
            .borrow_mut()
            .insert(wide_terminated(r".\direct"));
        assert!(matches!(
            resolve(r".\direct", &probe, &directories, &fallback, None, None),
            Err(WindowsResolveError::FullPath(_))
        ));
        assert_eq!(
            resolve(
                "missing",
                &probe,
                &directories,
                &FakeFullPathName::new(),
                None,
                None
            )
            .unwrap_err(),
            WindowsResolveError::NotFound
        );
    }

    #[test]
    fn empty_path_components_never_search_a_simulated_current_directory() {
        let probe = FakeCandidateProbe::new();
        // If empty components were used, the resolver would probe `tool.exe` and this would win.
        probe.present("tool.exe");
        let directories = FakeDirectories::none();
        let full_path = FakeFullPathName::new();
        assert_eq!(
            resolve(
                "tool",
                &probe,
                &directories,
                &full_path,
                Some(r#";"";;"#),
                Some(r#";"";;"#),
            )
            .unwrap_err(),
            WindowsResolveError::NotFound
        );
        assert!(probe.calls.borrow().is_empty());
    }
}
