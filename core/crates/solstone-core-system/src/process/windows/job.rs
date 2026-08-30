// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows Job creation and lifetime operations.

use std::io;

use super::handle::{JobHandle, RawWindowsHandle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct JobAccounting {
    pub(super) active_processes: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum JobMembership {
    Member,
    NotMember,
}

/// Narrow seam around the Job APIs so ordering and error behavior are tested
/// without requiring a Windows host.
pub(super) trait WindowsJobApi {
    fn create_unnamed_job(&self) -> io::Result<JobHandle>;
    fn enable_kill_on_close(&self, job: &JobHandle) -> io::Result<()>;
    fn kill_on_close_enabled(&self, job: &JobHandle) -> io::Result<bool>;
    fn terminate(&self, job: &JobHandle, exit_code: u32) -> io::Result<()>;
    fn accounting(&self, job: &JobHandle) -> io::Result<JobAccounting>;
    fn observe_member(
        &self,
        process: RawWindowsHandle,
        job: &JobHandle,
    ) -> io::Result<JobMembership>;
}

pub(super) fn create_kill_on_close_job(api: &impl WindowsJobApi) -> io::Result<JobHandle> {
    let job = api.create_unnamed_job()?;
    api.enable_kill_on_close(&job)?;
    Ok(job)
}

pub(super) struct SystemWindowsJobApi;

#[cfg(windows)]
impl WindowsJobApi for SystemWindowsJobApi {
    fn create_unnamed_job(&self) -> io::Result<JobHandle> {
        use std::ptr::null;

        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::JobObjects::CreateJobObjectW;

        // SAFETY: the security attributes and name are null for an unnamed,
        // default-security Job, and the returned handle becomes owned here.
        #[allow(unsafe_code)]
        let raw = unsafe { CreateJobObjectW(null(), null()) };
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(JobHandle::new(raw))
    }

    fn enable_kill_on_close(&self, job: &JobHandle) -> io::Result<()> {
        use std::mem::size_of;

        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is initialized and remains live for this synchronous call.
        #[allow(unsafe_code)]
        let set = unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn terminate(&self, job: &JobHandle, exit_code: u32) -> io::Result<()> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: `job` owns a valid Job handle for this synchronous call.
        #[allow(unsafe_code)]
        let terminated = unsafe { TerminateJobObject(job.raw(), exit_code) };
        if terminated == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn kill_on_close_enabled(&self, job: &JobHandle) -> io::Result<bool> {
        use std::mem::size_of;
        use std::ptr::null_mut;

        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, QueryInformationJobObject,
        };

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: `limits` is a correctly sized writable result buffer.
        #[allow(unsafe_code)]
        let queried = unsafe {
            QueryInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&raw mut limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                null_mut(),
            )
        };
        if queried == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0)
    }

    fn accounting(&self, job: &JobHandle) -> io::Result<JobAccounting> {
        use std::mem::size_of;
        use std::ptr::null_mut;

        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
            QueryInformationJobObject,
        };

        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: `accounting` is a correctly sized writable result buffer.
        #[allow(unsafe_code)]
        let queried = unsafe {
            QueryInformationJobObject(
                job.raw(),
                JobObjectBasicAccountingInformation,
                (&raw mut accounting).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        };
        if queried == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(JobAccounting {
            active_processes: accounting.ActiveProcesses,
        })
    }

    fn observe_member(
        &self,
        process: RawWindowsHandle,
        job: &JobHandle,
    ) -> io::Result<JobMembership> {
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;

        let mut member = 0;
        // SAFETY: both handles are live and `member` is writable for the call.
        #[allow(unsafe_code)]
        let observed = unsafe { IsProcessInJob(process, job.raw(), &raw mut member) };
        if observed == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(if member == 0 {
            JobMembership::NotMember
        } else {
            JobMembership::Member
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io;

    use super::{
        JobAccounting, JobHandle, JobMembership, RawWindowsHandle, WindowsJobApi,
        create_kill_on_close_job,
    };

    struct FakeJobApi {
        enabled: Cell<bool>,
    }

    impl WindowsJobApi for FakeJobApi {
        fn create_unnamed_job(&self) -> io::Result<JobHandle> {
            Ok(JobHandle::new(1usize as RawWindowsHandle))
        }

        fn enable_kill_on_close(&self, _job: &JobHandle) -> io::Result<()> {
            self.enabled.set(true);
            Ok(())
        }

        fn kill_on_close_enabled(&self, _job: &JobHandle) -> io::Result<bool> {
            Ok(self.enabled.get())
        }

        fn terminate(&self, _job: &JobHandle, _exit_code: u32) -> io::Result<()> {
            Ok(())
        }

        fn accounting(&self, _job: &JobHandle) -> io::Result<JobAccounting> {
            Ok(JobAccounting {
                active_processes: 0,
            })
        }

        fn observe_member(
            &self,
            _process: RawWindowsHandle,
            _job: &JobHandle,
        ) -> io::Result<JobMembership> {
            Ok(JobMembership::Member)
        }
    }

    #[test]
    fn creation_enables_kill_on_close_before_returning_the_handle() {
        let api = FakeJobApi {
            enabled: Cell::new(false),
        };
        let _job = create_kill_on_close_job(&api).expect("job is created");
        assert!(api.enabled.get());
    }
}
