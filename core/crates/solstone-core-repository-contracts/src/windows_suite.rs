// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::ci::{Registry, Suite};

pub const WINDOWS_COVERAGE_SCHEMA: &str = "solstone.journal.win-suite-coverage.v1";
pub const OWNER_RAIL_LEASE_SCHEMA: &str = "solstone.journal.win-owner-rail.lease.v1";
pub const OWNER_RAIL_RESULT_SCHEMA: &str = "solstone.journal.win-owner-rail.result.v1";

const KNOWN_REQUIRES: &[&str] = &[
    "native-restic",
    "native-rclone",
    "cloud-files-api",
    "onnx-runtime",
    "rfdetr-pin",
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowsPolicy {
    pub satisfaction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opt_in: Option<WindowsOptIn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marker_patterns: Vec<String>,
    #[serde(default)]
    pub body: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<WindowsReceiptTest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unlaunched: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledgement: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowsOptIn {
    pub control: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowsReceiptTest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub line_patterns: Vec<String>,
}

pub fn validate_windows_policy(suite: &Suite, policy: &WindowsPolicy) -> Result<(), String> {
    if !suite.platforms.iter().any(|p| p == "windows") {
        return Err(format!(
            "suite {}: windows policy declared but platforms does not include windows",
            suite.id
        ));
    }

    if let Some(opt_in) = &policy.opt_in {
        let control_regex = Regex::new(r"^[A-Z][A-Z0-9_]+$").map_err(|e| e.to_string())?;
        if !control_regex.is_match(&opt_in.control) {
            return Err(format!(
                "suite {}: invalid opt-in control variable name {:?}",
                suite.id, opt_in.control
            ));
        }
        for req in &opt_in.requires {
            if !KNOWN_REQUIRES.contains(&req.as_str()) {
                return Err(format!("suite {}: unknown opt-in require {req}", suite.id));
            }
        }
    }

    for pattern in &policy.marker_patterns {
        Regex::new(pattern).map_err(|err| {
            format!(
                "suite {}: invalid marker regex pattern {:?}: {err}",
                suite.id, pattern
            )
        })?;
    }

    match policy.satisfaction.as_str() {
        "cargo" => {
            if policy.body {
                return Err(format!(
                    "suite {}: cargo satisfaction cannot specify body = true",
                    suite.id
                ));
            }
            if !policy.tests.is_empty() {
                return Err(format!(
                    "suite {}: cargo satisfaction cannot specify tests",
                    suite.id
                ));
            }
            if !policy.unlaunched.is_empty() {
                return Err(format!(
                    "suite {}: cargo satisfaction cannot specify unlaunched",
                    suite.id
                ));
            }
        }
        "receipts" => {
            if policy.tests.is_empty() {
                return Err(format!(
                    "suite {}: receipts satisfaction requires non-empty tests",
                    suite.id
                ));
            }
            let mut seen = BTreeSet::new();
            for test in &policy.tests {
                if !seen.insert(test.name.clone()) {
                    return Err(format!(
                        "suite {}: duplicate receipt test name {}",
                        suite.id, test.name
                    ));
                }
                for pattern in &test.line_patterns {
                    Regex::new(pattern).map_err(|err| {
                        format!(
                            "suite {} test {}: invalid line regex pattern {:?}: {err}",
                            suite.id, test.name, pattern
                        )
                    })?;
                }
            }
            for unl in &policy.unlaunched {
                if seen.contains(unl) {
                    return Err(format!(
                        "suite {}: receipt test {unl} is also declared unlaunched",
                        suite.id
                    ));
                }
            }
        }
        "ordinary-owner" => {
            if !policy.tests.is_empty()
                || !policy.markers.is_empty()
                || !policy.marker_patterns.is_empty()
                || !policy.args.is_empty()
                || policy.acknowledgement.is_some()
                || policy.opt_in.is_some()
                || !policy.unlaunched.is_empty()
                || policy.body
            {
                return Err(format!(
                    "suite {}: ordinary-owner satisfaction cannot specify extra policy fields",
                    suite.id
                ));
            }
        }
        other => {
            return Err(format!("suite {}: unknown satisfaction {other}", suite.id));
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowsDisposition {
    Passed,
    OptedOut,
    Blocked,
    Failed,
    TimedOut,
    Unexecuted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanStatus {
    Ready,
    OptedOut,
    Blocked(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsPlannedCommand {
    pub is_receipt: bool,
    pub test_name: Option<String>,
    pub argv: Vec<String>,
    pub expected_markers: Vec<String>,
    pub expected_marker_patterns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsPlanItem {
    pub id: String,
    pub package: String,
    pub target: String,
    pub required_features: Vec<String>,
    pub timeout_seconds: u64,
    pub serial_group: Option<String>,
    pub default_full: bool,
    pub status: PlanStatus,
    pub commands: Vec<WindowsPlannedCommand>,
    pub policy: Option<WindowsPolicy>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsPlan {
    pub items: Vec<WindowsPlanItem>,
}

pub fn plan_windows(
    registry: &Registry,
    controls: &BTreeMap<String, bool>,
    probes: &BTreeMap<String, bool>,
) -> Result<WindowsPlan, String> {
    let mut items = Vec::new();

    for suite in &registry.suites {
        if !suite.platforms.iter().any(|p| p == "windows") {
            continue;
        }

        let timeout_seconds = registry
            .timeouts
            .get(&suite.timeout)
            .copied()
            .ok_or_else(|| {
                format!(
                    "suite {}: timeout class {:?} not found in registry",
                    suite.id, suite.timeout
                )
            })?;

        let status = if let Some(policy) = &suite.windows {
            if let Some(opt_in) = &policy.opt_in {
                let enabled = controls.get(&opt_in.control).copied().unwrap_or(false);
                if !enabled {
                    PlanStatus::OptedOut
                } else {
                    let mut missing = Vec::new();
                    for req in &opt_in.requires {
                        let present = probes.get(req).copied().unwrap_or(false);
                        if !present {
                            missing.push(req.clone());
                        }
                    }
                    if missing.is_empty() {
                        PlanStatus::Ready
                    } else {
                        PlanStatus::Blocked(missing.join(", "))
                    }
                }
            } else {
                PlanStatus::Ready
            }
        } else {
            PlanStatus::Ready
        };

        let mut commands = Vec::new();
        if status == PlanStatus::Ready {
            if let Some(policy) = &suite.windows {
                match policy.satisfaction.as_str() {
                    "ordinary-owner" => {
                        let mut argv = vec![
                            "cargo".to_owned(),
                            "test".to_owned(),
                            "--manifest-path".to_owned(),
                            r"core\Cargo.toml".to_owned(),
                            "--locked".to_owned(),
                            "-p".to_owned(),
                            suite.package.clone(),
                            "--test".to_owned(),
                            suite.target.clone(),
                        ];
                        if !suite.required_features.is_empty() {
                            argv.push("--features".to_owned());
                            argv.push(suite.required_features.join(","));
                        }
                        argv.push("--".to_owned());
                        argv.push("--nocapture".to_owned());
                        commands.push(WindowsPlannedCommand {
                            is_receipt: false,
                            test_name: None,
                            argv,
                            expected_markers: vec![],
                            expected_marker_patterns: vec![],
                        });
                    }
                    "receipts" => {
                        if policy.body {
                            let mut argv = vec![
                                "cargo".to_owned(),
                                "test".to_owned(),
                                "--manifest-path".to_owned(),
                                "core/Cargo.toml".to_owned(),
                                "--locked".to_owned(),
                                "-p".to_owned(),
                                suite.package.clone(),
                                "--test".to_owned(),
                                suite.target.clone(),
                            ];
                            if !suite.required_features.is_empty() {
                                argv.push("--features".to_owned());
                                argv.push(suite.required_features.join(","));
                            }
                            argv.push("--".to_owned());
                            argv.push("--nocapture".to_owned());
                            argv.extend(policy.args.clone());

                            commands.push(WindowsPlannedCommand {
                                is_receipt: false,
                                test_name: None,
                                argv,
                                expected_markers: policy.markers.clone(),
                                expected_marker_patterns: policy.marker_patterns.clone(),
                            });
                        }
                        for test in &policy.tests {
                            let mut argv = vec![
                                "cargo".to_owned(),
                                "test".to_owned(),
                                "--manifest-path".to_owned(),
                                "core/Cargo.toml".to_owned(),
                                "--locked".to_owned(),
                                "-p".to_owned(),
                                suite.package.clone(),
                                "--test".to_owned(),
                                suite.target.clone(),
                            ];
                            if !suite.required_features.is_empty() {
                                argv.push("--features".to_owned());
                                argv.push(suite.required_features.join(","));
                            }
                            argv.push("--".to_owned());
                            argv.push("--ignored".to_owned());
                            argv.push("--exact".to_owned());
                            argv.push(test.name.clone());
                            argv.push("--show-output".to_owned());

                            commands.push(WindowsPlannedCommand {
                                is_receipt: true,
                                test_name: Some(test.name.clone()),
                                argv,
                                expected_markers: test.lines.clone(),
                                expected_marker_patterns: test.line_patterns.clone(),
                            });
                        }
                    }
                    "cargo" => {
                        let mut argv = vec![
                            "cargo".to_owned(),
                            "test".to_owned(),
                            "--manifest-path".to_owned(),
                            "core/Cargo.toml".to_owned(),
                            "--locked".to_owned(),
                            "-p".to_owned(),
                            suite.package.clone(),
                            "--test".to_owned(),
                            suite.target.clone(),
                        ];
                        if !suite.required_features.is_empty() {
                            argv.push("--features".to_owned());
                            argv.push(suite.required_features.join(","));
                        }
                        argv.push("--".to_owned());
                        argv.push("--nocapture".to_owned());
                        argv.extend(policy.args.clone());

                        commands.push(WindowsPlannedCommand {
                            is_receipt: false,
                            test_name: None,
                            argv,
                            expected_markers: policy.markers.clone(),
                            expected_marker_patterns: policy.marker_patterns.clone(),
                        });
                    }
                    _ => {}
                }
            } else {
                let mut argv = vec![
                    "cargo".to_owned(),
                    "test".to_owned(),
                    "--manifest-path".to_owned(),
                    "core/Cargo.toml".to_owned(),
                    "--locked".to_owned(),
                    "-p".to_owned(),
                    suite.package.clone(),
                    "--test".to_owned(),
                    suite.target.clone(),
                ];
                if !suite.required_features.is_empty() {
                    argv.push("--features".to_owned());
                    argv.push(suite.required_features.join(","));
                }
                argv.push("--".to_owned());
                argv.push("--nocapture".to_owned());

                commands.push(WindowsPlannedCommand {
                    is_receipt: false,
                    test_name: None,
                    argv,
                    expected_markers: vec![],
                    expected_marker_patterns: vec![],
                });
            }
        }

        items.push(WindowsPlanItem {
            id: suite.id.clone(),
            package: suite.package.clone(),
            target: suite.target.clone(),
            required_features: suite.required_features.clone(),
            timeout_seconds,
            serial_group: suite.serial_group.clone(),
            default_full: suite.default_full,
            status,
            commands,
            policy: suite.windows.clone(),
        });
    }

    if items.is_empty() {
        return Err("windows selection matched zero suites".to_owned());
    }

    Ok(WindowsPlan { items })
}

pub fn resolve_default_controls() -> Result<BTreeMap<String, bool>, String> {
    let mut controls = BTreeMap::new();
    for name in [
        "JOURNAL_WIN_CI_RUN_BACKUP",
        "JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST",
        "JOURNAL_WIN_CI_RUN_RFDETR",
        "JOURNAL_WIN_CI_RUN_ONNX_BINDING",
    ] {
        match std::env::var(name) {
            Ok(val) => {
                if val == "1" {
                    controls.insert(name.to_owned(), true);
                } else if val == "0" || val.is_empty() {
                    controls.insert(name.to_owned(), false);
                } else {
                    return Err(format!(
                        "invalid control variable {name}={val:?}; must be 0 or 1"
                    ));
                }
            }
            Err(_) => {
                controls.insert(name.to_owned(), false);
            }
        }
    }
    Ok(controls)
}

pub fn resolve_default_probes() -> BTreeMap<String, bool> {
    let mut probes = BTreeMap::new();

    // native-restic & native-rclone
    let restic_ok = std::env::var("SOLSTONE_NATIVE_RESTIC")
        .ok()
        .map(|p| is_existing_absolute_file(&p))
        .unwrap_or(false);
    probes.insert("native-restic".to_owned(), restic_ok);

    let rclone_ok = std::env::var("SOLSTONE_NATIVE_RCLONE")
        .ok()
        .map(|p| is_existing_absolute_file(&p))
        .unwrap_or(false);
    probes.insert("native-rclone".to_owned(), rclone_ok);

    // rfdetr-pin
    let rfdetr_ok = std::env::var("SOLSTONE_WINDOWS_TEST_PIN")
        .ok()
        .map(|p| is_existing_absolute_file(&p))
        .unwrap_or(false);
    probes.insert("rfdetr-pin".to_owned(), rfdetr_ok);

    // onnx-runtime
    let onnx_pin_ok = std::env::var("SOLSTONE_WINDOWS_TEST_PIN")
        .ok()
        .map(|p| is_existing_absolute_file(&p))
        .unwrap_or(false);
    let onnx_dylib_ok = std::env::var("ORT_DYLIB_PATH")
        .ok()
        .map(|p| is_existing_absolute_file(&p))
        .unwrap_or(false);
    probes.insert("onnx-runtime".to_owned(), onnx_pin_ok && onnx_dylib_ok);

    // cloud-files-api
    #[cfg(windows)]
    {
        let cloud_ok = probe_windows_cloud_api();
        probes.insert("cloud-files-api".to_owned(), cloud_ok);
    }
    #[cfg(not(windows))]
    {
        probes.insert("cloud-files-api".to_owned(), false);
    }

    probes
}

fn is_existing_absolute_file(path_str: &str) -> bool {
    let path = Path::new(path_str);
    if !path.is_absolute() {
        return false;
    }
    path.is_file()
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn probe_windows_cloud_api() -> bool {
    use std::ffi::CString;
    unsafe extern "system" {
        fn LoadLibraryA(lpLibFileName: *const i8) -> *mut std::ffi::c_void;
        fn GetProcAddress(
            hModule: *mut std::ffi::c_void,
            lpProcName: *const i8,
        ) -> *mut std::ffi::c_void;
        fn FreeLibrary(hLibModule: *mut std::ffi::c_void) -> i32;
    }
    let lib_name = CString::new("cldapi.dll").unwrap();
    let proc_name = CString::new("CfRegisterSyncRoot").unwrap();
    // SAFETY: both names are NUL-terminated and the DLL handle is freed only
    // after the symbol-presence query; no function pointer is invoked.
    unsafe {
        let handle = LoadLibraryA(lib_name.as_ptr());
        if handle.is_null() {
            return false;
        }
        let proc = GetProcAddress(handle, proc_name.as_ptr());
        let found = !proc.is_null();
        FreeLibrary(handle);
        found
    }
}

pub trait ProcessTree {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn kill_tree(&mut self) -> std::io::Result<()>;
}

pub struct StandardProcessTree {
    child: Child,
    #[cfg(windows)]
    _job: windows_job::JobHandle,
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_job {
    // SAFETY: handles are uniquely owned here; buffers are zeroed Win32 PODs
    // with exact sizes. A borrowed Child handle stays alive through each call.
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    pub struct JobHandle(HANDLE);
    impl Drop for JobHandle {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() {
                    CloseHandle(self.0);
                }
            }
        }
    }

    pub fn create_kill_on_close_job() -> std::io::Result<JobHandle> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let res = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if res == 0 {
                CloseHandle(job);
                return Err(std::io::Error::last_os_error());
            }
            Ok(JobHandle(job))
        }
    }

    pub fn assign_child(job: &JobHandle, child: &std::process::Child) -> std::io::Result<()> {
        unsafe {
            let res = AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE);
            if res == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
    }

    pub fn terminate_job(job: &JobHandle) -> std::io::Result<()> {
        unsafe {
            let res = TerminateJobObject(job.0, 1);
            if res == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                if QueryInformationJobObject(
                    job.0,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut _,
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                if info.ActiveProcesses == 0 {
                    return Ok(());
                }
                if std::time::Instant::now() >= deadline {
                    return Err(std::io::Error::other(format!(
                        "job cleanup left {} active processes",
                        info.ActiveProcesses
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    pub fn resume_initial_thread(pid: u32) -> std::io::Result<()> {
        // Command creates the process suspended. Its initial thread cannot
        // spawn descendants before the job assignment has succeeded.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut present = Thread32First(snapshot, &mut entry);
            let mut thread_id = None;
            while present != 0 {
                if entry.th32OwnerProcessID == pid {
                    thread_id = Some(entry.th32ThreadID);
                    break;
                }
                entry.dwSize = std::mem::size_of_val(&entry) as u32;
                present = Thread32Next(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            let thread_id = thread_id
                .ok_or_else(|| std::io::Error::other("suspended process has no initial thread"))?;
            let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id);
            if thread.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let previous = ResumeThread(thread);
            let error = if previous == u32::MAX {
                Some(std::io::Error::last_os_error())
            } else if previous != 1 {
                Some(std::io::Error::other(
                    "unexpected initial thread suspend count",
                ))
            } else {
                None
            };
            CloseHandle(thread);
            error.map_or(Ok(()), Err)
        }
    }
}

impl ProcessTree for StandardProcessTree {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn kill_tree(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            windows_job::terminate_job(&self._job)?;
            self.child.wait()?;
            Ok(())
        }
        #[cfg(unix)]
        {
            use nix::sys::signal::{Signal, killpg};
            use nix::unistd::Pid;
            let pid = self.child.id() as i32;
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
            let _ = self.child.kill();
            Ok(())
        }
        #[cfg(not(any(windows, unix)))]
        {
            self.child.kill()
        }
    }
}

pub fn spawn_process_tree(command: &mut Command) -> std::io::Result<StandardProcessTree> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
        let job = windows_job::create_kill_on_close_job()?;
        let mut child = command.creation_flags(CREATE_SUSPENDED).spawn()?;
        if let Err(err) = windows_job::assign_child(&job, &child)
            .and_then(|()| windows_job::resume_initial_thread(child.id()))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
        Ok(StandardProcessTree { child, _job: job })
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        let child = command.spawn()?;
        Ok(StandardProcessTree { child })
    }
    #[cfg(not(any(windows, unix)))]
    {
        let child = command.spawn()?;
        Ok(StandardProcessTree { child })
    }
}

pub fn wait_bounded(
    tree: &mut dyn ProcessTree,
    timeout: Duration,
    mut now: impl FnMut() -> Instant,
    park: impl Fn(Duration),
) -> Result<ExitStatus, String> {
    let start = now();
    loop {
        match tree.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if now().duration_since(start) >= timeout {
                    tree.kill_tree()
                        .map_err(|error| format!("timed out; tree cleanup failed: {error}"))?;
                    return Err("timed out".to_owned());
                }
                park(Duration::from_millis(50));
            }
            Err(err) => {
                tree.kill_tree().map_err(|error| {
                    format!("process wait error: {err}; tree cleanup failed: {error}")
                })?;
                return Err(format!("process wait error: {err}"));
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowsSuiteEvidenceRow {
    pub suite_id: String,
    pub disposition: WindowsDisposition,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exit_codes: Vec<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub executed_counts: Vec<usize>,
    pub commit: String,
    pub cargo_lock_sha256: String,
    pub invocation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_sid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rail_matched: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowsCoverageReport {
    pub schema: String,
    pub invocation_id: String,
    pub commit: String,
    pub cargo_lock_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_sync_evidence: Option<String>,
    pub rows: Vec<WindowsSuiteEvidenceRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerEvidenceSnapshot {
    pub schema: String,
    pub lease_schema: String,
    pub result_schema: String,
    pub nonce: String,
    pub result_nonce: String,
    pub expected_commit: String,
    pub expected_cargo_lock_sha256: String,
    pub expected_owner_account: String,
    pub expected_owner_sid: String,
    pub passed: bool,
    pub cargo_exit_code: i32,
    pub owner_sid: String,
    pub elevated: bool,
    pub ordinary_owner_marker: String,
    pub ordinary_owner_refs_marker: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceBinding {
    pub commit: String,
    pub cargo_lock_sha256: String,
    pub owner_account: Option<String>,
    pub prepared_owner_nonce: Option<String>,
}

pub fn verify_owner_credit(
    owner: &OwnerEvidenceSnapshot,
    binding: &SourceBinding,
    row_nonce: Option<&str>,
    required_features: &[String],
) -> Result<(), String> {
    let prepared_nonce = binding
        .prepared_owner_nonce
        .as_deref()
        .ok_or_else(|| "missing prepared owner nonce".to_owned())?;
    if prepared_nonce.is_empty() {
        return Err("prepared owner nonce is empty".to_owned());
    }
    if owner.nonce != prepared_nonce {
        return Err("owner lease nonce does not match prepared nonce".to_owned());
    }
    if owner.result_nonce != prepared_nonce {
        return Err("owner result nonce does not match prepared nonce".to_owned());
    }
    let rn = row_nonce.ok_or_else(|| "missing evidence row nonce".to_owned())?;
    if rn != prepared_nonce {
        return Err("evidence row nonce does not match prepared nonce".to_owned());
    }
    if owner.lease_schema != OWNER_RAIL_LEASE_SCHEMA {
        return Err(format!(
            "invalid owner lease schema {:?}",
            owner.lease_schema
        ));
    }
    if owner.result_schema != OWNER_RAIL_RESULT_SCHEMA {
        return Err(format!(
            "invalid owner result schema {:?}",
            owner.result_schema
        ));
    }
    if owner.expected_commit != binding.commit {
        return Err("owner commit mismatch".to_owned());
    }
    if owner.expected_cargo_lock_sha256 != binding.cargo_lock_sha256 {
        return Err("owner lock mismatch".to_owned());
    }
    if let Some(expected_account) = &binding.owner_account
        && &owner.expected_owner_account != expected_account
    {
        return Err("owner account mismatch".to_owned());
    }
    if owner.owner_sid != owner.expected_owner_sid {
        return Err("owner sid does not match expected owner sid".to_owned());
    }
    if owner.elevated {
        return Err("ordinary owner token attestation is elevated".to_owned());
    }
    if !owner.passed || owner.cargo_exit_code != 0 {
        return Err("ordinary owner rail failed".to_owned());
    }
    if owner.ordinary_owner_marker != "JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed" {
        return Err("missing ordinary owner control marker".to_owned());
    }
    if owner.ordinary_owner_refs_marker != "JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed" {
        return Err("missing ordinary owner refs marker".to_owned());
    }
    if required_features != ["test-hooks"] {
        return Err("ordinary owner required features must be test-hooks".to_owned());
    }
    Ok(())
}

pub fn host_evidence_for_control(
    control: &str,
    plan: &WindowsPlan,
    coverage: &WindowsCoverageReport,
) -> Result<Option<String>, String> {
    let mut matching_item = None;
    for item in &plan.items {
        if let Some(ref policy) = item.policy
            && let Some(ref opt_in) = policy.opt_in
            && opt_in.control == control
        {
            matching_item = Some(item);
            break;
        }
    }
    let item = match matching_item {
        Some(i) => i,
        None => return Ok(None),
    };

    let row = coverage
        .rows
        .iter()
        .find(|r| r.suite_id == item.id)
        .ok_or_else(|| {
            format!(
                "suite {}: missing evidence row for control {control}",
                item.id
            )
        })?;

    match control {
        "JOURNAL_WIN_CI_RUN_BACKUP" => match row.disposition {
            WindowsDisposition::OptedOut => Ok(Some("not-run".to_owned())),
            WindowsDisposition::Passed => Ok(Some("executed/pass".to_owned())),
            _ => Err(format!(
                "suite {}: invalid disposition {:?} for backup control",
                item.id, row.disposition
            )),
        },
        "JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST" => match row.disposition {
            WindowsDisposition::OptedOut => Ok(Some("skipped".to_owned())),
            WindowsDisposition::Passed => Ok(Some("passed".to_owned())),
            _ => Err(format!(
                "suite {}: invalid disposition {:?} for cloud sync control",
                item.id, row.disposition
            )),
        },
        _ => Ok(None),
    }
}

pub fn parse_executed_count(output: &str) -> usize {
    let re =
        Regex::new(r"(?m)^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;")
            .unwrap();
    let mut count = 0;
    for cap in re.captures_iter(output) {
        let passed: usize = cap[1].parse().unwrap_or(0);
        let failed: usize = cap[2].parse().unwrap_or(0);
        count = passed + failed;
    }
    count
}

/// Requires each expected marker, and each expected pattern, on exactly one
/// output line.
///
/// A marker or pattern is matched against a line suffix, not the whole line.
/// Suites run with `--nocapture`, so a marker printed by a test can land after
/// text that is not yet newline-terminated: libtest's own `test <name> ... `
/// progress prefix, or another test thread's unfinished result line. A
/// marker's `println!` is one locked write that ends in its newline, so
/// nothing can follow it on its line; only a prefix can be glued on.
/// A pattern is tried against every suffix of a line, so one anchored with
/// `^` matches where the expected text begins, after any glued prefix.
pub fn verify_command_output(
    output: &str,
    expected_markers: &[String],
    expected_patterns: &[String],
) -> Result<(), String> {
    for marker in expected_markers {
        let count = output
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| l.ends_with(marker.as_str()))
            .count();
        if count != 1 {
            return Err(format!(
                "expected exactly one occurrence of marker {:?}, found {count}",
                marker
            ));
        }
    }
    for pat in expected_patterns {
        let re = Regex::new(pat).map_err(|e| e.to_string())?;
        let count = output
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| (0..=l.len()).any(|i| l.is_char_boundary(i) && re.is_match(&l[i..])))
            .count();
        if count != 1 {
            return Err(format!(
                "expected exactly one match for pattern {:?}, found {count}",
                pat
            ));
        }
    }
    Ok(())
}

pub fn judge(
    plan: &WindowsPlan,
    coverage: &WindowsCoverageReport,
    owner_evidence: Option<&OwnerEvidenceSnapshot>,
    binding: &SourceBinding,
) -> Result<(), String> {
    if plan.items.is_empty() {
        return Err("plan is empty".to_owned());
    }
    if coverage.schema != WINDOWS_COVERAGE_SCHEMA {
        return Err(format!(
            "invalid coverage schema {:?}; expected {WINDOWS_COVERAGE_SCHEMA}",
            coverage.schema
        ));
    }
    if coverage.commit != binding.commit {
        return Err(format!(
            "coverage commit mismatch: expected {}, actual {}",
            binding.commit, coverage.commit
        ));
    }
    if coverage.cargo_lock_sha256 != binding.cargo_lock_sha256 {
        return Err(format!(
            "coverage Cargo.lock digest mismatch: expected {}, actual {}",
            binding.cargo_lock_sha256, coverage.cargo_lock_sha256
        ));
    }

    let mut row_map = BTreeMap::new();
    for row in &coverage.rows {
        if row_map.insert(row.suite_id.clone(), row).is_some() {
            return Err(format!("duplicate coverage row for suite {}", row.suite_id));
        }
    }

    for item in &plan.items {
        let row = row_map
            .get(&item.id)
            .ok_or_else(|| format!("suite {}: missing evidence row in coverage report", item.id))?;

        if row.commit != binding.commit || row.cargo_lock_sha256 != binding.cargo_lock_sha256 {
            return Err(format!("suite {}: stale evidence binding", item.id));
        }
        if row.invocation_id != coverage.invocation_id {
            return Err(format!("suite {}: invocation id mismatch", item.id));
        }
        if row.features != item.required_features {
            return Err(format!(
                "suite {}: feature set mismatch: expected {:?}, found {:?}",
                item.id, item.required_features, row.features
            ));
        }

        match item.status {
            PlanStatus::OptedOut => {
                if row.disposition != WindowsDisposition::OptedOut {
                    return Err(format!(
                        "suite {}: expected opted-out disposition, found {:?}",
                        item.id, row.disposition
                    ));
                }
                continue;
            }
            PlanStatus::Blocked(ref token) => {
                return Err(format!(
                    "suite {}: blocked due to missing require {token}",
                    item.id
                ));
            }
            PlanStatus::Ready => {}
        }

        match row.disposition {
            WindowsDisposition::Passed => {}
            WindowsDisposition::Failed => {
                return Err(format!("suite {}: failed execution", item.id));
            }
            WindowsDisposition::TimedOut => {
                return Err(format!("suite {}: timed out during execution", item.id));
            }
            WindowsDisposition::Blocked => {
                return Err(format!("suite {}: blocked disposition", item.id));
            }
            WindowsDisposition::OptedOut => {
                return Err(format!("suite {}: unexpectedly opted out", item.id));
            }
            WindowsDisposition::Unexecuted => {
                return Err(format!("suite {}: unexecuted disposition", item.id));
            }
        }

        let policy = item.policy.as_ref();
        let satisfaction = policy.map(|p| p.satisfaction.as_str()).unwrap_or("cargo");

        if satisfaction == "ordinary-owner" {
            let owner = owner_evidence.ok_or_else(|| {
                format!(
                    "suite {}: missing ordinary owner evidence snapshot",
                    item.id
                )
            })?;
            verify_owner_credit(
                owner,
                binding,
                row.nonce.as_deref(),
                &item.required_features,
            )
            .map_err(|e| format!("suite {}: {e}", item.id))?;
            continue;
        }

        // Check unlaunched filter hazard
        if let Some(pol) = policy {
            for unl in &pol.unlaunched {
                for cmd in &row.commands {
                    if cmd.contains(&format!("--exact {unl}"))
                        || cmd.contains(&format!("--exact \"{unl}\""))
                    {
                        return Err(format!(
                            "suite {}: executed unlaunched filter hazard {unl}",
                            item.id
                        ));
                    }
                }
            }
        }

        if satisfaction == "cargo"
            || (satisfaction == "receipts" && policy.map(|p| p.body).unwrap_or(false))
        {
            let body_count = row.executed_counts.first().copied().unwrap_or(0);
            if body_count == 0 {
                return Err(format!(
                    "suite {}: body cargo run executed 0 tests",
                    item.id
                ));
            }
        }

        if satisfaction == "receipts" {
            let pol = policy.unwrap();
            let receipt_start_idx = if pol.body { 1 } else { 0 };
            if row.executed_counts.len() < receipt_start_idx + pol.tests.len() {
                return Err(format!(
                    "suite {}: missing receipt command evidence",
                    item.id
                ));
            }
            for (i, test) in pol.tests.iter().enumerate() {
                let count = row.executed_counts[receipt_start_idx + i];
                if count != 1 {
                    return Err(format!(
                        "suite {} test {}: executed count must be 1, found {count}",
                        item.id, test.name
                    ));
                }
            }
        }
    }

    let expected_backup = host_evidence_for_control("JOURNAL_WIN_CI_RUN_BACKUP", plan, coverage)?;
    if coverage.backup_evidence != expected_backup {
        return Err(format!(
            "backup evidence mismatch: expected {:?}, found {:?}",
            expected_backup, coverage.backup_evidence
        ));
    }
    let expected_cloud =
        host_evidence_for_control("JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST", plan, coverage)?;
    if coverage.cloud_sync_evidence != expected_cloud {
        return Err(format!(
            "cloud sync evidence mismatch: expected {:?}, found {:?}",
            expected_cloud, coverage.cloud_sync_evidence
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::load_registry;

    fn test_fixture_registry() -> Registry {
        Registry {
            version: 1,
            sets: vec![
                "component".to_owned(),
                "differential".to_owned(),
                "race".to_owned(),
                "live".to_owned(),
            ],
            areas: vec!["test".to_owned()],
            platforms: vec!["linux".to_owned(), "windows".to_owned()],
            prerequisites: vec!["cargo-cache".to_owned(), "host-tools".to_owned()],
            serial_groups: vec![],
            runtimes: vec!["none".to_owned()],
            timeouts: BTreeMap::from([("standard".to_owned(), 600), ("quick".to_owned(), 120)]),
            suites: vec![
                Suite {
                    id: "pkg-a::test_win".to_owned(),
                    package: "pkg-a".to_owned(),
                    target: "test_win".to_owned(),
                    set: "component".to_owned(),
                    areas: vec!["test".to_owned()],
                    platforms: vec!["windows".to_owned()],
                    prerequisites: vec!["cargo-cache".to_owned()],
                    timeout: "standard".to_owned(),
                    serial_group: None,
                    default_full: true,
                    required_features: vec!["test-hooks".to_owned()],
                    runtime: "none".to_owned(),
                    windows: None,
                },
                Suite {
                    id: "pkg-b::test_linux".to_owned(),
                    package: "pkg-b".to_owned(),
                    target: "test_linux".to_owned(),
                    set: "component".to_owned(),
                    areas: vec!["test".to_owned()],
                    platforms: vec!["linux".to_owned()],
                    prerequisites: vec!["cargo-cache".to_owned()],
                    timeout: "standard".to_owned(),
                    serial_group: None,
                    default_full: true,
                    required_features: vec![],
                    runtime: "none".to_owned(),
                    windows: None,
                },
            ],
            package_suites: vec![],
            legs: vec![],
        }
    }

    #[test]
    fn synthetic_windows_suite_selected_and_linux_excluded() {
        let registry = test_fixture_registry();
        let plan = plan_windows(&registry, &BTreeMap::new(), &BTreeMap::new()).unwrap();
        assert_eq!(plan.items.len(), 1);
        let item = &plan.items[0];
        assert_eq!(item.id, "pkg-a::test_win");
        assert_eq!(item.package, "pkg-a");
        assert_eq!(item.target, "test_win");
        assert_eq!(item.required_features, vec!["test-hooks"]);
    }

    #[test]
    fn real_suites_toml_loads_and_selects_expected_windows_suites() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let suites_path = manifest_dir.join("../../ci/suites.toml");
        let registry = load_registry(&suites_path).unwrap();
        let controls = BTreeMap::new();
        let probes = BTreeMap::new();
        let plan = plan_windows(&registry, &controls, &probes).unwrap();

        let plan_ids: BTreeSet<_> = plan.items.iter().map(|i| i.id.as_str()).collect();

        // Every default_full suite whose platforms contain windows must be in the plan
        for suite in &registry.suites {
            if suite.default_full && suite.platforms.iter().any(|p| p == "windows") {
                assert!(
                    plan_ids.contains(suite.id.as_str()),
                    "suite {} is default_full with windows platform but missing from plan",
                    suite.id
                );
            }
        }

        assert!(
            !plan_ids.contains("solstone-core::service_logs_cli"),
            "solstone-core::service_logs_cli must be absent from windows plan"
        );

        let backup = plan
            .items
            .iter()
            .find(|i| i.id == "solstone-core-offload::backup_native")
            .unwrap();
        assert_eq!(backup.status, PlanStatus::OptedOut);
        assert!(backup.commands.is_empty());

        let cloud = plan
            .items
            .iter()
            .find(|i| i.id == "solstone-core-journal-io::windows_cloud_sync_root_registration")
            .unwrap();
        assert_eq!(cloud.status, PlanStatus::OptedOut);
        assert!(cloud.commands.is_empty());

        let rfdetr = plan
            .items
            .iter()
            .find(|i| i.id == "solstone-core-describe::windows_rfdetr_consumers")
            .unwrap();
        assert_eq!(rfdetr.status, PlanStatus::OptedOut);

        let onnx = plan
            .items
            .iter()
            .find(|i| i.id == "solstone-core-speakers-onnx::windows_runtime_binding")
            .unwrap();
        assert_eq!(onnx.status, PlanStatus::OptedOut);
    }

    #[test]
    fn opt_in_tri_state_behavior() {
        // Test Backup, Cloud Files, and ONNX
        let mut registry = test_fixture_registry();
        registry.suites = vec![
            Suite {
                id: "backup_suite".to_owned(),
                package: "pkg-backup".to_owned(),
                target: "backup_target".to_owned(),
                set: "component".to_owned(),
                areas: vec!["test".to_owned()],
                platforms: vec!["windows".to_owned()],
                prerequisites: vec![],
                timeout: "standard".to_owned(),
                serial_group: None,
                default_full: true,
                required_features: vec![],
                runtime: "none".to_owned(),
                windows: Some(WindowsPolicy {
                    satisfaction: "cargo".to_owned(),
                    opt_in: Some(WindowsOptIn {
                        control: "JOURNAL_WIN_CI_RUN_BACKUP".to_owned(),
                        requires: vec!["native-restic".to_owned(), "native-rclone".to_owned()],
                    }),
                    args: vec![],
                    markers: vec![],
                    marker_patterns: vec![],
                    body: false,
                    tests: vec![],
                    unlaunched: vec![],
                    acknowledgement: None,
                }),
            },
            Suite {
                id: "cloud_suite".to_owned(),
                package: "pkg-cloud".to_owned(),
                target: "cloud_target".to_owned(),
                set: "component".to_owned(),
                areas: vec!["test".to_owned()],
                platforms: vec!["windows".to_owned()],
                prerequisites: vec![],
                timeout: "standard".to_owned(),
                serial_group: None,
                default_full: true,
                required_features: vec![],
                runtime: "none".to_owned(),
                windows: Some(WindowsPolicy {
                    satisfaction: "cargo".to_owned(),
                    opt_in: Some(WindowsOptIn {
                        control: "JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST".to_owned(),
                        requires: vec!["cloud-files-api".to_owned()],
                    }),
                    args: vec![],
                    markers: vec![],
                    marker_patterns: vec![],
                    body: false,
                    tests: vec![],
                    unlaunched: vec![],
                    acknowledgement: None,
                }),
            },
            Suite {
                id: "onnx_suite".to_owned(),
                package: "pkg-onnx".to_owned(),
                target: "onnx_target".to_owned(),
                set: "component".to_owned(),
                areas: vec!["test".to_owned()],
                platforms: vec!["windows".to_owned()],
                prerequisites: vec![],
                timeout: "standard".to_owned(),
                serial_group: None,
                default_full: true,
                required_features: vec![],
                runtime: "none".to_owned(),
                windows: Some(WindowsPolicy {
                    satisfaction: "cargo".to_owned(),
                    opt_in: Some(WindowsOptIn {
                        control: "JOURNAL_WIN_CI_RUN_ONNX_BINDING".to_owned(),
                        requires: vec!["onnx-runtime".to_owned()],
                    }),
                    args: vec![],
                    markers: vec![],
                    marker_patterns: vec![],
                    body: false,
                    tests: vec![],
                    unlaunched: vec![],
                    acknowledgement: None,
                }),
            },
        ];

        // 1. Off -> OptedOut + empty commands
        let controls = BTreeMap::from([
            ("JOURNAL_WIN_CI_RUN_BACKUP".to_owned(), false),
            ("JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST".to_owned(), false),
            ("JOURNAL_WIN_CI_RUN_ONNX_BINDING".to_owned(), false),
        ]);
        let probes = BTreeMap::new();
        let plan = plan_windows(&registry, &controls, &probes).unwrap();
        for item in &plan.items {
            assert_eq!(item.status, PlanStatus::OptedOut);
            assert!(item.commands.is_empty());
        }

        // Host evidence mapping for opted out
        let cov_opt_out = WindowsCoverageReport {
            schema: WINDOWS_COVERAGE_SCHEMA.to_owned(),
            invocation_id: "inv".to_owned(),
            commit: "c".to_owned(),
            cargo_lock_sha256: "l".to_owned(),
            backup_evidence: None,
            cloud_sync_evidence: None,
            rows: vec![
                WindowsSuiteEvidenceRow {
                    suite_id: "backup_suite".to_owned(),
                    disposition: WindowsDisposition::OptedOut,
                    commands: vec![],
                    exit_codes: vec![],
                    log_paths: vec![],
                    features: vec![],
                    executed_counts: vec![],
                    commit: "c".to_owned(),
                    cargo_lock_sha256: "l".to_owned(),
                    invocation_id: "inv".to_owned(),
                    nonce: None,
                    owner_account: None,
                    owner_sid: None,
                    elevated: None,
                    rail_matched: None,
                },
                WindowsSuiteEvidenceRow {
                    suite_id: "cloud_suite".to_owned(),
                    disposition: WindowsDisposition::OptedOut,
                    commands: vec![],
                    exit_codes: vec![],
                    log_paths: vec![],
                    features: vec![],
                    executed_counts: vec![],
                    commit: "c".to_owned(),
                    cargo_lock_sha256: "l".to_owned(),
                    invocation_id: "inv".to_owned(),
                    nonce: None,
                    owner_account: None,
                    owner_sid: None,
                    elevated: None,
                    rail_matched: None,
                },
                WindowsSuiteEvidenceRow {
                    suite_id: "onnx_suite".to_owned(),
                    disposition: WindowsDisposition::OptedOut,
                    commands: vec![],
                    exit_codes: vec![],
                    log_paths: vec![],
                    features: vec![],
                    executed_counts: vec![],
                    commit: "c".to_owned(),
                    cargo_lock_sha256: "l".to_owned(),
                    invocation_id: "inv".to_owned(),
                    nonce: None,
                    owner_account: None,
                    owner_sid: None,
                    elevated: None,
                    rail_matched: None,
                },
            ],
        };
        assert_eq!(
            host_evidence_for_control("JOURNAL_WIN_CI_RUN_BACKUP", &plan, &cov_opt_out).unwrap(),
            Some("not-run".to_owned())
        );
        assert_eq!(
            host_evidence_for_control("JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST", &plan, &cov_opt_out)
                .unwrap(),
            Some("skipped".to_owned())
        );

        // Host evidence mapping for passed
        let mut cov_pass = cov_opt_out.clone();
        cov_pass.rows[0].disposition = WindowsDisposition::Passed;
        cov_pass.rows[1].disposition = WindowsDisposition::Passed;
        assert_eq!(
            host_evidence_for_control("JOURNAL_WIN_CI_RUN_BACKUP", &plan, &cov_pass).unwrap(),
            Some("executed/pass".to_owned())
        );
        assert_eq!(
            host_evidence_for_control("JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST", &plan, &cov_pass)
                .unwrap(),
            Some("passed".to_owned())
        );

        // Blocked is an error and does not produce a pass string
        let mut cov_blocked = cov_opt_out.clone();
        cov_blocked.rows[0].disposition = WindowsDisposition::Blocked;
        assert!(
            host_evidence_for_control("JOURNAL_WIN_CI_RUN_BACKUP", &plan, &cov_blocked).is_err()
        );

        // 2. On + Probe missing -> Blocked + empty commands
        let controls = BTreeMap::from([
            ("JOURNAL_WIN_CI_RUN_BACKUP".to_owned(), true),
            ("JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST".to_owned(), true),
            ("JOURNAL_WIN_CI_RUN_ONNX_BINDING".to_owned(), true),
        ]);
        let probes = BTreeMap::from([
            ("native-restic".to_owned(), true),
            ("native-rclone".to_owned(), false), // backup missing rclone
            ("cloud-files-api".to_owned(), false),
            ("onnx-runtime".to_owned(), false),
        ]);
        let plan = plan_windows(&registry, &controls, &probes).unwrap();
        assert!(matches!(plan.items[0].status, PlanStatus::Blocked(_)));
        assert!(plan.items[0].commands.is_empty());
        assert!(matches!(plan.items[1].status, PlanStatus::Blocked(_)));
        assert!(plan.items[1].commands.is_empty());
        assert!(matches!(plan.items[2].status, PlanStatus::Blocked(_)));
        assert!(plan.items[2].commands.is_empty());

        // 3. On + Probes present -> Ready + non-empty commands
        let probes = BTreeMap::from([
            ("native-restic".to_owned(), true),
            ("native-rclone".to_owned(), true),
            ("cloud-files-api".to_owned(), true),
            ("onnx-runtime".to_owned(), true),
        ]);
        let plan = plan_windows(&registry, &controls, &probes).unwrap();
        assert_eq!(plan.items[0].status, PlanStatus::Ready);
        assert!(!plan.items[0].commands.is_empty());
        assert_eq!(plan.items[1].status, PlanStatus::Ready);
        assert!(!plan.items[1].commands.is_empty());
        assert_eq!(plan.items[2].status, PlanStatus::Ready);
        assert!(!plan.items[2].commands.is_empty());
    }

    #[test]
    fn invalid_policies_fail_validation() {
        let mut suite = test_fixture_registry().suites[0].clone();

        // Unknown satisfaction
        let bad_sat = WindowsPolicy {
            satisfaction: "magic".to_owned(),
            opt_in: None,
            args: vec![],
            markers: vec![],
            marker_patterns: vec![],
            body: false,
            tests: vec![],
            unlaunched: vec![],
            acknowledgement: None,
        };
        assert!(
            validate_windows_policy(&suite, &bad_sat)
                .unwrap_err()
                .contains("unknown satisfaction magic")
        );

        // Receipt name duplicated into unlaunched
        let bad_unl = WindowsPolicy {
            satisfaction: "receipts".to_owned(),
            opt_in: None,
            args: vec![],
            markers: vec![],
            marker_patterns: vec![],
            body: false,
            tests: vec![WindowsReceiptTest {
                name: "hazard_test".to_owned(),
                lines: vec![],
                line_patterns: vec![],
            }],
            unlaunched: vec!["hazard_test".to_owned()],
            acknowledgement: None,
        };
        assert!(
            validate_windows_policy(&suite, &bad_unl)
                .unwrap_err()
                .contains("also declared unlaunched")
        );

        // Platform mismatch
        suite.platforms = vec!["linux".to_owned()];
        assert!(
            validate_windows_policy(&suite, &bad_sat)
                .unwrap_err()
                .contains("platforms does not include windows")
        );
    }

    #[test]
    fn judge_matrix_cases() {
        let registry = test_fixture_registry();
        let plan = plan_windows(&registry, &BTreeMap::new(), &BTreeMap::new()).unwrap();
        let binding = SourceBinding {
            commit: "abcdef0123456789abcdef0123456789abcdef01".to_owned(),
            cargo_lock_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_owned(),
            owner_account: Some("LOCAL_USER".to_owned()),
            prepared_owner_nonce: Some("nonce-123".to_owned()),
        };

        let valid_coverage = WindowsCoverageReport {
            schema: WINDOWS_COVERAGE_SCHEMA.to_owned(),
            invocation_id: "inv-1".to_owned(),
            commit: binding.commit.clone(),
            cargo_lock_sha256: binding.cargo_lock_sha256.clone(),
            backup_evidence: None,
            cloud_sync_evidence: None,
            rows: vec![WindowsSuiteEvidenceRow {
                suite_id: "pkg-a::test_win".to_owned(),
                disposition: WindowsDisposition::Passed,
                commands: vec!["cargo test ...".to_owned()],
                exit_codes: vec![0],
                log_paths: vec!["log.txt".to_owned()],
                features: vec!["test-hooks".to_owned()],
                executed_counts: vec![5],
                commit: binding.commit.clone(),
                cargo_lock_sha256: binding.cargo_lock_sha256.clone(),
                invocation_id: "inv-1".to_owned(),
                nonce: None,
                owner_account: None,
                owner_sid: None,
                elevated: None,
                rail_matched: None,
            }],
        };

        // 1. Valid passes
        assert!(judge(&plan, &valid_coverage, None, &binding).is_ok());

        // 2. Empty plan fails
        let empty_plan = WindowsPlan { items: vec![] };
        assert_eq!(
            judge(&empty_plan, &valid_coverage, None, &binding).unwrap_err(),
            "plan is empty"
        );

        // 3. Dropped row fails with suite name
        let mut cov = valid_coverage.clone();
        cov.rows.clear();
        assert!(
            judge(&plan, &cov, None, &binding)
                .unwrap_err()
                .contains("suite pkg-a::test_win: missing evidence row")
        );

        // 4. Timed-out fails with suite name
        let mut cov = valid_coverage.clone();
        cov.rows[0].disposition = WindowsDisposition::TimedOut;
        assert!(
            judge(&plan, &cov, None, &binding)
                .unwrap_err()
                .contains("suite pkg-a::test_win: timed out")
        );

        // 5. Unexecuted fails with suite name
        let mut cov = valid_coverage.clone();
        cov.rows[0].disposition = WindowsDisposition::Unexecuted;
        assert!(
            judge(&plan, &cov, None, &binding)
                .unwrap_err()
                .contains("suite pkg-a::test_win: unexecuted")
        );

        // 6. Plan status Blocked fails with suite name
        let mut blocked_plan = plan.clone();
        blocked_plan.items[0].status = PlanStatus::Blocked("missing-tool".to_owned());
        assert!(
            judge(&blocked_plan, &valid_coverage, None, &binding)
                .unwrap_err()
                .contains("suite pkg-a::test_win: blocked")
        );

        // 7. Invocation id mismatch fails with suite name
        let mut cov = valid_coverage.clone();
        cov.rows[0].invocation_id = "inv-other".to_owned();
        assert!(
            judge(&plan, &cov, None, &binding)
                .unwrap_err()
                .contains("suite pkg-a::test_win: invocation id mismatch")
        );

        // 8. Feature mismatch fails with suite name
        let mut cov = valid_coverage.clone();
        cov.rows[0].features = vec!["wrong-feature".to_owned()];
        assert!(
            judge(&plan, &cov, None, &binding)
                .unwrap_err()
                .contains("suite pkg-a::test_win: feature set mismatch")
        );

        // 9. Zero-test execution on plain cargo fails
        let mut cov = valid_coverage.clone();
        cov.rows[0].executed_counts = vec![0];
        assert!(
            judge(&plan, &cov, None, &binding)
                .unwrap_err()
                .contains("suite pkg-a::test_win: body cargo run executed 0 tests")
        );

        // 10. Stale commit fails
        let mut cov = valid_coverage.clone();
        cov.rows[0].commit = "stale_sha".to_owned();
        assert!(
            judge(&plan, &cov, None, &binding)
                .unwrap_err()
                .contains("suite pkg-a::test_win: stale evidence")
        );

        // 11. Failed disposition fails
        let mut cov = valid_coverage.clone();
        cov.rows[0].disposition = WindowsDisposition::Failed;
        assert!(
            judge(&plan, &cov, None, &binding)
                .unwrap_err()
                .contains("suite pkg-a::test_win: failed execution")
        );

        // 12. Ordinary owner matrix
        let mut owner_registry = test_fixture_registry();
        owner_registry.suites[0].windows = Some(WindowsPolicy {
            satisfaction: "ordinary-owner".to_owned(),
            opt_in: None,
            args: vec![],
            markers: vec![],
            marker_patterns: vec![],
            body: false,
            tests: vec![],
            unlaunched: vec![],
            acknowledgement: None,
        });
        let owner_plan = plan_windows(&owner_registry, &BTreeMap::new(), &BTreeMap::new()).unwrap();

        let valid_owner_snapshot = OwnerEvidenceSnapshot {
            schema: "solstone.journal.win-owner-rail.evidence-snapshot.v1".to_owned(),
            lease_schema: OWNER_RAIL_LEASE_SCHEMA.to_owned(),
            result_schema: OWNER_RAIL_RESULT_SCHEMA.to_owned(),
            nonce: "nonce-123".to_owned(),
            result_nonce: "nonce-123".to_owned(),
            expected_commit: binding.commit.clone(),
            expected_cargo_lock_sha256: binding.cargo_lock_sha256.clone(),
            expected_owner_account: "LOCAL_USER".to_owned(),
            expected_owner_sid: "S-1-5-21-123".to_owned(),
            passed: true,
            cargo_exit_code: 0,
            owner_sid: "S-1-5-21-123".to_owned(),
            elevated: false,
            ordinary_owner_marker: "JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed".to_owned(),
            ordinary_owner_refs_marker: "JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed".to_owned(),
        };

        let mut owner_cov = valid_coverage.clone();
        owner_cov.rows[0].nonce = Some("nonce-123".to_owned());
        owner_cov.rows[0].owner_account = Some("LOCAL_USER".to_owned());
        owner_cov.rows[0].owner_sid = Some("S-1-5-21-123".to_owned());
        owner_cov.rows[0].elevated = Some(false);
        owner_cov.rows[0].rail_matched = Some(true);
        owner_cov.rows[0].executed_counts = vec![]; // empty counts for ordinary-owner

        // Valid owner passes
        assert!(
            judge(
                &owner_plan,
                &owner_cov,
                Some(&valid_owner_snapshot),
                &binding
            )
            .is_ok()
        );

        // Missing row nonce in coverage report fails
        let mut no_nonce_cov = owner_cov.clone();
        no_nonce_cov.rows[0].nonce = None;
        assert!(
            judge(
                &owner_plan,
                &no_nonce_cov,
                Some(&valid_owner_snapshot),
                &binding
            )
            .unwrap_err()
            .contains("suite pkg-a::test_win: missing evidence row nonce")
        );

        // Missing prepared nonce fails
        let binding_no_nonce = SourceBinding {
            commit: binding.commit.clone(),
            cargo_lock_sha256: binding.cargo_lock_sha256.clone(),
            owner_account: binding.owner_account.clone(),
            prepared_owner_nonce: None,
        };
        assert!(
            judge(
                &owner_plan,
                &owner_cov,
                Some(&valid_owner_snapshot),
                &binding_no_nonce
            )
            .unwrap_err()
            .contains("missing prepared owner nonce")
        );

        // Previous/different nonce fails
        let binding_diff_nonce = SourceBinding {
            commit: binding.commit.clone(),
            cargo_lock_sha256: binding.cargo_lock_sha256.clone(),
            owner_account: binding.owner_account.clone(),
            prepared_owner_nonce: Some("nonce-old".to_owned()),
        };
        assert!(
            judge(
                &owner_plan,
                &owner_cov,
                Some(&valid_owner_snapshot),
                &binding_diff_nonce
            )
            .unwrap_err()
            .contains("does not match prepared nonce")
        );

        // Result nonce different from lease nonce fails
        let mut bad_res_nonce = valid_owner_snapshot.clone();
        bad_res_nonce.result_nonce = "nonce-diff".to_owned();
        assert!(
            judge(&owner_plan, &owner_cov, Some(&bad_res_nonce), &binding)
                .unwrap_err()
                .contains("does not match prepared nonce")
        );

        // Owner account mismatch fails
        let mut bad_acct = valid_owner_snapshot.clone();
        bad_acct.expected_owner_account = "OTHER_USER".to_owned();
        assert!(
            judge(&owner_plan, &owner_cov, Some(&bad_acct), &binding)
                .unwrap_err()
                .contains("owner account mismatch")
        );

        // Owner sid mismatch fails
        let mut bad_sid = valid_owner_snapshot.clone();
        bad_sid.owner_sid = "S-1-5-21-999".to_owned();
        assert!(
            judge(&owner_plan, &owner_cov, Some(&bad_sid), &binding)
                .unwrap_err()
                .contains("owner sid does not match expected owner sid")
        );

        // Elevated token fails
        let mut bad_elev = valid_owner_snapshot.clone();
        bad_elev.elevated = true;
        assert!(
            judge(&owner_plan, &owner_cov, Some(&bad_elev), &binding)
                .unwrap_err()
                .contains("ordinary owner token attestation is elevated")
        );

        // Missing or wrong refs marker fails
        let mut bad_refs = valid_owner_snapshot.clone();
        bad_refs.ordinary_owner_refs_marker = "wrong".to_owned();
        assert!(
            judge(&owner_plan, &owner_cov, Some(&bad_refs), &binding)
                .unwrap_err()
                .contains("missing ordinary owner refs marker")
        );

        // 13. Ignored-only receipts (body = false)
        let mut receipts_registry = test_fixture_registry();
        receipts_registry.suites[0].windows = Some(WindowsPolicy {
            satisfaction: "receipts".to_owned(),
            opt_in: None,
            args: vec![],
            markers: vec![],
            marker_patterns: vec![],
            body: false,
            tests: vec![WindowsReceiptTest {
                name: "test_receipt".to_owned(),
                lines: vec![],
                line_patterns: vec![],
            }],
            unlaunched: vec![],
            acknowledgement: None,
        });
        let receipts_plan =
            plan_windows(&receipts_registry, &BTreeMap::new(), &BTreeMap::new()).unwrap();

        // Zero count for receipt test fails
        let mut receipts_cov = valid_coverage.clone();
        receipts_cov.rows[0].executed_counts = vec![0];
        assert!(
            judge(&receipts_plan, &receipts_cov, None, &binding)
                .unwrap_err()
                .contains("executed count must be 1, found 0")
        );

        // Count 1 passes
        receipts_cov.rows[0].executed_counts = vec![1];
        assert!(judge(&receipts_plan, &receipts_cov, None, &binding).is_ok());

        // Unlaunched filter hazard fails
        receipts_registry.suites[0].windows = Some(WindowsPolicy {
            satisfaction: "receipts".to_owned(),
            opt_in: None,
            args: vec![],
            markers: vec![],
            marker_patterns: vec![],
            body: false,
            tests: vec![WindowsReceiptTest {
                name: "test_receipt".to_owned(),
                lines: vec![],
                line_patterns: vec![],
            }],
            unlaunched: vec![
                "process::backup_native_job_helper".to_owned(),
                "windows_service_capture::native_capture_child".to_owned(),
            ],
            acknowledgement: None,
        });
        let hazard_plan =
            plan_windows(&receipts_registry, &BTreeMap::new(), &BTreeMap::new()).unwrap();
        let mut hazard_cov = receipts_cov.clone();
        hazard_cov.rows[0].commands =
            vec!["cargo test ... --exact process::backup_native_job_helper".to_owned()];
        assert!(
            judge(&hazard_plan, &hazard_cov, None, &binding)
                .unwrap_err()
                .contains("executed unlaunched filter hazard")
        );
        hazard_cov.rows[0].commands =
            vec!["cargo test ... --exact windows_service_capture::native_capture_child".to_owned()];
        assert!(
            judge(&hazard_plan, &hazard_cov, None, &binding)
                .unwrap_err()
                .contains("executed unlaunched filter hazard")
        );
    }

    struct FakeTree {
        calls: usize,
        killed: bool,
    }
    impl ProcessTree for FakeTree {
        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            self.calls += 1;
            Ok(None)
        }
        fn kill_tree(&mut self) -> std::io::Result<()> {
            self.killed = true;
            Ok(())
        }
    }

    const INSTALL_FILE_PROTOCOL_MARKER: &str = "JOURNAL_WIN_CI_INSTALL_FILE_PROTOCOL=admission/retry/sharing/reconciliation/cleanup/uncertainty/pass";

    #[test]
    fn verify_command_output_counts_a_marker_glued_after_another_tests_output() {
        // Captured from a native Windows run: the marker test's println landed
        // after the ignored test's unterminated result line.
        let output = "\r\nrunning 18 tests\r\n\
            test journal_win_ci_windows_install_file_protocol_marker ... ignored, source-origin marker for the native Windows gateJOURNAL_WIN_CI_INSTALL_FILE_PROTOCOL=admission/retry/sharing/reconciliation/cleanup/uncertainty/pass\r\n\
            \r\n\
            test install_file_protocol_receipt_marker ... ok\r\n\
            test cross_volume_refusal_cleans_the_held_source ... ok\r\n\
            \r\n\
            test result: ok. 17 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.06s\r\n";
        verify_command_output(output, &[INSTALL_FILE_PROTOCOL_MARKER.to_owned()], &[]).unwrap();
    }

    #[test]
    fn verify_command_output_requires_exactly_one_marker_line() {
        let marker = vec![INSTALL_FILE_PROTOCOL_MARKER.to_owned()];

        let missing = "test install_file_protocol_receipt_marker ... ok\r\n";
        let err = verify_command_output(missing, &marker, &[]).unwrap_err();
        assert!(err.contains("found 0"), "{err}");

        let repeated = format!(
            "{INSTALL_FILE_PROTOCOL_MARKER}\r\ntest a ... {INSTALL_FILE_PROTOCOL_MARKER}\r\n"
        );
        let err = verify_command_output(&repeated, &marker, &[]).unwrap_err();
        assert!(err.contains("found 2"), "{err}");

        // The marker must end its line; text after it is a different line.
        let not_a_suffix = format!("{INSTALL_FILE_PROTOCOL_MARKER}/extra\r\n");
        let err = verify_command_output(&not_a_suffix, &marker, &[]).unwrap_err();
        assert!(err.contains("found 0"), "{err}");
    }

    #[test]
    fn verify_command_output_matches_an_anchored_pattern_after_a_glued_prefix() {
        let patterns = vec!["^JOURNAL_WIN_CI_STAGED_OS=.+$".to_owned()];

        // A serial run writes libtest's progress prefix before the test body
        // prints, so the first receipt line can share that line.
        let glued = "test staged_protocol_covers_ntfs_and_refs ... JOURNAL_WIN_CI_STAGED_OS=Microsoft Windows [Version 10.0.26100.9457]\r\nok\r\n";
        verify_command_output(glued, &[], &patterns).unwrap();

        let missing = "test staged_protocol_covers_ntfs_and_refs ... ok\r\n";
        let err = verify_command_output(missing, &[], &patterns).unwrap_err();
        assert!(err.contains("found 0"), "{err}");

        let repeated = "JOURNAL_WIN_CI_STAGED_OS=a\r\ntest x ... JOURNAL_WIN_CI_STAGED_OS=b\r\n";
        let err = verify_command_output(repeated, &[], &patterns).unwrap_err();
        assert!(err.contains("found 2"), "{err}");
    }

    #[test]
    fn fake_tree_timeout_kills_tree() {
        let mut tree = FakeTree {
            calls: 0,
            killed: false,
        };
        let start = Instant::now();
        let now = {
            let mut offset = Duration::ZERO;
            move || {
                let current = start + offset;
                offset += Duration::from_secs(10);
                current
            }
        };
        let res = wait_bounded(&mut tree, Duration::from_secs(5), now, |_| {});
        assert!(res.is_err());
        assert!(tree.killed);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn descendant_cleanup_on_timeout() {
        #[cfg(unix)]
        {
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;

            let temp_dir = tempfile::tempdir().unwrap();
            let pid_file = temp_dir.path().join("child.pid");
            let script = format!("sleep 100 & echo $! > {}; wait", pid_file.display());
            let mut cmd = Command::new("sh");
            cmd.args(["-c", &script]);
            let mut tree = spawn_process_tree(&mut cmd).unwrap();

            // Wait for pid file
            let start = Instant::now();
            while !pid_file.exists() && start.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(50));
            }
            let pid_str = std::fs::read_to_string(&pid_file).unwrap();
            let grandchild_pid: i32 = pid_str.trim().parse().unwrap();

            let res = wait_bounded(
                &mut tree,
                Duration::from_millis(100),
                Instant::now,
                std::thread::sleep,
            );
            assert!(res.is_err());

            // Assert grandchild is dead (kill -0 fails)
            let poll_start = Instant::now();
            let mut is_alive = true;
            while poll_start.elapsed() < Duration::from_secs(3) {
                if kill(Pid::from_raw(grandchild_pid), None).is_err() {
                    is_alive = false;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }

            if is_alive {
                let _ = kill(Pid::from_raw(grandchild_pid), Signal::SIGKILL);
                panic!("grandchild process {grandchild_pid} was not killed on timeout");
            }
        }
    }
}
