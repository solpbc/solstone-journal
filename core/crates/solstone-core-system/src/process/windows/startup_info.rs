// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Extended startup information with Job and inheritable-handle attributes.

use std::ffi::c_void;
use std::io;

use super::handle::{JobHandle, PipeEndHandle, RawWindowsHandle};

#[repr(align(16))]
struct AttributeStorage([u8; 16]);

pub(super) trait WindowsStartupInfoApi {
    fn attribute_list_size(&self, attribute_count: u32) -> io::Result<usize>;
    fn initialize_attribute_list(
        &self,
        list: *mut c_void,
        attribute_count: u32,
        bytes: usize,
    ) -> io::Result<()>;
    fn update_job_list(
        &self,
        list: *mut c_void,
        jobs: *const RawWindowsHandle,
        bytes: usize,
    ) -> io::Result<()>;
    fn update_handle_list(
        &self,
        list: *mut c_void,
        handles: *const RawWindowsHandle,
        bytes: usize,
    ) -> io::Result<()>;
    fn delete_attribute_list(&self, list: *mut c_void);
}

/// Stable backing storage for the two process-thread attribute values.
pub(super) struct WindowsStartupInfo {
    _storage: Vec<AttributeStorage>,
    list: *mut c_void,
    initialized: bool,
    job_list: Box<[RawWindowsHandle; 1]>,
    handle_list: Box<[RawWindowsHandle; 3]>,
}

impl WindowsStartupInfo {
    pub(super) fn new(
        api: &impl WindowsStartupInfoApi,
        job: &JobHandle,
        child_stdin: &PipeEndHandle,
        child_stdout: &PipeEndHandle,
        child_stderr: &PipeEndHandle,
    ) -> io::Result<Self> {
        const ATTRIBUTE_COUNT: u32 = 2;

        let required = api.attribute_list_size(ATTRIBUTE_COUNT)?;
        let slot_count = required.div_ceil(std::mem::size_of::<AttributeStorage>());
        let mut storage = Vec::with_capacity(slot_count);
        storage.resize_with(slot_count, || AttributeStorage([0; 16]));
        let list = storage.as_mut_ptr().cast();
        let job_list = Box::new([job.raw()]);
        // The Job is deliberately absent from this list: it belongs only to
        // PROC_THREAD_ATTRIBUTE_JOB_LIST, never to the inheritable handles.
        let handle_list = Box::new([child_stdin.raw(), child_stdout.raw(), child_stderr.raw()]);

        api.initialize_attribute_list(list, ATTRIBUTE_COUNT, required)?;
        let startup = Self {
            _storage: storage,
            list,
            initialized: true,
            job_list,
            handle_list,
        };
        api.update_job_list(
            startup.list,
            startup.job_list.as_ptr(),
            std::mem::size_of_val(startup.job_list.as_ref()),
        )?;
        api.update_handle_list(
            startup.list,
            startup.handle_list.as_ptr(),
            std::mem::size_of_val(startup.handle_list.as_ref()),
        )?;
        Ok(startup)
    }

    pub(super) fn delete_with(&mut self, api: &impl WindowsStartupInfoApi) {
        if self.initialized {
            api.delete_attribute_list(self.list);
            self.initialized = false;
        }
    }

    #[cfg(windows)]
    pub(super) fn as_startup_info(
        &self,
        child_stdin: RawWindowsHandle,
        child_stdout: RawWindowsHandle,
        child_stderr: RawWindowsHandle,
    ) -> windows_sys::Win32::System::Threading::STARTUPINFOEXW {
        use std::mem::size_of;

        use windows_sys::Win32::System::Threading::{STARTF_USESTDHANDLES, STARTUPINFOEXW};

        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = child_stdin;
        startup.StartupInfo.hStdOutput = child_stdout;
        startup.StartupInfo.hStdError = child_stderr;
        startup.lpAttributeList = self.list;
        startup
    }
}

impl Drop for WindowsStartupInfo {
    fn drop(&mut self) {
        #[cfg(windows)]
        self.delete_with(&SystemWindowsStartupInfoApi);
    }
}

pub(super) struct SystemWindowsStartupInfoApi;

#[cfg(windows)]
impl WindowsStartupInfoApi for SystemWindowsStartupInfoApi {
    fn attribute_list_size(&self, attribute_count: u32) -> io::Result<usize> {
        use std::ptr::null_mut;

        use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
        use windows_sys::Win32::System::Threading::InitializeProcThreadAttributeList;

        let mut bytes = 0;
        // SAFETY: the documented sizing call accepts a null list and writes
        // only the required byte count.
        #[allow(unsafe_code)]
        let initialized = unsafe {
            InitializeProcThreadAttributeList(null_mut(), attribute_count, 0, &raw mut bytes)
        };
        if initialized != 0
            || io::Error::last_os_error().raw_os_error() == Some(ERROR_INSUFFICIENT_BUFFER as i32)
        {
            return Ok(bytes);
        }
        Err(io::Error::last_os_error())
    }

    fn initialize_attribute_list(
        &self,
        list: *mut c_void,
        attribute_count: u32,
        mut bytes: usize,
    ) -> io::Result<()> {
        use windows_sys::Win32::System::Threading::InitializeProcThreadAttributeList;

        // SAFETY: `list` points to a stable allocation of `bytes` bytes.
        #[allow(unsafe_code)]
        let initialized =
            unsafe { InitializeProcThreadAttributeList(list, attribute_count, 0, &raw mut bytes) };
        if initialized == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn update_job_list(
        &self,
        list: *mut c_void,
        jobs: *const RawWindowsHandle,
        bytes: usize,
    ) -> io::Result<()> {
        use std::ptr::null_mut;

        use windows_sys::Win32::System::Threading::{
            PROC_THREAD_ATTRIBUTE_JOB_LIST, UpdateProcThreadAttribute,
        };

        // SAFETY: `jobs` points to the stable single-element Job handle array.
        #[allow(unsafe_code)]
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                jobs.cast(),
                bytes,
                null_mut(),
                std::ptr::null(),
            )
        };
        if updated == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn update_handle_list(
        &self,
        list: *mut c_void,
        handles: *const RawWindowsHandle,
        bytes: usize,
    ) -> io::Result<()> {
        use std::ptr::null_mut;

        use windows_sys::Win32::System::Threading::{
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, UpdateProcThreadAttribute,
        };

        // SAFETY: `handles` points to the stable three-element child pipe array.
        #[allow(unsafe_code)]
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.cast(),
                bytes,
                null_mut(),
                std::ptr::null(),
            )
        };
        if updated == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn delete_attribute_list(&self, list: *mut c_void) {
        use windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList;

        // SAFETY: caller invokes this only after a successful initialization.
        #[allow(unsafe_code)]
        unsafe {
            DeleteProcThreadAttributeList(list)
        };
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::io;

    use super::{
        JobHandle, PipeEndHandle, RawWindowsHandle, WindowsStartupInfo, WindowsStartupInfoApi,
    };

    #[derive(Default)]
    struct FakeStartupApi {
        calls: RefCell<Vec<&'static str>>,
        job_bytes: RefCell<usize>,
        handle_bytes: RefCell<usize>,
    }

    impl WindowsStartupInfoApi for FakeStartupApi {
        fn attribute_list_size(&self, attribute_count: u32) -> io::Result<usize> {
            assert_eq!(attribute_count, 2);
            self.calls.borrow_mut().push("size");
            Ok(64)
        }

        fn initialize_attribute_list(
            &self,
            _list: *mut c_void,
            _attribute_count: u32,
            _bytes: usize,
        ) -> io::Result<()> {
            self.calls.borrow_mut().push("initialize");
            Ok(())
        }

        fn update_job_list(
            &self,
            _list: *mut c_void,
            _jobs: *const RawWindowsHandle,
            bytes: usize,
        ) -> io::Result<()> {
            self.calls.borrow_mut().push("job");
            *self.job_bytes.borrow_mut() = bytes;
            Ok(())
        }

        fn update_handle_list(
            &self,
            _list: *mut c_void,
            _handles: *const RawWindowsHandle,
            bytes: usize,
        ) -> io::Result<()> {
            self.calls.borrow_mut().push("handles");
            *self.handle_bytes.borrow_mut() = bytes;
            Ok(())
        }

        fn delete_attribute_list(&self, _list: *mut c_void) {
            self.calls.borrow_mut().push("delete");
        }
    }

    #[test]
    fn exactly_two_attributes_have_one_job_and_three_child_handles() {
        let api = FakeStartupApi::default();
        let job = JobHandle::new(1usize as RawWindowsHandle);
        let child = [
            PipeEndHandle::new(2usize as RawWindowsHandle),
            PipeEndHandle::new(3usize as RawWindowsHandle),
            PipeEndHandle::new(4usize as RawWindowsHandle),
        ];
        let mut info = WindowsStartupInfo::new(&api, &job, &child[0], &child[1], &child[2])
            .expect("attributes initialize");
        assert_eq!(
            *api.calls.borrow(),
            ["size", "initialize", "job", "handles"]
        );
        assert_eq!(*info.job_list, [1usize as RawWindowsHandle]);
        assert_eq!(
            *info.handle_list,
            [
                2usize as RawWindowsHandle,
                3usize as RawWindowsHandle,
                4usize as RawWindowsHandle,
            ]
        );
        assert_eq!(
            *api.job_bytes.borrow(),
            std::mem::size_of::<RawWindowsHandle>()
        );
        assert_eq!(
            *api.handle_bytes.borrow(),
            3 * std::mem::size_of::<RawWindowsHandle>()
        );
        info.delete_with(&api);
        assert_eq!(api.calls.borrow().last(), Some(&"delete"));
    }
}
