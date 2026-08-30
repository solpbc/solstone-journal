// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Atomic Job-contained Windows process launch and lifecycle ownership.

use std::io;
use std::time::{Duration, Instant};

use crate::process::{ProcessBirth, ProcessInstance};

use super::handle::{JobHandle, PrimaryThreadHandle, RootProcessHandle};
use super::identity::{WindowsFileTime, filetime_value};
use super::job::{JobMembership, SystemWindowsJobApi, WindowsJobApi, create_kill_on_close_job};
use super::pipes::{PipedStdio, SystemWindowsPipeApi, WindowsPipeApi};
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

pub(super) trait WindowsProcessControlApi {
    fn wait_for_process(
        &self,
        process: &RootProcessHandle,
        timeout: Duration,
    ) -> io::Result<ProcessWait>;
    fn exit_code(&self, process: &RootProcessHandle) -> io::Result<u32>;
}

pub(super) struct SystemWindowsProcessControlApi;

#[cfg(windows)]
impl WindowsProcessControlApi for SystemWindowsProcessControlApi {
    fn wait_for_process(
        &self,
        process: &RootProcessHandle,
        timeout: Duration,
    ) -> io::Result<ProcessWait> {
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        let milliseconds = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
        // SAFETY: `process` owns a live process handle for this synchronous wait.
        #[allow(unsafe_code)]
        let result = unsafe { WaitForSingleObject(process.raw(), milliseconds) };
        match result {
            WAIT_TIMEOUT => Ok(ProcessWait::Timeout),
            WAIT_OBJECT_0 => Ok(ProcessWait::Signaled),
            _ => Err(io::Error::last_os_error()),
        }
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

/// All process-owned objects retained after a successful atomic launch.
pub(super) struct WindowsJobProcess {
    // Option permits the test-only duplicate-handle negative control to close
    // the original without manufacturing a second production ownership path.
    job: Option<JobHandle>,
    root: RootProcessHandle,
    identity: ProcessInstance,
    pipes: PipedStdio,
    hard_stop_issued: bool,
}

impl WindowsJobProcess {
    pub(super) fn identity(&self) -> ProcessInstance {
        self.identity.clone()
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
    ) -> io::Result<i32> {
        if self.hard_stop_issued {
            return Err(io::Error::other("hard stop was already issued"));
        }
        jobs.terminate(self.job()?, JOB_HARD_STOP_EXIT_CODE)?;
        self.hard_stop_issued = true;

        let deadline = Instant::now() + JOB_HARD_STOP_TIMEOUT;
        loop {
            if jobs.accounting(self.job()?)?.active_processes == 0 {
                return process
                    .exit_code(&self.root)
                    .map(|code| i32::from_ne_bytes(code.to_ne_bytes()));
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Job did not reach zero active processes after hard stop",
                ));
            };
            let _ =
                process.wait_for_process(&self.root, remaining.min(Duration::from_millis(20)))?;
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
        self.hard_stop_with(&SystemWindowsJobApi, &SystemWindowsProcessControlApi)
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

#[cfg(windows)]
struct CreatedWindowsProcess {
    root: RootProcessHandle,
    thread: PrimaryThreadHandle,
    pid: u32,
}

#[cfg(windows)]
trait WindowsCreateProcessApi {
    fn create_process(
        &self,
        launch_spec: &mut super::launch_spec::WindowsLaunchSpec,
        startup: &WindowsStartupInfo,
        pipes: &PipedStdio,
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
        pipes: &PipedStdio,
    ) -> io::Result<CreatedWindowsProcess> {
        use std::ptr::null;

        use windows_sys::Win32::System::Threading::{
            CREATE_UNICODE_ENVIRONMENT, CreateProcessW, EXTENDED_STARTUPINFO_PRESENT,
            PROCESS_INFORMATION,
        };

        let startup_info = startup.as_startup_info(
            pipes.child_stdin_read().raw(),
            pipes.child_stdout_write().raw(),
            pipes.child_stderr_write().raw(),
        );
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
                1,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                launch_spec.environment().as_ptr().cast(),
                null(),
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
pub(super) fn launch_windows_job_process(
    command: &[String],
    environment_overrides: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> io::Result<WindowsJobProcess> {
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

    let jobs = SystemWindowsJobApi;
    let pipe_api = SystemWindowsPipeApi;
    let startup_api = SystemWindowsStartupInfoApi;
    let job = create_kill_on_close_job(&jobs)?;
    let mut pipes = PipedStdio::create(&pipe_api)?;
    let mut startup = WindowsStartupInfo::new(
        &startup_api,
        &job,
        pipes.child_stdin_read(),
        pipes.child_stdout_write(),
        pipes.child_stderr_write(),
    )?;
    let mut created =
        SystemWindowsCreateProcessApi.create_process(&mut launch_spec, &startup, &pipes)?;
    let birth = get_process_birth(&created.root)?;
    created.thread.close()?;
    pipes.close_child_ends(&pipe_api)?;
    startup.delete_with(&startup_api);

    Ok(WindowsJobProcess {
        job: Some(job),
        root: created.root,
        identity: ProcessInstance {
            pid: created.pid,
            birth,
        },
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
    let pipe_api = SystemWindowsPipeApi;
    let (decoy_read, _decoy_write) = pipe_api
        .create_inheritable_pipe()
        .map_err(|error| error.to_string())?;
    let command = test_child_command(
        "probe-handle-absent",
        &[format!("{}", decoy_read.raw() as usize)],
    )?;
    let mut owner = launch_windows_job_process(&command, &Default::default())
        .map_err(|error| error.to_string())?;
    owner.wait().map_err(|error| error.to_string())?;
    let output = read_pipe_for_test(&mut owner.pipes.parent_stdout_read)?;
    if output.trim() != "absent" {
        return Err(format!(
            "decoy inheritable handle survived child launch: {output:?}"
        ));
    }
    Ok(())
}

#[cfg(all(windows, feature = "test-hooks"))]
pub(super) fn windows_job_process_owner_receipt_for_test() -> Result<(), String> {
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
    if !SystemWindowsJobApi
        .kill_on_close_enabled(owner.job().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?
    {
        return Err("launched Job does not retain the kill-on-close limit".to_owned());
    }
    if SystemWindowsJobApi
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

    let sleeping = test_child_command("sleep", &["30".to_owned()])?;
    let mut owner = launch_windows_job_process(&sleeping, &Default::default())
        .map_err(|error| error.to_string())?;
    let original = owner
        .job
        .take()
        .ok_or_else(|| "owner did not retain the Job handle".to_owned())?;
    let mut duplicate = null_mut();
    // SAFETY: the source Job handle is live and the output pointer is writable.
    #[allow(unsafe_code)]
    let duplicated = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            original.raw(),
            GetCurrentProcess(),
            &raw mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if duplicated == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    drop(original);
    if owner.poll().map_err(|error| error.to_string())?.is_some() {
        return Err("closing one of two Job handles unexpectedly killed the child".to_owned());
    }
    let duplicate = JobHandle::new(duplicate);
    if SystemWindowsJobApi
        .observe_member(owner.root.raw(), &duplicate)
        .map_err(|error| error.to_string())?
        != JobMembership::Member
    {
        return Err("duplicate Job handle did not preserve the membership observation".to_owned());
    }
    drop(duplicate);
    let _ = owner.wait().map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::io;
    use std::time::{Duration, Instant};

    use crate::process::{ProcessBirth, ProcessInstance};

    use super::super::handle::{JobHandle, PipeEndHandle, RawWindowsHandle, RootProcessHandle};
    use super::super::job::{JobAccounting, JobMembership, WindowsJobApi};
    use super::super::pipes::{PipedStdio, WindowsPipeApi};
    use super::{
        JOB_HARD_STOP_TIMEOUT, ProcessWait, WindowsJobProcess, WindowsProcessControlApi,
        map_wait_to_exit,
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
        delay_wait: bool,
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
                delay_wait: false,
            }
        }

        fn delayed(mut self) -> Self {
            self.delay_wait = true;
            self
        }
    }

    impl WindowsProcessControlApi for FakeProcessControlApi {
        fn wait_for_process(
            &self,
            _process: &RootProcessHandle,
            timeout: Duration,
        ) -> io::Result<ProcessWait> {
            self.wait_calls.set(self.wait_calls.get() + 1);
            if self.delay_wait {
                std::thread::sleep(timeout);
            }
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

    impl WindowsPipeApi for FakePipeApi {
        fn create_inheritable_pipe(&self) -> io::Result<(PipeEndHandle, PipeEndHandle)> {
            let read = self.next.get();
            self.next.set(read + 1);
            let write = self.next.get();
            self.next.set(write + 1);
            Ok((
                PipeEndHandle::new(read as RawWindowsHandle),
                PipeEndHandle::new(write as RawWindowsHandle),
            ))
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
                .hard_stop_with(&jobs, &process)
                .expect("Job reaches zero"),
            i32::from_ne_bytes(0xf0f0_0001u32.to_ne_bytes())
        );
        assert_eq!(jobs.terminate_calls.get(), 1);
        assert_eq!(jobs.accounting_calls.get(), 3);
        assert_eq!(process.wait_calls.get(), 2);
        assert_eq!(process.exit_calls.get(), 1);
        assert!(owner.hard_stop_issued);
    }

    #[test]
    fn hard_stop_times_out_when_active_processes_never_reach_zero() {
        let mut owner = owner();
        let jobs = FakeJobApi::new([], 1);
        let process = FakeProcessControlApi::new([], ProcessWait::Timeout, Ok(0)).delayed();
        let started = Instant::now();

        let error = owner
            .hard_stop_with(&jobs, &process)
            .expect_err("active Job must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= JOB_HARD_STOP_TIMEOUT);
        assert_eq!(jobs.terminate_calls.get(), 1);
        assert!(owner.hard_stop_issued);
    }

    #[test]
    fn hard_stop_rejects_a_second_termination_request() {
        let mut owner = owner();
        let jobs = FakeJobApi::new([0], 0);
        let process = FakeProcessControlApi::new([], ProcessWait::Signaled, Ok(0));

        assert_eq!(
            owner.hard_stop_with(&jobs, &process).expect("first stop"),
            0
        );
        let error = owner
            .hard_stop_with(&jobs, &process)
            .expect_err("second stop is rejected");
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(jobs.terminate_calls.get(), 1);
    }

    #[test]
    fn failed_termination_leaves_hard_stop_retryable() {
        let mut owner = owner();
        let jobs = FakeJobApi::new([0], 0);
        jobs.terminate_error
            .set(Some(io::ErrorKind::PermissionDenied));
        let process = FakeProcessControlApi::new([], ProcessWait::Signaled, Ok(0));

        let error = owner
            .hard_stop_with(&jobs, &process)
            .expect_err("terminate failure propagates");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!owner.hard_stop_issued);

        jobs.terminate_error.set(None);
        assert_eq!(
            owner
                .hard_stop_with(&jobs, &process)
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
