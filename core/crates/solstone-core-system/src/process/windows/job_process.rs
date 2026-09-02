// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Atomic Job-contained Windows process launch and lifecycle ownership.

use std::io;
use std::time::{Duration, Instant};

use crate::process::{ProcessBirth, ProcessInstance};

use super::handle::{JobHandle, PrimaryThreadHandle, RootProcessHandle};
#[cfg(windows)]
use super::identity::{WindowsFileTime, filetime_value};
use super::job::{JobMembership, WindowsJobApi};
#[cfg(windows)]
use super::job::{JobResourceLimits, SystemWindowsJobApi, create_kill_on_close_job_with_limits};
#[cfg(windows)]
use super::pipes::SystemWindowsPipeApi;
use super::pipes::{PipedStdio, WindowsPipeApi};
#[cfg(windows)]
use super::startup_info::{SystemWindowsStartupInfoApi, WindowsStartupInfo};

/// The separate Job-reap window used after `TerminateJobObject`.
pub(super) const JOB_HARD_STOP_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(windows)]
const JOB_HARD_STOP_EXIT_CODE: u32 = windows_sys::Win32::Foundation::ERROR_PROCESS_ABORTED;
#[cfg(not(windows))]
const JOB_HARD_STOP_EXIT_CODE: u32 = 1067;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProcessWait {
    Timeout,
    Signaled,
}

fn map_windows_wait_result(result: u32, failed_error: io::Error) -> io::Result<ProcessWait> {
    const WAIT_OBJECT_0_VALUE: u32 = 0;
    const WAIT_TIMEOUT_VALUE: u32 = 258;
    const WAIT_FAILED_VALUE: u32 = u32::MAX;

    match result {
        WAIT_TIMEOUT_VALUE => Ok(ProcessWait::Timeout),
        WAIT_OBJECT_0_VALUE => Ok(ProcessWait::Signaled),
        WAIT_FAILED_VALUE => Err(failed_error),
        unexpected => Err(io::Error::other(format!(
            "WaitForSingleObject returned unexpected value 0x{unexpected:08x}"
        ))),
    }
}

pub(super) trait WindowsProcessControlApi {
    fn wait_for_process(
        &self,
        process: &RootProcessHandle,
        timeout: Duration,
    ) -> io::Result<ProcessWait>;
    fn exit_code(&self, process: &RootProcessHandle) -> io::Result<u32>;
}

#[cfg(windows)]
pub(super) struct SystemWindowsProcessControlApi;

#[cfg(windows)]
impl WindowsProcessControlApi for SystemWindowsProcessControlApi {
    fn wait_for_process(
        &self,
        process: &RootProcessHandle,
        timeout: Duration,
    ) -> io::Result<ProcessWait> {
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        let milliseconds = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
        // SAFETY: `process` owns a live process handle for this synchronous wait.
        #[allow(unsafe_code)]
        let result = unsafe { WaitForSingleObject(process.raw(), milliseconds) };
        map_windows_wait_result(result, io::Error::last_os_error())
    }

    fn exit_code(&self, process: &RootProcessHandle) -> io::Result<u32> {
        use windows_sys::Win32::System::Threading::GetExitCodeProcess;

        let mut exit_code = 0;
        // SAFETY: `process` owns a live process handle and `exit_code` is writable.
        #[allow(unsafe_code)]
        let read = unsafe { GetExitCodeProcess(process.raw(), &raw mut exit_code) };
        if read == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(exit_code)
    }
}

fn map_wait_to_exit(
    wait: ProcessWait,
    exit_code: impl FnOnce() -> io::Result<u32>,
) -> io::Result<Option<i32>> {
    match wait {
        ProcessWait::Timeout => Ok(None),
        ProcessWait::Signaled => {
            exit_code().map(|code| Some(i32::from_ne_bytes(code.to_ne_bytes())))
        }
    }
}

fn run_create_window<T>(
    pipes: &mut PipedStdio,
    pipe_api: &impl WindowsPipeApi,
    create: impl FnOnce(&PipedStdio) -> io::Result<T>,
) -> io::Result<T> {
    pipes.make_child_ends_inheritable(pipe_api)?;
    let created = create(pipes);
    let cleanup = pipes.close_child_ends(pipe_api);
    match created {
        Ok(created) => {
            cleanup?;
            Ok(created)
        }
        Err(error) => {
            let _ = cleanup;
            Err(error)
        }
    }
}

#[cfg(any(test, all(windows, feature = "test-hooks")))]
fn duplicate_job_for_consuming_transition(
    owner: &WindowsJobProcess,
    duplicate: impl FnOnce(super::handle::RawWindowsHandle) -> io::Result<JobHandle>,
) -> io::Result<JobHandle> {
    duplicate(owner.job()?.raw())
}

/// All process-owned objects retained after a successful atomic launch.
pub(super) struct WindowsJobProcess {
    // Option permits the test-only duplicate-handle negative control to close
    // the original without manufacturing a second production ownership path.
    job: Option<JobHandle>,
    root: RootProcessHandle,
    #[cfg_attr(not(windows), allow(dead_code))]
    identity: ProcessInstance,
    #[cfg_attr(not(windows), allow(dead_code))]
    pipes: PipedStdio,
    hard_stop_issued: bool,
}

impl WindowsJobProcess {
    #[cfg(windows)]
    pub(super) fn identity(&self) -> ProcessInstance {
        self.identity
    }

    fn job(&self) -> io::Result<&JobHandle> {
        self.job
            .as_ref()
            .ok_or_else(|| io::Error::other("Job handle is unavailable"))
    }

    pub(super) fn observe_member_with(
        &self,
        jobs: &impl WindowsJobApi,
    ) -> io::Result<JobMembership> {
        jobs.observe_member(self.root.raw(), self.job()?)
    }

    fn poll_with(&self, process: &impl WindowsProcessControlApi) -> io::Result<Option<i32>> {
        map_wait_to_exit(
            process.wait_for_process(&self.root, Duration::ZERO)?,
            || process.exit_code(&self.root),
        )
    }

    fn wait_with(&self, process: &impl WindowsProcessControlApi) -> io::Result<i32> {
        match map_wait_to_exit(
            process.wait_for_process(&self.root, Duration::from_millis(u64::from(u32::MAX)))?,
            || process.exit_code(&self.root),
        )? {
            Some(exit_code) => Ok(exit_code),
            None => Err(io::Error::other("infinite process wait timed out")),
        }
    }

    fn hard_stop_with(
        &mut self,
        jobs: &impl WindowsJobApi,
        process: &impl WindowsProcessControlApi,
        deadline: Instant,
    ) -> io::Result<i32> {
        if !self.hard_stop_issued {
            jobs.terminate(self.job()?, JOB_HARD_STOP_EXIT_CODE)
                .map_err(|error| {
                    io::Error::new(error.kind(), format!("TerminateJobObject failed: {error}"))
                })?;
            self.hard_stop_issued = true;
        }

        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "root process did not signal before the hard-stop deadline",
                )
            })?;
        match process
            .wait_for_process(&self.root, remaining)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("waiting for the hard-stopped root failed: {error}"),
                )
            })? {
            ProcessWait::Signaled => {}
            ProcessWait::Timeout => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "root process did not signal before the hard-stop deadline",
                ));
            }
        }

        loop {
            if jobs
                .accounting(self.job()?)
                .map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!("querying hard-stopped Job accounting failed: {error}"),
                    )
                })?
                .active_processes
                == 0
            {
                return process
                    .exit_code(&self.root)
                    .map_err(|error| {
                        io::Error::new(
                            error.kind(),
                            format!("reading hard-stopped root exit code failed: {error}"),
                        )
                    })
                    .map(|code| i32::from_ne_bytes(code.to_ne_bytes()));
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Job did not reach zero active processes after hard stop",
                ));
            };
            std::thread::sleep(remaining.min(Duration::from_millis(20)));
        }
    }

    #[cfg(windows)]
    pub(super) fn observe_member(&self) -> io::Result<JobMembership> {
        self.observe_member_with(&SystemWindowsJobApi)
    }

    #[cfg(windows)]
    pub(super) fn poll(&self) -> io::Result<Option<i32>> {
        self.poll_with(&SystemWindowsProcessControlApi)
    }

    #[cfg(windows)]
    pub(super) fn wait(&self) -> io::Result<i32> {
        self.wait_with(&SystemWindowsProcessControlApi)
    }

    #[cfg(windows)]
    pub(super) fn hard_stop(&mut self) -> io::Result<i32> {
        self.hard_stop_with(
            &SystemWindowsJobApi,
            &SystemWindowsProcessControlApi,
            Instant::now() + JOB_HARD_STOP_TIMEOUT,
        )
    }

    /// Terminate the complete Job and require both root reap and Job quiescence
    /// before the caller's already-established deadline.
    #[cfg(windows)]
    pub(super) fn hard_stop_until(&mut self, deadline: Instant) -> io::Result<i32> {
        self.hard_stop_with(
            &SystemWindowsJobApi,
            &SystemWindowsProcessControlApi,
            deadline,
        )
    }

    /// Transfer the two parent-owned output pipe ends to the managed facade.
    /// The pipe handles leave this owner exactly once, after child endpoints
    /// have been closed and atomic Job enrollment has succeeded.
    #[cfg(windows)]
    pub(super) fn take_output_files(&mut self) -> (std::fs::File, std::fs::File) {
        use std::os::windows::io::{FromRawHandle, RawHandle};

        let stdout = self.pipes.parent_stdout_read.take_raw();
        let stderr = self.pipes.parent_stderr_read.take_raw();
        // SAFETY: `take_raw` transfers each uniquely owned pipe handle to its
        // corresponding File, which becomes its sole closer.
        #[allow(unsafe_code)]
        unsafe {
            (
                std::fs::File::from_raw_handle(stdout as RawHandle),
                std::fs::File::from_raw_handle(stderr as RawHandle),
            )
        }
    }

    #[cfg(windows)]
    pub(super) fn is_quiescent(&self) -> io::Result<bool> {
        Ok(SystemWindowsJobApi
            .accounting(self.job()?)?
            .active_processes
            == 0)
    }
}

#[cfg(windows)]
fn get_process_birth(process: &RootProcessHandle) -> io::Result<ProcessBirth> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the process handle is live and all four FILETIME outputs are writable.
    #[allow(unsafe_code)]
    let read = unsafe {
        GetProcessTimes(
            process.raw(),
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ProcessBirth::windows(filetime_value(WindowsFileTime {
        high: creation.dwHighDateTime,
        low: creation.dwLowDateTime,
    })))
}

struct CreatedWindowsProcess {
    root: RootProcessHandle,
    thread: PrimaryThreadHandle,
    pid: u32,
}

struct FinalizedWindowsProcess {
    root: RootProcessHandle,
    identity: ProcessInstance,
}

fn finalize_created_process(
    mut created: CreatedWindowsProcess,
    pipes: &mut PipedStdio,
    close_thread: impl FnOnce(&mut PrimaryThreadHandle) -> io::Result<()>,
    close_parent_stdin: impl FnOnce(&mut PipedStdio) -> io::Result<()>,
    process_birth: impl FnOnce(&RootProcessHandle) -> io::Result<ProcessBirth>,
) -> io::Result<FinalizedWindowsProcess> {
    close_thread(&mut created.thread).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("closing the primary thread handle failed: {error}"),
        )
    })?;
    close_parent_stdin(pipes).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("closing the parent stdin handle failed: {error}"),
        )
    })?;
    let birth = process_birth(&created.root).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("capturing the root process birth token failed: {error}"),
        )
    })?;
    Ok(FinalizedWindowsProcess {
        root: created.root,
        identity: ProcessInstance {
            pid: created.pid,
            birth,
        },
    })
}

#[cfg(windows)]
trait WindowsCreateProcessApi {
    fn create_process(
        &self,
        launch_spec: &mut super::launch_spec::WindowsLaunchSpec,
        startup: &WindowsStartupInfo,
        pipes: Option<&PipedStdio>,
        inherit_handles: bool,
    ) -> io::Result<CreatedWindowsProcess>;
}

#[cfg(windows)]
struct SystemWindowsCreateProcessApi;

#[cfg(windows)]
impl WindowsCreateProcessApi for SystemWindowsCreateProcessApi {
    fn create_process(
        &self,
        launch_spec: &mut super::launch_spec::WindowsLaunchSpec,
        startup: &WindowsStartupInfo,
        pipes: Option<&PipedStdio>,
        inherit_handles: bool,
    ) -> io::Result<CreatedWindowsProcess> {
        use std::ptr::null;

        use windows_sys::Win32::System::Threading::{
            CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION,
        };

        let stdio = pipes.map(|pipes| {
            [
                pipes.child_stdin_read().raw(),
                pipes.child_stdout_write().raw(),
                pipes.child_stderr_write().raw(),
            ]
        });
        let startup_info = startup.as_startup_info(stdio);
        let mut information = PROCESS_INFORMATION::default();
        // SAFETY: every buffer is owned and remains live through the synchronous
        // call; only the three listed child pipe ends are inheritable.
        #[allow(unsafe_code)]
        let created = unsafe {
            CreateProcessW(
                launch_spec.application_name().as_ptr(),
                launch_spec.command_line_mut().as_mut_ptr(),
                null(),
                null(),
                i32::from(inherit_handles),
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
                launch_spec.environment().as_ptr().cast(),
                launch_spec
                    .current_directory()
                    .map_or_else(null, |current_directory| current_directory.as_ptr()),
                &raw const startup_info.StartupInfo,
                &raw mut information,
            )
        };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        let root =
            (!information.hProcess.is_null()).then(|| RootProcessHandle::new(information.hProcess));
        let thread =
            (!information.hThread.is_null()).then(|| PrimaryThreadHandle::new(information.hThread));
        let (Some(root), Some(thread)) = (root, thread) else {
            return Err(io::Error::other(
                "CreateProcessW returned a null process handle",
            ));
        };
        Ok(CreatedWindowsProcess {
            root,
            thread,
            pid: information.dwProcessId,
        })
    }
}

#[cfg(windows)]
fn prepare_launch_spec(
    command: &[String],
    environment_overrides: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    current_directory: Option<&std::path::Path>,
) -> io::Result<super::launch_spec::WindowsLaunchSpec> {
    use super::environment::{
        SystemInheritedWindowsEnvironment, SystemWindowsOrdinalCompare, SystemWindowsWideEncoder,
    };
    use super::launch_spec::{WindowsLaunchAdapters, prepare_windows_launch_spec};
    use super::resolve::{SystemWindowsCandidateProbe, SystemWindowsDirectoryLookup};
    use super::user_path::SystemWindowsFullPathName;

    let probe = SystemWindowsCandidateProbe;
    let directories = SystemWindowsDirectoryLookup;
    let ordinal = SystemWindowsOrdinalCompare;
    let inherited_environment = SystemInheritedWindowsEnvironment;
    let wide_encoder = SystemWindowsWideEncoder;
    let full_path = SystemWindowsFullPathName;
    let adapters = WindowsLaunchAdapters {
        probe: &probe,
        directories: &directories,
        ordinal: &ordinal,
        inherited_environment: &inherited_environment,
        wide_encoder: &wide_encoder,
    };
    let mut launch_spec =
        prepare_windows_launch_spec(command, environment_overrides, &adapters, &full_path)
            .map_err(io::Error::other)?;
    if let Some(current_directory) = current_directory {
        use std::os::windows::ffi::OsStrExt;

        let current_directory = current_directory
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>();
        launch_spec
            .set_current_directory(&current_directory, &full_path)
            .map_err(io::Error::other)?;
    }
    Ok(launch_spec)
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct WindowsJobLaunchOptions<'a> {
    /// An explicit absolute current directory for the child.
    pub(super) current_directory: Option<&'a std::path::Path>,
    /// Every requested limit is installed before `CreateProcessW` may execute
    /// the helper's first instruction.
    pub(super) resource_limits: Option<JobResourceLimits>,
}

#[cfg(windows)]
pub(super) fn launch_windows_job_process(
    command: &[String],
    environment_overrides: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> io::Result<WindowsJobProcess> {
    launch_windows_job_process_with_options(
        command,
        environment_overrides,
        WindowsJobLaunchOptions::default(),
    )
}

/// Atomically enroll a helper in its fully configured Job before its first
/// instruction, with explicit current-directory and Job-resource choices.
#[cfg(windows)]
pub(super) fn launch_windows_job_process_with_options(
    command: &[String],
    environment_overrides: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    options: WindowsJobLaunchOptions<'_>,
) -> io::Result<WindowsJobProcess> {
    let mut launch_spec =
        prepare_launch_spec(command, environment_overrides, options.current_directory)?;

    let jobs = SystemWindowsJobApi;
    let pipe_api = SystemWindowsPipeApi;
    let startup_api = SystemWindowsStartupInfoApi;
    let job = create_kill_on_close_job_with_limits(&jobs, options.resource_limits)?;
    let mut pipes = PipedStdio::create(&pipe_api)?;
    let mut startup = WindowsStartupInfo::new(
        &startup_api,
        &job,
        pipes.child_stdin_read(),
        pipes.child_stdout_write(),
        pipes.child_stderr_write(),
    )?;
    // Operator-approved YAGNI residual: while these exact three child stdio
    // handles are inheritable, a concurrent list-less bInheritHandles=TRUE
    // child can inherit them. Its accidental hold may spend the two-second
    // bounded drain join once for stdout and once for stderr (about four
    // seconds total), lose tail log lines, and leave both drain threads plus
    // their detached join waiters parked until that holder exits. Containment,
    // Job identity, and the stop ladder do not depend on pipe EOF. Deliberately
    // add no mutex, helper, broker, or other mitigation for this residual.
    let created = run_create_window(&mut pipes, &pipe_api, |pipes| {
        SystemWindowsCreateProcessApi
            .create_process(&mut launch_spec, &startup, Some(pipes), true)
            .map_err(|error| {
                io::Error::new(error.kind(), format!("CreateProcessW failed: {error}"))
            })
    });
    startup.delete_with(&startup_api);
    let finalized = finalize_created_process(
        created?,
        &mut pipes,
        |thread| thread.close(),
        |pipes| pipes.close_parent_stdin(&pipe_api),
        get_process_birth,
    )?;

    Ok(WindowsJobProcess {
        job: Some(job),
        root: finalized.root,
        identity: finalized.identity,
        pipes,
        hard_stop_issued: false,
    })
}

#[cfg(all(windows, feature = "test-hooks"))]
fn test_child_command(mode: &str, arguments: &[String]) -> Result<Vec<String>, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let fixture = current
        .parent()
        .and_then(|directory| directory.parent())
        .map(|directory| directory.join("solstone-system-test-child.exe"))
        .filter(|candidate| candidate.is_file())
        .ok_or_else(|| {
            "could not locate solstone-system-test-child.exe beside test artifacts".to_owned()
        })?;
    let mut command = vec![fixture.to_string_lossy().into_owned(), mode.to_owned()];
    command.extend(arguments.iter().cloned());
    Ok(command)
}

#[cfg(all(windows, feature = "test-hooks"))]
fn read_pipe_for_test(end: &mut super::handle::PipeEndHandle) -> Result<String, String> {
    use std::io::Read;
    use std::os::windows::io::{FromRawHandle, RawHandle};

    let raw = end.take_raw();
    // SAFETY: ownership was removed from `end`, so this File is the sole closer.
    #[allow(unsafe_code)]
    let mut file = unsafe { std::fs::File::from_raw_handle(raw as RawHandle) };
    let mut output = String::new();
    file.read_to_string(&mut output)
        .map_err(|error| error.to_string())?;
    Ok(output)
}

#[cfg(all(windows, feature = "test-hooks"))]
pub(super) fn windows_job_process_no_inheritance_premise_for_test() -> Result<(), String> {
    let command = test_child_command("sleep", &["30".to_owned()])?;
    let mut launch_spec = prepare_launch_spec(&command, &Default::default(), None)
        .map_err(|error| error.to_string())?;
    let jobs = SystemWindowsJobApi;
    let startup_api = SystemWindowsStartupInfoApi;
    let job =
        create_kill_on_close_job_with_limits(&jobs, None).map_err(|error| error.to_string())?;
    let mut startup =
        WindowsStartupInfo::new_job_only(&startup_api, &job).map_err(|error| error.to_string())?;
    let created =
        SystemWindowsCreateProcessApi.create_process(&mut launch_spec, &startup, None, false);
    startup.delete_with(&startup_api);
    let mut created = created.map_err(|error| error.to_string())?;
    created.thread.close().map_err(|error| error.to_string())?;
    if jobs
        .observe_member(created.root.raw(), &job)
        .map_err(|error| error.to_string())?
        != JobMembership::Member
    {
        return Err(
            "JOB_LIST did not atomically enroll the bInheritHandles=FALSE child".to_owned(),
        );
    }
    jobs.terminate(&job, JOB_HARD_STOP_EXIT_CODE)
        .map_err(|error| error.to_string())?;
    if SystemWindowsProcessControlApi
        .wait_for_process(&created.root, JOB_HARD_STOP_TIMEOUT)
        .map_err(|error| error.to_string())?
        != ProcessWait::Signaled
    {
        return Err("JOB_LIST premise child did not stop within the receipt bound".to_owned());
    }
    Ok(())
}

#[cfg(all(windows, feature = "test-hooks"))]
struct TestProcessObserver {
    process: RootProcessHandle,
    birth: ProcessBirth,
}

#[cfg(all(windows, feature = "test-hooks"))]
fn observe_process_for_test(
    owner: &WindowsJobProcess,
    pid: u32,
) -> Result<TestProcessObserver, String> {
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: OpenProcess either returns a newly owned process handle or null.
    #[allow(unsafe_code)]
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        return Err(format!(
            "opening Job member {pid} failed: {}",
            io::Error::last_os_error()
        ));
    }
    let process = RootProcessHandle::new(raw);
    let birth = get_process_birth(&process)
        .map_err(|error| format!("capturing Job member {pid} birth failed: {error}"))?;
    match SystemWindowsJobApi
        .observe_member(
            process.raw(),
            owner.job().map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("querying exact Job membership for {pid} failed: {error}"))?
    {
        JobMembership::Member => Ok(TestProcessObserver { process, birth }),
        JobMembership::NotMember => Err(format!("process {pid} is not in the owner's exact Job")),
    }
}

#[cfg(all(windows, feature = "test-hooks"))]
fn receipt_path(label: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("solstone-{label}-{}-{nonce}", std::process::id()))
}

#[cfg(all(windows, feature = "test-hooks"))]
fn wait_for_receipt_pid(
    owner: &WindowsJobProcess,
    path: &std::path::Path,
    label: &str,
) -> Result<u32, String> {
    let deadline = Instant::now() + JOB_HARD_STOP_TIMEOUT;
    loop {
        match std::fs::read_to_string(path) {
            Ok(value) => {
                return value
                    .trim()
                    .parse()
                    .map_err(|error| format!("invalid {label} PID receipt: {error}"));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("reading {label} PID receipt failed: {error}")),
        }
        if owner.poll().map_err(|error| error.to_string())?.is_some() {
            return Err(format!("Job tree exited before publishing {label} PID"));
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {label} PID receipt"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(all(windows, feature = "test-hooks"))]
fn wait_for_observer_signal(observer: &TestProcessObserver, label: &str) -> Result<(), String> {
    if SystemWindowsProcessControlApi
        .wait_for_process(&observer.process, JOB_HARD_STOP_TIMEOUT)
        .map_err(|error| format!("waiting for {label} observer failed: {error}"))?
        != ProcessWait::Signaled
    {
        return Err(format!(
            "{label} remained live after the final Job handle closed"
        ));
    }
    Ok(())
}

#[cfg(all(windows, feature = "test-hooks"))]
pub(super) fn windows_job_process_owner_receipt_for_test() -> Result<(), String> {
    let jobs = SystemWindowsJobApi;
    let limits = JobResourceLimits {
        cpu_rate_per_10_000: 2_500,
        committed_memory_bytes: 512 * 1024 * 1024,
    };
    let resource_job = create_kill_on_close_job_with_limits(&jobs, Some(limits))
        .map_err(|error| format!("creating limited Job failed: {error}"))?;
    let observed_limits = jobs
        .resource_limits(&resource_job)
        .map_err(|error| format!("reading limited Job configuration failed: {error}"))?;
    if observed_limits.cpu_rate_per_10_000 != limits.cpu_rate_per_10_000
        || observed_limits.committed_memory_bytes != limits.committed_memory_bytes
        || !observed_limits.process_memory_enforced
        || !observed_limits.job_memory_enforced
        || !observed_limits.cpu_hard_cap_enforced
    {
        return Err(format!(
            "limited Job readback did not retain the configured hard limits: {observed_limits:?}"
        ));
    }
    let limited_sleep = test_child_command("sleep", &["30".to_owned()])?;
    let mut limited_launch_spec = prepare_launch_spec(&limited_sleep, &Default::default(), None)
        .map_err(|error| error.to_string())?;
    let startup_api = SystemWindowsStartupInfoApi;
    let mut limited_startup = WindowsStartupInfo::new_job_only(&startup_api, &resource_job)
        .map_err(|error| error.to_string())?;
    let limited_created = SystemWindowsCreateProcessApi.create_process(
        &mut limited_launch_spec,
        &limited_startup,
        None,
        false,
    );
    limited_startup.delete_with(&startup_api);
    let mut limited_created = limited_created.map_err(|error| error.to_string())?;
    limited_created
        .thread
        .close()
        .map_err(|error| error.to_string())?;
    if jobs
        .observe_member(limited_created.root.raw(), &resource_job)
        .map_err(|error| error.to_string())?
        != JobMembership::Member
    {
        return Err("limited Job did not atomically enroll its root process".to_owned());
    }
    jobs.terminate(&resource_job, JOB_HARD_STOP_EXIT_CODE)
        .map_err(|error| error.to_string())?;
    if SystemWindowsProcessControlApi
        .wait_for_process(&limited_created.root, JOB_HARD_STOP_TIMEOUT)
        .map_err(|error| error.to_string())?
        != ProcessWait::Signaled
    {
        return Err("limited Job root did not stop within the receipt bound".to_owned());
    }

    let expected_current_directory =
        std::fs::canonicalize(std::env::temp_dir()).map_err(|error| {
            format!("canonicalizing current-directory receipt root failed: {error}")
        })?;
    if !expected_current_directory.is_absolute() {
        return Err("current-directory receipt root was not absolute".to_owned());
    }
    let current_directory = test_child_command("current-directory", &[])?;
    let mut current_directory_owner = launch_windows_job_process_with_options(
        &current_directory,
        &Default::default(),
        WindowsJobLaunchOptions {
            current_directory: Some(&expected_current_directory),
            resource_limits: None,
        },
    )
    .map_err(|error| error.to_string())?;
    if current_directory_owner
        .wait()
        .map_err(|error| error.to_string())?
        != 0
    {
        return Err("current-directory fixture did not exit successfully".to_owned());
    }
    let current_directory_stdout =
        read_pipe_for_test(&mut current_directory_owner.pipes.parent_stdout_read)?;
    let current_directory_stderr =
        read_pipe_for_test(&mut current_directory_owner.pipes.parent_stderr_read)?;
    if !current_directory_stderr.is_empty() {
        return Err(format!(
            "current-directory fixture wrote stderr: {current_directory_stderr}"
        ));
    }
    let observed_current_directory = std::fs::canonicalize(current_directory_stdout.trim_end())
        .map_err(|error| {
            format!("canonicalizing child current-directory receipt failed: {error}")
        })?;
    if observed_current_directory != expected_current_directory {
        return Err(format!(
            "child current directory did not retain the explicit owned path: expected {}, got {}",
            expected_current_directory.display(),
            observed_current_directory.display()
        ));
    }

    let lines = test_child_command("lines", &[])?;
    let mut lines_owner = launch_windows_job_process(&lines, &Default::default())
        .map_err(|error| error.to_string())?;
    if lines_owner.wait().map_err(|error| error.to_string())? != 0 {
        return Err("line fixture did not exit successfully".to_owned());
    }
    let stdout = read_pipe_for_test(&mut lines_owner.pipes.parent_stdout_read)?;
    let stderr = read_pipe_for_test(&mut lines_owner.pipes.parent_stderr_read)?;
    if !stdout.contains("stdout-line") || !stderr.contains("stderr-line") {
        return Err("stdio pipe receipt did not receive both streams".to_owned());
    }

    let exit_259 = test_child_command("exit-code", &["259".to_owned()])?;
    let owner_259 = launch_windows_job_process(&exit_259, &Default::default())
        .map_err(|error| error.to_string())?;
    if owner_259.wait().map_err(|error| error.to_string())? != 259 {
        return Err("signaled process exit code 259 was not retained as terminal".to_owned());
    }

    let sleeping = test_child_command("sleep", &["30".to_owned()])?;
    let mut owner = launch_windows_job_process(&sleeping, &Default::default())
        .map_err(|error| error.to_string())?;
    let identity = owner.identity();
    if identity.pid == 0 || identity.birth.windows_filetime().is_none() {
        return Err("launched root did not retain a PID and Windows birth identity".to_owned());
    }
    if owner.observe_member().map_err(|error| error.to_string())? != JobMembership::Member {
        return Err("launched root was not a Job member".to_owned());
    }
    if !jobs
        .kill_on_close_enabled(owner.job().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?
    {
        return Err("launched Job does not retain the kill-on-close limit".to_owned());
    }
    if jobs
        .accounting(owner.job().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?
        .active_processes
        == 0
    {
        return Err("newly launched Job reports no active process".to_owned());
    }
    if owner.poll().map_err(|error| error.to_string())?.is_some() {
        return Err("sleeping child was unexpectedly terminal".to_owned());
    }
    let _ = owner.hard_stop().map_err(|error| error.to_string())?;
    if SystemWindowsJobApi
        .accounting(owner.job().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?
        .active_processes
        != 0
    {
        return Err("hard-stop returned before the Job reached zero active processes".to_owned());
    }
    Ok(())
}

#[cfg(all(windows, feature = "test-hooks"))]
pub(super) fn windows_job_duplicate_handle_negative_control_for_test() -> Result<(), String> {
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let root_ready = receipt_path("job-root");
    let grandchild_ready = receipt_path("job-grandchild");
    let tree = test_child_command(
        "job-tree-root",
        &[
            root_ready.to_string_lossy().into_owned(),
            grandchild_ready.to_string_lossy().into_owned(),
        ],
    )?;
    let mut owner = launch_windows_job_process(&tree, &Default::default())
        .map_err(|error| error.to_string())?;
    let root_pid = wait_for_receipt_pid(&owner, &root_ready, "root")?;
    let grandchild_pid = wait_for_receipt_pid(&owner, &grandchild_ready, "grandchild")?;
    let _ = std::fs::remove_file(&root_ready);
    let _ = std::fs::remove_file(&grandchild_ready);
    // SAFETY: GetCurrentProcess returns a valid non-owning pseudo-handle.
    #[allow(unsafe_code)]
    let current_process = unsafe { GetCurrentProcess() };
    if SystemWindowsJobApi
        .observe_member(
            current_process,
            owner.job().map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?
        != JobMembership::NotMember
    {
        return Err("receipt process unexpectedly belongs to the child's Job".to_owned());
    }
    let root_observer = observe_process_for_test(&owner, root_pid)?;
    let grandchild_observer = observe_process_for_test(&owner, grandchild_pid)?;
    if root_observer.birth != owner.identity().birth {
        return Err("root observer birth token did not match the retained owner".to_owned());
    }
    let duplicate = duplicate_job_for_consuming_transition(&owner, |original_raw| {
        let mut duplicate = null_mut();
        // SAFETY: the source Job handle is live and the output pointer is writable.
        #[allow(unsafe_code)]
        let duplicated = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                original_raw,
                GetCurrentProcess(),
                &raw mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(JobHandle::new(duplicate))
    })
    .map_err(|error| error.to_string())?;
    if SystemWindowsJobApi
        .observe_member(root_observer.process.raw(), &duplicate)
        .map_err(|error| error.to_string())?
        != JobMembership::Member
    {
        return Err("duplicate Job handle did not preserve the membership observation".to_owned());
    }
    let original = owner
        .job
        .take()
        .ok_or_else(|| "owner did not retain the Job handle".to_owned())?;
    drop(original);
    drop(owner);
    for (label, observer) in [
        ("root", &root_observer),
        ("grandchild", &grandchild_observer),
    ] {
        if SystemWindowsProcessControlApi
            .wait_for_process(&observer.process, Duration::ZERO)
            .map_err(|error| error.to_string())?
            != ProcessWait::Timeout
        {
            return Err(format!(
                "{label} died before the final duplicate Job handle closed"
            ));
        }
    }
    drop(duplicate);
    wait_for_observer_signal(&root_observer, "root")?;
    wait_for_observer_signal(&grandchild_observer, "grandchild")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::io;
    use std::time::{Duration, Instant};

    use crate::process::{ProcessBirth, ProcessInstance};

    use super::super::handle::{
        JobHandle, PipeEndHandle, PrimaryThreadHandle, RawWindowsHandle, RootProcessHandle,
    };
    use super::super::job::{
        JobAccounting, JobMembership, JobResourceLimitReceipt, JobResourceLimits, WindowsJobApi,
    };
    use super::super::pipes::{PipedStdio, WindowsPipeApi};
    use super::{
        CreatedWindowsProcess, JOB_HARD_STOP_TIMEOUT, ProcessWait, WindowsJobProcess,
        WindowsProcessControlApi, duplicate_job_for_consuming_transition, finalize_created_process,
        map_wait_to_exit, map_windows_wait_result, run_create_window,
    };

    #[derive(Clone, Copy)]
    enum MembershipResult {
        Member,
        NotMember,
        Error,
    }

    struct FakeJobApi {
        terminate_error: Cell<Option<io::ErrorKind>>,
        terminate_calls: Cell<u32>,
        accounting_calls: Cell<u32>,
        accounting: RefCell<VecDeque<u32>>,
        accounting_fallback: u32,
        membership: Cell<MembershipResult>,
    }

    impl FakeJobApi {
        fn new(accounting: impl IntoIterator<Item = u32>, accounting_fallback: u32) -> Self {
            Self {
                terminate_error: Cell::new(None),
                terminate_calls: Cell::new(0),
                accounting_calls: Cell::new(0),
                accounting: RefCell::new(accounting.into_iter().collect()),
                accounting_fallback,
                membership: Cell::new(MembershipResult::Member),
            }
        }
    }

    impl WindowsJobApi for FakeJobApi {
        fn create_unnamed_job(&self) -> io::Result<JobHandle> {
            Ok(JobHandle::new(1usize as RawWindowsHandle))
        }

        fn enable_kill_on_close(&self, _job: &JobHandle) -> io::Result<()> {
            Ok(())
        }

        fn configure_resource_limits(
            &self,
            _job: &JobHandle,
            _limits: JobResourceLimits,
        ) -> io::Result<()> {
            Ok(())
        }

        fn resource_limits(&self, _job: &JobHandle) -> io::Result<JobResourceLimitReceipt> {
            Ok(JobResourceLimitReceipt {
                cpu_rate_per_10_000: 0,
                committed_memory_bytes: 0,
                process_memory_enforced: false,
                job_memory_enforced: false,
                cpu_hard_cap_enforced: false,
            })
        }

        fn kill_on_close_enabled(&self, _job: &JobHandle) -> io::Result<bool> {
            Ok(true)
        }

        fn terminate(&self, _job: &JobHandle, _exit_code: u32) -> io::Result<()> {
            self.terminate_calls.set(self.terminate_calls.get() + 1);
            match self.terminate_error.get() {
                Some(kind) => Err(io::Error::from(kind)),
                None => Ok(()),
            }
        }

        fn accounting(&self, _job: &JobHandle) -> io::Result<JobAccounting> {
            self.accounting_calls.set(self.accounting_calls.get() + 1);
            Ok(JobAccounting {
                active_processes: self
                    .accounting
                    .borrow_mut()
                    .pop_front()
                    .unwrap_or(self.accounting_fallback),
            })
        }

        fn observe_member(
            &self,
            _process: RawWindowsHandle,
            _job: &JobHandle,
        ) -> io::Result<JobMembership> {
            match self.membership.get() {
                MembershipResult::Member => Ok(JobMembership::Member),
                MembershipResult::NotMember => Ok(JobMembership::NotMember),
                MembershipResult::Error => Err(io::Error::other("membership observation failed")),
            }
        }
    }

    struct FakeProcessControlApi {
        wait_results: RefCell<VecDeque<Result<ProcessWait, io::ErrorKind>>>,
        fallback_wait: ProcessWait,
        exit_result: Cell<Result<u32, io::ErrorKind>>,
        wait_calls: Cell<u32>,
        exit_calls: Cell<u32>,
    }

    impl FakeProcessControlApi {
        fn new(
            wait_results: impl IntoIterator<Item = Result<ProcessWait, io::ErrorKind>>,
            fallback_wait: ProcessWait,
            exit_result: Result<u32, io::ErrorKind>,
        ) -> Self {
            Self {
                wait_results: RefCell::new(wait_results.into_iter().collect()),
                fallback_wait,
                exit_result: Cell::new(exit_result),
                wait_calls: Cell::new(0),
                exit_calls: Cell::new(0),
            }
        }
    }

    impl WindowsProcessControlApi for FakeProcessControlApi {
        fn wait_for_process(
            &self,
            _process: &RootProcessHandle,
            timeout: Duration,
        ) -> io::Result<ProcessWait> {
            self.wait_calls.set(self.wait_calls.get() + 1);
            let _ = timeout;
            match self.wait_results.borrow_mut().pop_front() {
                Some(Ok(wait)) => Ok(wait),
                Some(Err(kind)) => Err(io::Error::from(kind)),
                None => Ok(self.fallback_wait),
            }
        }

        fn exit_code(&self, _process: &RootProcessHandle) -> io::Result<u32> {
            self.exit_calls.set(self.exit_calls.get() + 1);
            match self.exit_result.get() {
                Ok(code) => Ok(code),
                Err(kind) => Err(io::Error::from(kind)),
            }
        }
    }

    struct FakePipeApi {
        next: Cell<usize>,
    }

    struct WindowPipeApi {
        next: Cell<usize>,
        trace: RefCell<Vec<String>>,
        fail_clear: Cell<Option<usize>>,
    }

    impl WindowPipeApi {
        fn new() -> Self {
            Self {
                next: Cell::new(1),
                trace: RefCell::new(Vec::new()),
                fail_clear: Cell::new(None),
            }
        }
    }

    impl WindowsPipeApi for WindowPipeApi {
        fn create_pipe(&self) -> io::Result<(PipeEndHandle, PipeEndHandle)> {
            let read = self.next.get();
            self.next.set(read + 1);
            let write = self.next.get();
            self.next.set(write + 1);
            Ok((
                PipeEndHandle::new(read as RawWindowsHandle),
                PipeEndHandle::new(write as RawWindowsHandle),
            ))
        }

        fn set_inherit(&self, end: &PipeEndHandle) -> io::Result<()> {
            self.trace
                .borrow_mut()
                .push(format!("set:{}", end.raw() as usize));
            Ok(())
        }

        fn clear_inherit(&self, end: &PipeEndHandle) -> io::Result<()> {
            let raw = end.raw() as usize;
            self.trace.borrow_mut().push(format!("clear:{raw}"));
            if self.fail_clear.get() == Some(raw) {
                return Err(io::Error::other(format!("clear:{raw}")));
            }
            Ok(())
        }

        fn close_end(&self, end: &mut PipeEndHandle) -> io::Result<()> {
            let raw = end.raw() as usize;
            self.trace.borrow_mut().push(format!("close:{raw}"));
            end.release_for_test();
            Ok(())
        }
    }

    impl WindowsPipeApi for FakePipeApi {
        fn create_pipe(&self) -> io::Result<(PipeEndHandle, PipeEndHandle)> {
            let read = self.next.get();
            self.next.set(read + 1);
            let write = self.next.get();
            self.next.set(write + 1);
            Ok((
                PipeEndHandle::new(read as RawWindowsHandle),
                PipeEndHandle::new(write as RawWindowsHandle),
            ))
        }

        fn set_inherit(&self, _end: &PipeEndHandle) -> io::Result<()> {
            Ok(())
        }

        fn clear_inherit(&self, _end: &PipeEndHandle) -> io::Result<()> {
            Ok(())
        }
    }

    fn owner() -> WindowsJobProcess {
        let pipes = PipedStdio::create(&FakePipeApi { next: Cell::new(2) })
            .expect("fake pipes are created");
        WindowsJobProcess {
            job: Some(JobHandle::new(1usize as RawWindowsHandle)),
            root: RootProcessHandle::new(8usize as RawWindowsHandle),
            identity: ProcessInstance {
                pid: 7,
                birth: ProcessBirth::windows(9),
            },
            pipes,
            hard_stop_issued: false,
        }
    }

    fn created_process() -> CreatedWindowsProcess {
        CreatedWindowsProcess {
            root: RootProcessHandle::new(8usize as RawWindowsHandle),
            thread: PrimaryThreadHandle::new(9usize as RawWindowsHandle),
            pid: 7,
        }
    }

    #[test]
    fn duplicate_failure_preserves_the_complete_owner_for_retry() {
        let owner = owner();
        let result = duplicate_job_for_consuming_transition(&owner, |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "duplicate fault",
            ))
        });
        let error = match result {
            Ok(_) => panic!("duplicate failure did not propagate"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(owner.job.is_some());
        assert_eq!(owner.root.raw() as usize, 8);
        assert_eq!(owner.pipes.parent_stdin_write.raw() as usize, 3);
        assert_eq!(owner.pipes.parent_stdout_read.raw() as usize, 4);
        assert_eq!(owner.pipes.parent_stderr_read.raw() as usize, 6);
    }

    #[test]
    fn created_process_finalization_closes_thread_and_stdin_before_birth_capture() {
        let api = FakePipeApi { next: Cell::new(2) };
        let mut pipes = PipedStdio::create(&api).expect("fake pipes");
        let trace = RefCell::new(Vec::new());
        let finalized = finalize_created_process(
            created_process(),
            &mut pipes,
            |thread| {
                assert_eq!(thread.raw() as usize, 9);
                trace.borrow_mut().push("thread");
                Ok(())
            },
            |_| {
                trace.borrow_mut().push("stdin");
                Ok(())
            },
            |_| {
                trace.borrow_mut().push("birth");
                Ok(ProcessBirth::windows(11))
            },
        )
        .expect("finalization succeeds");

        assert_eq!(*trace.borrow(), ["thread", "stdin", "birth"]);
        assert_eq!(finalized.root.raw() as usize, 8);
        assert_eq!(finalized.identity.pid, 7);
        assert_eq!(finalized.identity.birth, ProcessBirth::windows(11));
    }

    #[test]
    fn each_created_process_finalization_failure_stops_at_the_first_error() {
        for failure in ["thread", "stdin", "birth"] {
            let api = FakePipeApi { next: Cell::new(2) };
            let mut pipes = PipedStdio::create(&api).expect("fake pipes");
            let trace = RefCell::new(Vec::new());
            let result = finalize_created_process(
                created_process(),
                &mut pipes,
                |_| {
                    trace.borrow_mut().push("thread");
                    if failure == "thread" {
                        Err(io::Error::other("thread fault"))
                    } else {
                        Ok(())
                    }
                },
                |_| {
                    trace.borrow_mut().push("stdin");
                    if failure == "stdin" {
                        Err(io::Error::other("stdin fault"))
                    } else {
                        Ok(())
                    }
                },
                |_| {
                    trace.borrow_mut().push("birth");
                    if failure == "birth" {
                        Err(io::Error::other("birth fault"))
                    } else {
                        Ok(ProcessBirth::windows(11))
                    }
                },
            );
            let error = match result {
                Ok(_) => panic!("injected finalization fault did not propagate"),
                Err(error) => error,
            };

            assert!(error.to_string().contains(failure));
            let expected = match failure {
                "thread" => &["thread"][..],
                "stdin" => &["thread", "stdin"][..],
                "birth" => &["thread", "stdin", "birth"][..],
                _ => unreachable!(),
            };
            assert_eq!(&*trace.borrow(), expected);
        }
    }

    #[test]
    fn creation_window_sets_exact_child_bits_calls_once_then_clears_and_closes() {
        let api = WindowPipeApi::new();
        let mut pipes = PipedStdio::create(&api).expect("fake pipes");
        let created = run_create_window(&mut pipes, &api, |_| {
            api.trace.borrow_mut().push("create".to_owned());
            Ok(17)
        })
        .expect("creation window succeeds");

        assert_eq!(created, 17);
        assert_eq!(
            *api.trace.borrow(),
            [
                "set:1", "set:4", "set:6", "create", "clear:1", "close:1", "clear:4", "close:4",
                "clear:6", "close:6"
            ]
        );
    }

    #[test]
    fn creation_error_remains_primary_while_all_child_cleanup_runs() {
        let api = WindowPipeApi::new();
        api.fail_clear.set(Some(1));
        let mut pipes = PipedStdio::create(&api).expect("fake pipes");
        let error = run_create_window::<()>(&mut pipes, &api, |_| {
            api.trace.borrow_mut().push("create".to_owned());
            Err(io::Error::other("create failed"))
        })
        .expect_err("creation fails");

        assert_eq!(error.to_string(), "create failed");
        assert_eq!(
            &api.trace.borrow()[4..],
            [
                "clear:1", "close:1", "clear:4", "close:4", "clear:6", "close:6"
            ]
        );
    }

    #[test]
    fn successful_creation_surfaces_the_first_cleanup_error() {
        let api = WindowPipeApi::new();
        api.fail_clear.set(Some(4));
        let mut pipes = PipedStdio::create(&api).expect("fake pipes");
        let error = run_create_window(&mut pipes, &api, |_| Ok(())).expect_err("cleanup fails");

        assert_eq!(error.to_string(), "clear:4");
        assert_eq!(
            api.trace
                .borrow()
                .iter()
                .filter(|entry| *entry == "clear:6")
                .count(),
            1
        );
        assert_eq!(
            api.trace
                .borrow()
                .iter()
                .filter(|entry| *entry == "close:6")
                .count(),
            1
        );
    }

    #[test]
    fn signaled_process_with_still_active_value_is_terminal() {
        let mapped = map_wait_to_exit(ProcessWait::Signaled, || Ok(259));
        assert_eq!(mapped.expect("exit code is available"), Some(259));
    }

    #[test]
    fn timeout_is_live_without_reading_an_exit_code() {
        let mapped = map_wait_to_exit(ProcessWait::Timeout, || {
            Err(io::Error::other("exit code must not be read"))
        });
        assert_eq!(mapped.expect("wait maps to live"), None);
    }

    #[test]
    fn raw_wait_mapping_separates_failure_from_unexpected_values() {
        assert_eq!(
            map_windows_wait_result(0, io::Error::other("unused")).expect("signaled"),
            ProcessWait::Signaled
        );
        assert_eq!(
            map_windows_wait_result(258, io::Error::other("unused")).expect("timeout"),
            ProcessWait::Timeout
        );
        let failed = map_windows_wait_result(
            u32::MAX,
            io::Error::new(io::ErrorKind::PermissionDenied, "wait failed"),
        )
        .expect_err("WAIT_FAILED uses last error");
        assert_eq!(failed.kind(), io::ErrorKind::PermissionDenied);
        let unexpected = map_windows_wait_result(17, io::Error::other("stale last error"))
            .expect_err("unexpected raw wait is contextual");
        assert_eq!(
            unexpected.to_string(),
            "WaitForSingleObject returned unexpected value 0x00000011"
        );
    }

    #[test]
    fn poll_with_timeout_is_live_without_reading_an_exit_code() {
        let owner = owner();
        let process = FakeProcessControlApi::new([], ProcessWait::Timeout, Ok(99));

        assert_eq!(owner.poll_with(&process).expect("timeout is live"), None);
        assert_eq!(process.wait_calls.get(), 1);
        assert_eq!(process.exit_calls.get(), 0);
    }

    #[test]
    fn wait_with_signaled_process_preserves_the_full_exit_code_bits() {
        let owner = owner();
        let process = FakeProcessControlApi::new(
            [Ok(ProcessWait::Signaled)],
            ProcessWait::Signaled,
            Ok(0xffff_ff80),
        );

        assert_eq!(owner.wait_with(&process).expect("signaled exit"), -128);
        assert_eq!(process.exit_calls.get(), 1);
    }

    #[test]
    fn wait_with_process_error_preserves_owner_state() {
        let owner = owner();
        let process = FakeProcessControlApi::new(
            [Err(io::ErrorKind::PermissionDenied)],
            ProcessWait::Timeout,
            Ok(0),
        );

        let error = owner
            .wait_with(&process)
            .expect_err("wait error propagates");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(owner.job.is_some());
        assert!(!owner.hard_stop_issued);
    }

    #[test]
    fn hard_stop_retries_until_descendants_are_quiescent() {
        let mut owner = owner();
        let jobs = FakeJobApi::new([2, 1, 0], 0);
        let process = FakeProcessControlApi::new([], ProcessWait::Signaled, Ok(0xf0f0_0001));

        assert_eq!(
            owner
                .hard_stop_with(&jobs, &process, Instant::now() + JOB_HARD_STOP_TIMEOUT,)
                .expect("Job reaches zero"),
            i32::from_ne_bytes(0xf0f0_0001u32.to_ne_bytes())
        );
        assert_eq!(jobs.terminate_calls.get(), 1);
        assert_eq!(jobs.accounting_calls.get(), 3);
        assert_eq!(process.wait_calls.get(), 1);
        assert_eq!(process.exit_calls.get(), 1);
        assert!(owner.hard_stop_issued);
    }

    #[test]
    fn hard_stop_times_out_when_active_processes_never_reach_zero() {
        let mut owner = owner();
        let jobs = FakeJobApi::new([], 1);
        let process = FakeProcessControlApi::new([], ProcessWait::Signaled, Ok(0));
        let started = Instant::now();
        let timeout = Duration::from_millis(25);

        let error = owner
            .hard_stop_with(&jobs, &process, started + timeout)
            .expect_err("active Job must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= timeout);
        assert_eq!(jobs.terminate_calls.get(), 1);
        assert!(owner.hard_stop_issued);
    }

    #[test]
    fn hard_stop_retry_after_proof_timeout_does_not_terminate_twice() {
        let mut owner = owner();
        let first_jobs = FakeJobApi::new([], 1);
        let process = FakeProcessControlApi::new([], ProcessWait::Signaled, Ok(0));

        owner
            .hard_stop_with(
                &first_jobs,
                &process,
                Instant::now() + Duration::from_millis(1),
            )
            .expect_err("first proof times out");
        let retry_jobs = FakeJobApi::new([0], 0);
        assert_eq!(
            owner
                .hard_stop_with(
                    &retry_jobs,
                    &process,
                    Instant::now() + JOB_HARD_STOP_TIMEOUT,
                )
                .expect("proof retry succeeds"),
            0
        );
        assert_eq!(first_jobs.terminate_calls.get(), 1);
        assert_eq!(retry_jobs.terminate_calls.get(), 0);
    }

    #[test]
    fn failed_termination_leaves_hard_stop_retryable() {
        let mut owner = owner();
        let jobs = FakeJobApi::new([0], 0);
        jobs.terminate_error
            .set(Some(io::ErrorKind::PermissionDenied));
        let process = FakeProcessControlApi::new([], ProcessWait::Signaled, Ok(0));

        let error = owner
            .hard_stop_with(&jobs, &process, Instant::now() + JOB_HARD_STOP_TIMEOUT)
            .expect_err("terminate failure propagates");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!owner.hard_stop_issued);

        jobs.terminate_error.set(None);
        assert_eq!(
            owner
                .hard_stop_with(&jobs, &process, Instant::now() + JOB_HARD_STOP_TIMEOUT,)
                .expect("retry succeeds"),
            0
        );
        assert_eq!(jobs.terminate_calls.get(), 2);
    }

    #[test]
    fn observe_member_with_preserves_member_not_member_and_errors() {
        let owner = owner();
        let jobs = FakeJobApi::new([], 0);

        assert_eq!(
            owner.observe_member_with(&jobs).expect("member result"),
            JobMembership::Member
        );
        jobs.membership.set(MembershipResult::NotMember);
        assert_eq!(
            owner.observe_member_with(&jobs).expect("not-member result"),
            JobMembership::NotMember
        );
        jobs.membership.set(MembershipResult::Error);
        assert_eq!(
            owner
                .observe_member_with(&jobs)
                .expect_err("error result")
                .kind(),
            io::ErrorKind::Other
        );
        assert!(owner.job.is_some());
    }
}
