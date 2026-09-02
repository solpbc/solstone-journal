// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owned UTF-16 Windows launch parameters prepared without creating a process.

use std::collections::BTreeMap;
use std::ffi::OsString;

use thiserror::Error;

use super::command_line::{WindowsCommandLineError, make_command_line};
use super::environment::{
    InheritedWindowsEnvironment, WindowsEnvironmentError, WindowsOrdinalCompare,
    WindowsWideEncoder, prepare_environment,
};
use super::resolve::{
    WindowsCandidateProbe, WindowsDirectoryLookup, WindowsResolveError, resolve_executable,
};
use super::user_path::{WindowsFullPathError, WindowsFullPathName, to_user_path};

const NUL: u16 = 0;
const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;
const COLON: u16 = b':' as u16;

#[derive(Debug, Error, PartialEq, Eq)]
pub(super) enum WindowsLaunchPrepError {
    #[error("managed launch command is empty")]
    EmptyCommand,
    #[error("Windows launch input contains an interior NUL")]
    InteriorNul,
    #[error("managed helper current directory must be an absolute Windows path")]
    CurrentDirectoryNotAbsolute,
    #[error("invalid managed helper current directory: {0}")]
    CurrentDirectory(WindowsFullPathError),
    #[error(transparent)]
    Environment(#[from] WindowsEnvironmentError),
    #[error(transparent)]
    Resolve(#[from] WindowsResolveError),
    #[error(transparent)]
    CommandLine(#[from] WindowsCommandLineError),
}

/// Owned, NUL-terminated UTF-16 buffer for a const Win32 string parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WideNulConst {
    units: Vec<u16>,
}

impl WideNulConst {
    fn from_terminated(units: Vec<u16>) -> Result<Self, WindowsLaunchPrepError> {
        validate_exactly_one_terminator(&units)?;
        Ok(Self { units })
    }

    pub(super) fn as_ptr(&self) -> *const u16 {
        self.units.as_ptr()
    }

    pub(super) fn units_len(&self) -> usize {
        self.units.len()
    }

    pub(super) fn units(&self) -> &[u16] {
        &self.units
    }
}

/// Owned, NUL-terminated UTF-16 buffer for Win32's mutable command-line parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WideNulMut {
    units: Vec<u16>,
}

impl WideNulMut {
    fn from_unterminated(mut units: Vec<u16>) -> Result<Self, WindowsLaunchPrepError> {
        if units.contains(&NUL) {
            return Err(WindowsLaunchPrepError::InteriorNul);
        }
        units.push(NUL);
        Ok(Self { units })
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut u16 {
        self.units.as_mut_ptr()
    }

    pub(super) fn units_len(&self) -> usize {
        self.units.len()
    }

    pub(super) fn units(&self) -> &[u16] {
        &self.units
    }
}

/// Owned double-NUL-terminated UTF-16 Windows environment block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WideEnvironmentBlock {
    units: Vec<u16>,
}

impl WideEnvironmentBlock {
    fn from_environment_plan(mut units: Vec<u16>) -> Result<Self, WindowsLaunchPrepError> {
        if units.is_empty() {
            units.extend([NUL, NUL]);
        } else {
            units.push(NUL);
        }
        Ok(Self { units })
    }

    pub(super) fn as_ptr(&self) -> *const u16 {
        self.units.as_ptr()
    }

    #[cfg(windows)]
    pub(super) fn units_len(&self) -> usize {
        self.units.len()
    }

    pub(super) fn units(&self) -> &[u16] {
        &self.units
    }
}

/// The owned buffers needed by a future `CreateProcessW` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WindowsLaunchSpec {
    application_name: WideNulConst,
    command_line: WideNulMut,
    environment: WideEnvironmentBlock,
    current_directory: Option<WideNulConst>,
}

impl WindowsLaunchSpec {
    pub(super) fn application_name(&self) -> &WideNulConst {
        &self.application_name
    }

    pub(super) fn command_line(&self) -> &WideNulMut {
        &self.command_line
    }

    pub(super) fn command_line_mut(&mut self) -> &mut WideNulMut {
        &mut self.command_line
    }

    pub(super) fn environment(&self) -> &WideEnvironmentBlock {
        &self.environment
    }

    /// Set an explicit, absolute Windows current directory for this child.
    ///
    /// The caller must pass an absolute path. Keeping this value in the owned
    /// launch specification prevents it from falling back to a process-global
    /// current directory between validation and `CreateProcessW`.
    pub(super) fn set_current_directory<F: WindowsFullPathName>(
        &mut self,
        current_directory: &[u16],
        full_path: &F,
    ) -> Result<(), WindowsLaunchPrepError> {
        self.current_directory = Some(prepare_windows_current_directory(
            current_directory,
            full_path,
        )?);
        Ok(())
    }

    pub(super) fn current_directory(&self) -> Option<&WideNulConst> {
        self.current_directory.as_ref()
    }
}

/// Injectable OS boundaries used by the preparation layer.
pub(super) struct WindowsLaunchAdapters<'a> {
    pub(super) probe: &'a dyn WindowsCandidateProbe,
    pub(super) directories: &'a dyn WindowsDirectoryLookup,
    pub(super) ordinal: &'a dyn WindowsOrdinalCompare,
    pub(super) inherited_environment: &'a dyn InheritedWindowsEnvironment,
    pub(super) wide_encoder: &'a dyn WindowsWideEncoder,
}

/// Build owned Win32 launch buffers from the narrow journal-managed launch shape.
///
/// Derived from the preparation portion of Rust 1.97.1's Windows `Command::spawn` flow in
/// `std/sys/process/windows.rs`; this orchestration has no single std symbol.
pub(super) fn prepare_windows_launch_spec<F: WindowsFullPathName>(
    command: &[String],
    environment_overrides: &BTreeMap<OsString, OsString>,
    adapters: &WindowsLaunchAdapters<'_>,
    full_path: &F,
) -> Result<WindowsLaunchSpec, WindowsLaunchPrepError> {
    let Some((program, arguments)) = command.split_first() else {
        return Err(WindowsLaunchPrepError::EmptyCommand);
    };
    let program = string_to_wide(program)?;
    let arguments = arguments
        .iter()
        .map(|argument| string_to_wide(argument))
        .collect::<Result<Vec<_>, _>>()?;

    let environment = prepare_environment(
        environment_overrides,
        adapters.ordinal,
        adapters.inherited_environment,
        adapters.wide_encoder,
    )?;
    let application_name = resolve_executable(
        &program,
        adapters.probe,
        adapters.directories,
        full_path,
        environment.child_path.as_deref(),
        environment.parent_path.as_deref(),
    )?;
    let command_line = make_command_line(&program, &arguments)?;

    Ok(WindowsLaunchSpec {
        application_name: WideNulConst::from_terminated(application_name)?,
        command_line: WideNulMut::from_unterminated(command_line)?,
        environment: WideEnvironmentBlock::from_environment_plan(environment.block)?,
        current_directory: None,
    })
}

/// Prepare an explicit helper current directory for `CreateProcessW`.
///
/// Windows drive-relative paths (for example `C:bin`) resolve through mutable
/// per-drive state, and root-relative paths resolve through mutable drive
/// selection. Neither is acceptable for a package-owned helper boundary.
pub(super) fn prepare_windows_current_directory<F: WindowsFullPathName>(
    current_directory: &[u16],
    full_path: &F,
) -> Result<WideNulConst, WindowsLaunchPrepError> {
    if !is_windows_absolute_path(current_directory) {
        return Err(WindowsLaunchPrepError::CurrentDirectoryNotAbsolute);
    }
    let current_directory = to_user_path(current_directory, full_path)
        .map_err(WindowsLaunchPrepError::CurrentDirectory)?;
    WideNulConst::from_terminated(current_directory)
}

fn is_windows_absolute_path(path: &[u16]) -> bool {
    matches!(
        path,
        [drive, COLON, BACKSLASH | SLASH, ..]
            if *drive != BACKSLASH && *drive != SLASH
    ) || matches!(path, [BACKSLASH | SLASH, BACKSLASH | SLASH, _, ..])
}

fn string_to_wide(value: &str) -> Result<Vec<u16>, WindowsLaunchPrepError> {
    let units = value.encode_utf16().collect::<Vec<_>>();
    if units.contains(&NUL) {
        Err(WindowsLaunchPrepError::InteriorNul)
    } else {
        Ok(units)
    }
}

fn validate_exactly_one_terminator(units: &[u16]) -> Result<(), WindowsLaunchPrepError> {
    if units.last() != Some(&NUL) || units[..units.len().saturating_sub(1)].contains(&NUL) {
        return Err(WindowsLaunchPrepError::InteriorNul);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::cmp::Ordering;
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::{OsStr, OsString};

    use super::super::environment::{
        WindowsEnvironmentSourceResult, WindowsOrdinalResult, WindowsWideResult,
    };
    use super::super::resolve::{WindowsDirectoryResult, WindowsProbeResult};
    use super::super::user_path::WindowsFullPathResult;
    use super::*;

    struct FakeProbe {
        present: RefCell<BTreeSet<Vec<u16>>>,
        calls: RefCell<Vec<Vec<u16>>>,
    }

    impl FakeProbe {
        fn new() -> Self {
            Self {
                present: RefCell::new(BTreeSet::new()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn present(&self, path: &str) {
            self.present.borrow_mut().insert(wide_terminated(path));
        }
    }

    impl WindowsCandidateProbe for FakeProbe {
        fn program_exists(&self, candidate: &[u16]) -> WindowsProbeResult<bool> {
            self.calls.borrow_mut().push(candidate.to_vec());
            Ok(self.present.borrow().contains(candidate))
        }
    }

    struct FakeDirectories;

    impl WindowsDirectoryLookup for FakeDirectories {
        fn current_exe_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>> {
            Ok(None)
        }

        fn system_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>> {
            Ok(None)
        }

        fn windows_directory(&self) -> WindowsDirectoryResult<Option<Vec<u16>>> {
            Ok(None)
        }
    }

    struct FakeFullPath;

    impl WindowsFullPathName for FakeFullPath {
        fn get_full_path_name(&self, path: &[u16]) -> WindowsFullPathResult<Vec<u16>> {
            Ok(path.to_vec())
        }
    }

    struct FakeOrdinal;

    impl WindowsOrdinalCompare for FakeOrdinal {
        fn compare_ignore_case(
            &self,
            left: &[u16],
            right: &[u16],
        ) -> WindowsOrdinalResult<Ordering> {
            Ok(ascii_upper(left).cmp(&ascii_upper(right)))
        }
    }

    struct FakeInherited {
        entries: Vec<(OsString, OsString)>,
    }

    impl InheritedWindowsEnvironment for FakeInherited {
        fn snapshot(&self) -> WindowsEnvironmentSourceResult<Vec<(OsString, OsString)>> {
            Ok(self.entries.clone())
        }
    }

    struct FakeWideEncoder;

    impl WindowsWideEncoder for FakeWideEncoder {
        fn encode_wide(&self, value: &OsStr) -> WindowsWideResult<Vec<u16>> {
            Ok(value
                .as_encoded_bytes()
                .iter()
                .map(|byte| u16::from(*byte))
                .collect())
        }
    }

    fn ascii_upper(value: &[u16]) -> Vec<u16> {
        value
            .iter()
            .map(|unit| {
                if (b'a' as u16..=b'z' as u16).contains(unit) {
                    *unit - (b'a' as u16 - b'A' as u16)
                } else {
                    *unit
                }
            })
            .collect()
    }

    fn wide_terminated(value: &str) -> Vec<u16> {
        value.encode_utf16().chain([NUL]).collect()
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    fn adapters<'a>(
        probe: &'a FakeProbe,
        directories: &'a FakeDirectories,
        ordinal: &'a FakeOrdinal,
        inherited_environment: &'a FakeInherited,
        wide_encoder: &'a FakeWideEncoder,
    ) -> WindowsLaunchAdapters<'a> {
        WindowsLaunchAdapters {
            probe,
            directories,
            ordinal,
            inherited_environment,
            wide_encoder,
        }
    }

    fn direct_command() -> Vec<String> {
        vec![r"C:\tool.EXE".to_owned()]
    }

    #[test]
    fn builder_returns_owned_nul_terminated_native_buffers() {
        let probe = FakeProbe::new();
        let directories = FakeDirectories;
        let ordinal = FakeOrdinal;
        let inherited = FakeInherited {
            entries: Vec::new(),
        };
        let encoder = FakeWideEncoder;
        let adapters = adapters(&probe, &directories, &ordinal, &inherited, &encoder);
        let mut spec = prepare_windows_launch_spec(
            &direct_command(),
            &BTreeMap::new(),
            &adapters,
            &FakeFullPath,
        )
        .unwrap();

        assert_eq!(spec.application_name().units().last(), Some(&NUL));
        assert!(
            !spec.application_name().units()[..spec.application_name().units_len() - 1]
                .contains(&NUL)
        );
        assert_eq!(spec.command_line().units().last(), Some(&NUL));
        assert!(!spec.command_line().units()[..spec.command_line().units_len() - 1].contains(&NUL));
        assert_eq!(spec.environment().units(), &[NUL, NUL]);
        assert_eq!(
            spec.application_name().as_ptr(),
            spec.application_name().units().as_ptr()
        );
        assert_eq!(
            spec.environment().as_ptr(),
            spec.environment().units().as_ptr()
        );
        assert_eq!(
            spec.command_line_mut().as_mut_ptr(),
            spec.command_line().units().as_ptr().cast_mut()
        );
    }

    #[test]
    fn environment_entries_have_single_separators_and_one_final_terminator() {
        let probe = FakeProbe::new();
        let directories = FakeDirectories;
        let ordinal = FakeOrdinal;
        let inherited = FakeInherited {
            entries: Vec::new(),
        };
        let encoder = FakeWideEncoder;
        let adapters = adapters(&probe, &directories, &ordinal, &inherited, &encoder);
        let mut overrides = BTreeMap::new();
        overrides.insert(OsString::from("ALPHA"), OsString::from("one"));
        overrides.insert(OsString::from("BETA"), OsString::from("two"));
        let spec =
            prepare_windows_launch_spec(&direct_command(), &overrides, &adapters, &FakeFullPath)
                .unwrap();
        assert_eq!(
            spec.environment().units(),
            &"ALPHA=one\0BETA=two\0\0".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn returned_buffers_outlive_command_and_environment_inputs() {
        let probe = FakeProbe::new();
        let directories = FakeDirectories;
        let ordinal = FakeOrdinal;
        let inherited = FakeInherited {
            entries: Vec::new(),
        };
        let encoder = FakeWideEncoder;
        let adapters = adapters(&probe, &directories, &ordinal, &inherited, &encoder);
        let spec = {
            let command = direct_command();
            let mut overrides = BTreeMap::new();
            overrides.insert(OsString::from("KEY"), OsString::from("value"));
            prepare_windows_launch_spec(&command, &overrides, &adapters, &FakeFullPath).unwrap()
        };
        assert_eq!(
            spec.application_name().units(),
            &wide_terminated(r"C:\tool.EXE")
        );
        assert_eq!(
            spec.environment().units(),
            &"KEY=value\0\0".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn command_line_uses_original_spelling_not_resolved_application_name() {
        let probe = FakeProbe::new();
        probe.present(r"C:\child\tool.exe");
        let directories = FakeDirectories;
        let ordinal = FakeOrdinal;
        let inherited = FakeInherited {
            entries: Vec::new(),
        };
        let encoder = FakeWideEncoder;
        let adapters = adapters(&probe, &directories, &ordinal, &inherited, &encoder);
        let mut overrides = BTreeMap::new();
        overrides.insert(OsString::from("PATH"), OsString::from(r"C:\child"));
        let spec = prepare_windows_launch_spec(
            &["tool".to_owned(), "argument".to_owned()],
            &overrides,
            &adapters,
            &FakeFullPath,
        )
        .unwrap();
        assert_eq!(
            spec.application_name().units(),
            &wide_terminated(r"C:\child\tool.exe")
        );
        assert_eq!(
            spec.command_line().units(),
            &"\"tool\" argument\0".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn narrow_builder_needs_only_command_environment_and_adapters() {
        let probe = FakeProbe::new();
        let directories = FakeDirectories;
        let ordinal = FakeOrdinal;
        let inherited = FakeInherited {
            entries: Vec::new(),
        };
        let encoder = FakeWideEncoder;
        let adapters = adapters(&probe, &directories, &ordinal, &inherited, &encoder);
        assert!(
            prepare_windows_launch_spec(
                &direct_command(),
                &BTreeMap::new(),
                &adapters,
                &FakeFullPath
            )
            .is_ok()
        );
        assert_eq!(
            prepare_windows_launch_spec(&[], &BTreeMap::new(), &adapters, &FakeFullPath),
            Err(WindowsLaunchPrepError::EmptyCommand)
        );
    }

    #[test]
    fn current_directory_is_owned_and_absolute_before_create_process() {
        let probe = FakeProbe::new();
        let directories = FakeDirectories;
        let ordinal = FakeOrdinal;
        let inherited = FakeInherited {
            entries: Vec::new(),
        };
        let encoder = FakeWideEncoder;
        let adapters = adapters(&probe, &directories, &ordinal, &inherited, &encoder);
        let mut spec = prepare_windows_launch_spec(
            &direct_command(),
            &BTreeMap::new(),
            &adapters,
            &FakeFullPath,
        )
        .unwrap();

        spec.set_current_directory(&wide(r"C:\package\bin"), &FakeFullPath)
            .unwrap();

        assert_eq!(
            spec.current_directory().map(WideNulConst::units),
            Some(wide_terminated(r"C:\package\bin").as_slice())
        );
    }

    #[test]
    fn current_directory_rejects_relative_and_interior_nul_paths() {
        for current_directory in ["relative", r"C:relative", r"\root-relative", r"\\"] {
            assert_eq!(
                prepare_windows_current_directory(&wide(current_directory), &FakeFullPath),
                Err(WindowsLaunchPrepError::CurrentDirectoryNotAbsolute),
                "{current_directory}"
            );
        }

        let mut interior_nul = wide(r"C:\package");
        interior_nul.push(NUL);
        interior_nul.extend(wide("bin"));
        assert_eq!(
            prepare_windows_current_directory(&interior_nul, &FakeFullPath),
            Err(WindowsLaunchPrepError::CurrentDirectory(
                WindowsFullPathError::InteriorNul
            ))
        );
    }

    #[test]
    fn current_directory_accepts_unc_and_normalizes_verbatim_absolute_forms() {
        for (current_directory, expected) in [
            (r"\\server\share\package", r"\\server\share\package"),
            (r"\\?\C:\package", r"C:\package"),
        ] {
            assert_eq!(
                prepare_windows_current_directory(&wide(current_directory), &FakeFullPath)
                    .unwrap()
                    .units(),
                wide_terminated(expected),
                "{current_directory}"
            );
        }
    }

    #[test]
    fn caller_path_is_used_for_resolution_and_encoded_into_the_environment() {
        let probe = FakeProbe::new();
        probe.present(r"C:\caller\tool.exe");
        let directories = FakeDirectories;
        let ordinal = FakeOrdinal;
        let inherited = FakeInherited {
            entries: Vec::new(),
        };
        let encoder = FakeWideEncoder;
        let adapters = adapters(&probe, &directories, &ordinal, &inherited, &encoder);
        let mut overrides = BTreeMap::new();
        overrides.insert(OsString::from("PATH"), OsString::from(r"C:\caller"));
        let spec =
            prepare_windows_launch_spec(&["tool".to_owned()], &overrides, &adapters, &FakeFullPath)
                .unwrap();
        assert_eq!(
            probe.calls.borrow().as_slice(),
            [wide_terminated(r"C:\caller\tool.exe")]
        );
        assert_eq!(
            spec.environment().units(),
            &"PATH=C:\\caller\0\0".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn omitted_caller_path_still_searches_inherited_parent_path_last() {
        let probe = FakeProbe::new();
        probe.present(r"C:\parent\tool.exe");
        let directories = FakeDirectories;
        let ordinal = FakeOrdinal;
        let inherited = FakeInherited {
            entries: vec![(OsString::from("Path"), OsString::from(r"C:\parent"))],
        };
        let encoder = FakeWideEncoder;
        let adapters = adapters(&probe, &directories, &ordinal, &inherited, &encoder);
        let spec = prepare_windows_launch_spec(
            &["tool".to_owned()],
            &BTreeMap::new(),
            &adapters,
            &FakeFullPath,
        )
        .unwrap();
        assert_eq!(
            spec.application_name().units(),
            &wide_terminated(r"C:\parent\tool.exe")
        );
        assert_eq!(
            probe.calls.borrow().as_slice(),
            [wide_terminated(r"C:\parent\tool.exe")]
        );
    }
}
