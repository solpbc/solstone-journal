// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows Job creation and lifetime operations.

use std::io;

use super::handle::{JobHandle, RawWindowsHandle};

/// Limits that must be fully installed on a Job before a helper's first
/// instruction. A bounded helper gets one hard CPU share plus equal process
/// and complete-Job committed-memory ceilings, so a descendant cannot evade
/// the root's memory boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct JobResourceLimits {
    pub(super) cpu_rate_per_10_000: u32,
    pub(super) committed_memory_bytes: usize,
}

impl JobResourceLimits {
    fn validate(self) -> io::Result<()> {
        if !(1..=10_000).contains(&self.cpu_rate_per_10_000) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Job CPU rate must be within 1..=10000 per 10000 cycles",
            ));
        }
        if self.committed_memory_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Job committed-memory ceiling must be nonzero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct JobAccounting {
    pub(super) active_processes: u32,
}

/// The limit state read back from an existing Job for a native receipt.
#[cfg(any(test, all(windows, feature = "test-hooks")))]
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct JobResourceLimitReceipt {
    pub(super) cpu_rate_per_10_000: u32,
    pub(super) committed_memory_bytes: usize,
    pub(super) process_memory_enforced: bool,
    pub(super) job_memory_enforced: bool,
    pub(super) cpu_hard_cap_enforced: bool,
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
    fn configure_resource_limits(
        &self,
        job: &JobHandle,
        limits: JobResourceLimits,
    ) -> io::Result<()>;
    #[cfg(any(test, all(windows, feature = "test-hooks")))]
    #[cfg_attr(not(windows), allow(dead_code))]
    fn resource_limits(&self, job: &JobHandle) -> io::Result<JobResourceLimitReceipt>;
    #[cfg_attr(not(windows), allow(dead_code))]
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
    create_kill_on_close_job_with_limits(api, None)
}

/// Create a kill-on-close Job and install every requested resource limit
/// before handing its handle to the atomic process-launch boundary.
pub(super) fn create_kill_on_close_job_with_limits(
    api: &impl WindowsJobApi,
    limits: Option<JobResourceLimits>,
) -> io::Result<JobHandle> {
    if let Some(limits) = limits {
        limits.validate()?;
    }
    let job = api.create_unnamed_job()?;
    api.enable_kill_on_close(&job)?;
    if let Some(limits) = limits {
        api.configure_resource_limits(&job, limits)?;
    }
    Ok(job)
}

#[cfg(windows)]
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

    fn configure_resource_limits(
        &self,
        job: &JobHandle,
        limits: JobResourceLimits,
    ) -> io::Result<()> {
        use std::mem::size_of;

        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectCpuRateControlInformation,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };

        let mut extended = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        extended.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY;
        extended.ProcessMemoryLimit = limits.committed_memory_bytes;
        extended.JobMemoryLimit = limits.committed_memory_bytes;
        // SAFETY: `extended` is initialized and remains live for the synchronous call.
        #[allow(unsafe_code)]
        let memory_set = unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&raw const extended).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if memory_set == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        cpu.ControlFlags =
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
        cpu.Anonymous.CpuRate = limits.cpu_rate_per_10_000;
        // SAFETY: `cpu` is initialized and remains live for the synchronous call.
        #[allow(unsafe_code)]
        let cpu_set = unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectCpuRateControlInformation,
                (&raw const cpu).cast(),
                size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
            )
        };
        if cpu_set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(any(test, all(windows, feature = "test-hooks")))]
    fn resource_limits(&self, job: &JobHandle) -> io::Result<JobResourceLimitReceipt> {
        use std::mem::size_of;

        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
            JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectCpuRateControlInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject,
        };

        let mut extended = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: `extended` is writable for the exact information-class size and
        // the synchronous query does not retain its pointer.
        #[allow(unsafe_code)]
        let memory_read = unsafe {
            QueryInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&raw mut extended).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if memory_read == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        // SAFETY: `cpu` is writable for the exact information-class size and
        // the synchronous query does not retain its pointer.
        #[allow(unsafe_code)]
        let cpu_read = unsafe {
            QueryInformationJobObject(
                job.raw(),
                JobObjectCpuRateControlInformation,
                (&raw mut cpu).cast(),
                size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if cpu_read == 0 {
            return Err(io::Error::last_os_error());
        }

        let limit_flags = extended.BasicLimitInformation.LimitFlags;
        // SAFETY: this query initialized the CPU-rate information structure for
        // the matching information class immediately above.
        #[allow(unsafe_code)]
        let cpu_rate_per_10_000 = unsafe { cpu.Anonymous.CpuRate };
        Ok(JobResourceLimitReceipt {
            cpu_rate_per_10_000,
            committed_memory_bytes: extended.JobMemoryLimit,
            process_memory_enforced: limit_flags & JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0,
            job_memory_enforced: limit_flags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0,
            cpu_hard_cap_enforced: cpu.ControlFlags
                & (JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP)
                == (JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP),
        })
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
    use std::cell::{Cell, RefCell};
    use std::io;

    use super::{
        JobAccounting, JobHandle, JobMembership, JobResourceLimitReceipt, JobResourceLimits,
        RawWindowsHandle, WindowsJobApi, create_kill_on_close_job,
        create_kill_on_close_job_with_limits,
    };

    struct FakeJobApi {
        enabled: Cell<bool>,
        setup: RefCell<Vec<&'static str>>,
    }

    impl WindowsJobApi for FakeJobApi {
        fn create_unnamed_job(&self) -> io::Result<JobHandle> {
            self.setup.borrow_mut().push("create");
            Ok(JobHandle::new(1usize as RawWindowsHandle))
        }

        fn enable_kill_on_close(&self, _job: &JobHandle) -> io::Result<()> {
            self.setup.borrow_mut().push("kill-on-close");
            self.enabled.set(true);
            Ok(())
        }

        fn configure_resource_limits(
            &self,
            _job: &JobHandle,
            _limits: JobResourceLimits,
        ) -> io::Result<()> {
            self.setup.borrow_mut().push("resource-limits");
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
            setup: RefCell::new(Vec::new()),
        };
        let _job = create_kill_on_close_job(&api).expect("job is created");
        assert!(api.enabled.get());
        assert_eq!(*api.setup.borrow(), ["create", "kill-on-close"]);
    }

    #[test]
    fn limited_creation_installs_every_limit_before_returning_the_job() {
        let api = FakeJobApi {
            enabled: Cell::new(false),
            setup: RefCell::new(Vec::new()),
        };
        let limits = JobResourceLimits {
            cpu_rate_per_10_000: 2_500,
            committed_memory_bytes: 512 * 1024 * 1024,
        };
        let _job = create_kill_on_close_job_with_limits(&api, Some(limits))
            .expect("limited job is created");

        assert!(api.enabled.get());
        assert_eq!(
            *api.setup.borrow(),
            ["create", "kill-on-close", "resource-limits"]
        );
    }

    #[test]
    fn resource_limits_refuse_zero_and_out_of_range_values_before_job_creation() {
        for cpu_rate_per_10_000 in [0, 10_001] {
            let api = FakeJobApi {
                enabled: Cell::new(false),
                setup: RefCell::new(Vec::new()),
            };
            let error = match create_kill_on_close_job_with_limits(
                &api,
                Some(JobResourceLimits {
                    cpu_rate_per_10_000,
                    committed_memory_bytes: 1,
                }),
            ) {
                Ok(_) => panic!("invalid CPU rate must refuse before creating a Job"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(api.setup.borrow().is_empty());
        }
        let api = FakeJobApi {
            enabled: Cell::new(false),
            setup: RefCell::new(Vec::new()),
        };
        let error = match create_kill_on_close_job_with_limits(
            &api,
            Some(JobResourceLimits {
                cpu_rate_per_10_000: 1,
                committed_memory_bytes: 0,
            }),
        ) {
            Ok(_) => panic!("zero memory limit must refuse before creating a Job"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(api.setup.borrow().is_empty());
    }
}
