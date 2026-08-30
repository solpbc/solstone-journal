// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Conservative no-owned-process facade for non-Unix targets.

use std::fmt;
use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Output};
use std::time::{Duration, Instant};

use super::{
    BoxedTerminateFn, CommandLaunchRequest, DescendantObservationFailure,
    DescendantTerminationOutcome, Disposition, HostedLaunchProvenance, InspectResult,
    InstanceCensus, InstanceVerdict, LaunchError, LaunchedProcessIdentity, ManagedLaunchRequest,
    ProcessInstance, ProcessInstanceSource, SignalKind, SpawnError, SpawnOptions,
    SystemProcessInstanceSource, TerminationError, TerminationOutcome,
    require_managed_process_capability,
};

#[cfg(any(windows, test))]
mod command_line;
#[cfg(any(windows, test))]
mod environment;
#[cfg(any(windows, test))]
mod identity;
#[cfg(any(windows, test))]
mod launch_spec;
#[cfg(any(windows, test))]
mod path_list;
#[cfg(any(windows, test))]
mod resolve;
#[cfg(any(windows, test))]
mod user_path;

#[cfg(windows)]
pub(crate) use identity::current_windows_process_instance;
#[cfg(all(windows, feature = "test-hooks"))]
pub use identity::windows_filetime_value_from_raw_for_test;

#[cfg(all(windows, feature = "test-hooks"))]
pub fn windows_launch_environment_preparation_receipt_for_test() -> Result<(), String> {
    use std::cmp::Ordering;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    use environment::{
        SystemInheritedWindowsEnvironment, SystemWindowsOrdinalCompare, SystemWindowsWideEncoder,
        WindowsOrdinalCompare, WindowsWideEncoder, prepare_environment,
    };

    let lone_surrogate = OsString::from_wide(&[0xd800]);
    let encoder = SystemWindowsWideEncoder;
    let encoded = encoder
        .encode_wide(lone_surrogate.as_os_str())
        .map_err(|error| error.to_string())?;
    if encoded != [0xd800] {
        return Err("production OsString encoding did not preserve a lone surrogate".to_owned());
    }

    let ordinal = SystemWindowsOrdinalCompare;
    let upper = "\u{00c5}ngstr\u{00f6}m".encode_utf16().collect::<Vec<_>>();
    let lower = "\u{00e5}ngstr\u{00f6}m".encode_utf16().collect::<Vec<_>>();
    if ordinal
        .compare_ignore_case(&upper, &lower)
        .map_err(|error| error.to_string())?
        != Ordering::Equal
    {
        return Err("CompareStringOrdinal did not fold a non-ASCII case pair".to_owned());
    }

    let key = OsString::from("SOLSTONE_LONE_SURROGATE_RECEIPT");
    let key_wide = key.encode_wide().collect::<Vec<_>>();
    let mut overrides = BTreeMap::new();
    overrides.insert(key, lone_surrogate);
    let plan = prepare_environment(
        &overrides,
        &ordinal,
        &SystemInheritedWindowsEnvironment,
        &encoder,
    )
    .map_err(|error| error.to_string())?;
    let mut expected = key_wide;
    expected.extend([b'=' as u16, 0xd800, 0]);
    if !plan
        .block
        .windows(expected.len())
        .any(|window| window == expected)
    {
        return Err("production environment block lost the lone surrogate override".to_owned());
    }
    Ok(())
}

#[cfg(all(windows, feature = "test-hooks"))]
pub fn windows_launch_path_preparation_receipt_for_test() -> Result<(), String> {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;

    use environment::{
        SystemInheritedWindowsEnvironment, SystemWindowsOrdinalCompare, SystemWindowsWideEncoder,
    };
    use launch_spec::{WindowsLaunchAdapters, prepare_windows_launch_spec};
    use resolve::{
        SystemWindowsCandidateProbe, SystemWindowsDirectoryLookup, WindowsCandidateProbe,
    };
    use user_path::{SystemWindowsFullPathName, WindowsFullPathName, get_long_path};

    let split = path_list::split_windows_paths(
        &r#"C:\one;"C:\two;three";C:\four"#.encode_utf16().collect::<Vec<_>>(),
    );
    let expected_split = [r"C:\one", r"C:\two;three", r"C:\four"]
        .map(|value| value.encode_utf16().collect::<Vec<_>>());
    if split != expected_split {
        return Err("production Windows PATH parser mishandled a quoted semicolon".to_owned());
    }

    let full_path = SystemWindowsFullPathName;
    let long_relative = format!("{}tool.exe", r"segment\".repeat(40));
    let terminated = long_relative
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let expanded = full_path
        .get_full_path_name(&terminated)
        .map_err(|error| error.to_string())?;
    if expanded.len() <= 260 {
        return Err("GetFullPathNameW receipt did not exercise buffer growth".to_owned());
    }
    let user_path =
        get_long_path(terminated, false, &full_path).map_err(|error| error.to_string())?;
    let verbatim_prefix = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    if !user_path.starts_with(&verbatim_prefix) {
        return Err("long Windows user path did not receive a verbatim prefix".to_owned());
    }

    let current_exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let current_exe_wide = current_exe
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let probe = SystemWindowsCandidateProbe;
    if !probe
        .program_exists(&current_exe_wide)
        .map_err(|error| error.to_string())?
    {
        return Err("GetFileAttributesW did not observe the running executable".to_owned());
    }
    let missing = current_exe.with_extension(format!("u2-missing-{}", std::process::id()));
    let missing_wide = missing
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    if probe
        .program_exists(&missing_wide)
        .map_err(|error| error.to_string())?
    {
        return Err("GetFileAttributesW receipt's absent control unexpectedly exists".to_owned());
    }

    let adapters = WindowsLaunchAdapters {
        probe: &probe,
        directories: &SystemWindowsDirectoryLookup,
        ordinal: &SystemWindowsOrdinalCompare,
        inherited_environment: &SystemInheritedWindowsEnvironment,
        wide_encoder: &SystemWindowsWideEncoder,
    };
    let command = vec!["cmd".to_owned(), "quoted argument".to_owned()];
    let spec = prepare_windows_launch_spec(
        &command,
        &BTreeMap::<OsString, OsString>::new(),
        &adapters,
        &full_path,
    )
    .map_err(|error| error.to_string())?;
    if spec.application_name().units().last() != Some(&0)
        || spec.application_name().units_len() < "cmd.exe\0".encode_utf16().count()
        || spec.command_line().units().last() != Some(&0)
        || !spec.environment().units().ends_with(&[0, 0])
    {
        return Err(
            "production PATH resolution did not return complete owned launch buffers".to_owned(),
        );
    }
    Ok(())
}

#[cfg(not(unix))]
impl ProcessInstanceSource for SystemProcessInstanceSource {
    fn inspect(&self, _pid: u32) -> InspectResult {
        #[cfg(target_os = "ios")]
        {
            return InspectResult::Unverifiable;
        }
        #[cfg(not(target_os = "ios"))]
        InspectResult::Unverifiable
    }

    fn census(&self) -> InstanceCensus {
        #[cfg(target_os = "ios")]
        {
            return InstanceCensus::Incomplete(Vec::new());
        }
        #[cfg(not(target_os = "ios"))]
        InstanceCensus::Incomplete(Vec::new())
    }

    fn observe(&self, expected: &ProcessInstance) -> InstanceVerdict {
        #[cfg(windows)]
        {
            return identity::verdict_from_windows_probe(
                expected,
                identity::sample_windows_process_with(
                    &identity::SystemWindowsProcessApi,
                    expected.pid,
                ),
            );
        }
        #[cfg(not(windows))]
        {
            let _ = expected;
            InstanceVerdict::Unverifiable
        }
    }
}

/// No Windows-owned process can exist until a later implementation supplies
/// a birth-bound containment primitive.
pub enum ManagedProcess {}

impl ManagedProcess {
    pub fn spawn(_cmd: Vec<String>, _options: SpawnOptions) -> Result<Self, SpawnError> {
        require_managed_process_capability()
            .map_err(|needed| SpawnError::CapabilityUnavailable { needed })?;
        unreachable!("non-Unix managed-process capability unexpectedly available")
    }

    pub fn spawn_exact(_cmd: Vec<String>, _options: SpawnOptions) -> Result<Self, SpawnError> {
        require_managed_process_capability()
            .map_err(|needed| SpawnError::CapabilityUnavailable { needed })?;
        unreachable!("non-Unix managed-process capability unexpectedly available")
    }

    pub fn pid(&self) -> u32 {
        match *self {}
    }

    pub fn pgid(&self) -> io::Result<i32> {
        match *self {}
    }

    pub fn name(&self) -> &str {
        match *self {}
    }

    pub fn cmd(&self) -> &[String] {
        match *self {}
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        match *self {}
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        match *self {}
    }

    pub fn terminate(
        &mut self,
        _timeout: Duration,
    ) -> Result<TerminationOutcome, TerminationError> {
        match *self {}
    }

    pub fn terminate_exact(
        &mut self,
        _timeout: Duration,
    ) -> Result<TerminationOutcome, TerminationError> {
        match *self {}
    }

    pub fn terminate_exact_until(
        &mut self,
        _deadline: Instant,
    ) -> Result<TerminationOutcome, TerminationError> {
        match *self {}
    }

    pub fn detach_after_bounded_shutdown(&mut self) {
        match *self {}
    }

    pub fn signal_exact(&mut self, _signal: SignalKind) -> Result<(), TerminationError> {
        match *self {}
    }

    pub fn log_path(&self) -> std::path::PathBuf {
        match *self {}
    }

    pub fn cleanup(&mut self) {
        match *self {}
    }

    pub fn cleanup_until(&mut self, _deadline: Instant) -> bool {
        match *self {}
    }
}

pub enum LaunchAuthority {}

impl fmt::Debug for LaunchAuthority {
    fn fmt(&self, _formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl LaunchAuthority {
    pub fn pid(&self) -> u32 {
        match *self {}
    }

    pub fn disposition(&self) -> &Disposition {
        match *self {}
    }

    pub fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
        match *self {}
    }

    pub fn bind_exact_identity(
        &mut self,
        _identity: LaunchedProcessIdentity,
    ) -> Result<(), LaunchError> {
        match *self {}
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        match *self {}
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        match *self {}
    }

    pub fn terminate(&mut self, _timeout: Duration) -> Result<(), LaunchError> {
        match *self {}
    }

    pub fn terminate_exact(&mut self, _timeout: Duration) -> Result<(), LaunchError> {
        match *self {}
    }

    pub(crate) fn terminate_exact_until(&mut self, _deadline: Instant) -> Result<(), LaunchError> {
        match *self {}
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        match *self {}
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        match *self {}
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        match *self {}
    }

    pub fn wait_with_output(self) -> Result<Output, LaunchError> {
        match self {}
    }

    pub fn cleanup(&mut self) {
        match *self {}
    }

    pub fn relinquish_explicitly_unowned(self) -> Result<(), LaunchError> {
        match self {}
    }

    pub fn into_managed(self) -> Result<ManagedProcess, LaunchError> {
        match self {}
    }

    pub(crate) fn cleanup_until(&mut self, _deadline: Instant) -> bool {
        match *self {}
    }

    pub(crate) fn detach_after_bounded_shutdown(&mut self) {
        match *self {}
    }
}

pub fn launch<F>(
    _disposition: Disposition,
    _spawn: F,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> io::Result<Child>,
{
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_managed<F>(
    _disposition: Disposition,
    _spawn: F,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> Result<ManagedProcess, SpawnError>,
{
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_with<F, Cap, Conf>(
    _disposition: Disposition,
    _spawn: F,
    _terminate_fn: BoxedTerminateFn,
    _capability: Cap,
    _confirm: Conf,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> io::Result<Child>,
    Cap: FnOnce(&Disposition) -> Result<(), LaunchError>,
    Conf: FnOnce(u32) -> io::Result<()>,
{
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_managed_with<F, Cap>(
    _disposition: Disposition,
    _spawn: F,
    _capability: Cap,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> Result<ManagedProcess, SpawnError>,
    Cap: FnOnce(&Disposition) -> Result<(), LaunchError>,
{
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_command(
    _disposition: Disposition,
    _request: CommandLaunchRequest,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_command_hosted(
    _disposition: Disposition,
    _request: CommandLaunchRequest,
    _provenance: HostedLaunchProvenance,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_managed_request(
    _disposition: Disposition,
    _request: ManagedLaunchRequest,
) -> Result<LaunchAuthority, LaunchError> {
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_managed_hosted(
    _disposition: Disposition,
    _request: ManagedLaunchRequest,
    _provenance: HostedLaunchProvenance,
) -> Result<LaunchAuthority, LaunchError> {
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn terminate_descendants_exact<F>(
    _root: ProcessInstance,
    _owner_uid: u32,
    _timeout: Duration,
    _source: &dyn ProcessInstanceSource,
    stop_service: F,
) -> Result<DescendantTerminationOutcome, DescendantObservationFailure>
where
    F: FnOnce(),
{
    stop_service();
    Err(DescendantObservationFailure::CensusIncomplete)
}

pub fn terminate(
    _child: &mut Child,
    _timeout: Duration,
) -> Result<TerminationOutcome, TerminationError> {
    Err(TerminationError::DescendantCoverageUnavailable)
}

pub fn terminate_exact_instance(
    _child: &mut Child,
    _expected: ProcessInstance,
    _timeout: Duration,
    _source: &dyn ProcessInstanceSource,
) -> Result<TerminationOutcome, TerminationError> {
    Err(TerminationError::DescendantCoverageUnavailable)
}

pub fn signal_exact_instance(
    _expected: ProcessInstance,
    _signal: SignalKind,
    _source: &dyn ProcessInstanceSource,
) -> Result<(), TerminationError> {
    Err(TerminationError::DescendantCoverageUnavailable)
}

pub fn apply_parent_death_kill(_command: &mut Command) {}
