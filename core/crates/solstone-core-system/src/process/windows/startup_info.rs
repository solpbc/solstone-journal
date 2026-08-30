// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Extended startup information with Job and inheritable-handle attributes.

use std::ffi::c_void;
use std::io;

use super::handle::{JobHandle, PipeEndHandle, RawWindowsHandle};

#[repr(align(16))]
struct AttributeStorage {
    _bytes: [u8; 16],
}

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

/// Stable backing storage for the process-thread attribute values.
pub(super) struct WindowsStartupInfo {
    _storage: Vec<AttributeStorage>,
    list: *mut c_void,
    initialized: bool,
    job_list: Box<[RawWindowsHandle; 1]>,
    handle_list: Option<Box<[RawWindowsHandle; 3]>>,
}

impl WindowsStartupInfo {
    pub(super) fn new(
        api: &impl WindowsStartupInfoApi,
        job: &JobHandle,
        child_stdin: &PipeEndHandle,
        child_stdout: &PipeEndHandle,
        child_stderr: &PipeEndHandle,
    ) -> io::Result<Self> {
        Self::new_with_handle_list(
            api,
            job,
            Some([child_stdin.raw(), child_stdout.raw(), child_stderr.raw()]),
        )
    }

    pub(super) fn new_job_only(
        api: &impl WindowsStartupInfoApi,
        job: &JobHandle,
    ) -> io::Result<Self> {
        Self::new_with_handle_list(api, job, None)
    }

    fn new_with_handle_list(
        api: &impl WindowsStartupInfoApi,
        job: &JobHandle,
        handles: Option<[RawWindowsHandle; 3]>,
    ) -> io::Result<Self> {
        let attribute_count = if handles.is_some() { 2 } else { 1 };

        let required = api.attribute_list_size(attribute_count)?;
        let slot_count = required.div_ceil(std::mem::size_of::<AttributeStorage>());
        let mut storage = Vec::with_capacity(slot_count);
        storage.resize_with(slot_count, || AttributeStorage { _bytes: [0; 16] });
        let list = storage.as_mut_ptr().cast();
        let job_list = Box::new([job.raw()]);
        // The Job is deliberately absent from this list: it belongs only to
        // PROC_THREAD_ATTRIBUTE_JOB_LIST, never to the inheritable handles.
        let handle_list = handles.map(Box::new);

        api.initialize_attribute_list(list, attribute_count, required)?;
        let mut startup = Self {
            _storage: storage,
            list,
            initialized: true,
            job_list,
            handle_list,
        };
        if let Err(error) = api.update_job_list(
            startup.list,
            startup.job_list.as_ptr(),
            std::mem::size_of_val(startup.job_list.as_ref()),
        ) {
            startup.delete_with(api);
            return Err(error);
        }
        if let Some(handle_list) = startup.handle_list.as_ref()
            && let Err(error) = api.update_handle_list(
                startup.list,
                handle_list.as_ptr(),
                std::mem::size_of_val(handle_list.as_ref()),
            )
        {
            startup.delete_with(api);
            return Err(error);
        }
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
        stdio: Option<[RawWindowsHandle; 3]>,
    ) -> windows_sys::Win32::System::Threading::STARTUPINFOEXW {
        use std::mem::size_of;

        use windows_sys::Win32::System::Threading::{STARTF_USESTDHANDLES, STARTUPINFOEXW};

        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        assert_eq!(
            self.handle_list.is_some(),
            stdio.is_some(),
            "startup stdio must match the initialized handle-list mode"
        );
        if let Some([child_stdin, child_stdout, child_stderr]) = stdio {
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = child_stdin;
            startup.StartupInfo.hStdOutput = child_stdout;
            startup.StartupInfo.hStdError = child_stderr;
        }
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

#[cfg(windows)]
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
        let error = io::Error::last_os_error();
        validate_attribute_list_size(
            initialized != 0,
            error.raw_os_error() == Some(ERROR_INSUFFICIENT_BUFFER as i32),
            bytes,
            error,
        )
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

fn validate_attribute_list_size(
    succeeded: bool,
    error_is_insufficient_buffer: bool,
    bytes: usize,
    error: io::Error,
) -> io::Result<usize> {
    if succeeded {
        return Err(io::Error::other(
            "attribute-list sizing call unexpectedly succeeded",
        ));
    }
    if !error_is_insufficient_buffer {
        return Err(error);
    }
    if bytes == 0 {
        return Err(io::Error::other(
            "attribute-list sizing call returned a zero size",
        ));
    }
    Ok(bytes)
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
        counts: RefCell<Vec<u32>>,
        job_bytes: RefCell<usize>,
        handle_bytes: RefCell<usize>,
    }

    impl WindowsStartupInfoApi for FakeStartupApi {
        fn attribute_list_size(&self, attribute_count: u32) -> io::Result<usize> {
            self.counts.borrow_mut().push(attribute_count);
            self.calls.borrow_mut().push("size");
            Ok(64)
        }

        fn initialize_attribute_list(
            &self,
            _list: *mut c_void,
            attribute_count: u32,
            _bytes: usize,
        ) -> io::Result<()> {
            self.counts.borrow_mut().push(attribute_count);
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
        assert_eq!(*api.counts.borrow(), [2, 2]);
        assert_eq!(*info.job_list, [1usize as RawWindowsHandle]);
        assert_eq!(
            **info.handle_list.as_ref().expect("handle list"),
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

    #[test]
    fn job_only_mode_initializes_one_attribute_without_a_handle_list() {
        let api = FakeStartupApi::default();
        let job = JobHandle::new(1usize as RawWindowsHandle);
        let mut info = WindowsStartupInfo::new_job_only(&api, &job).expect("attributes initialize");

        assert_eq!(*api.calls.borrow(), ["size", "initialize", "job"]);
        assert_eq!(*api.counts.borrow(), [1, 1]);
        assert_eq!(*info.job_list, [1usize as RawWindowsHandle]);
        assert!(info.handle_list.is_none());
        assert_eq!(*api.handle_bytes.borrow(), 0);
        info.delete_with(&api);
        assert_eq!(api.calls.borrow().last(), Some(&"delete"));
    }

    #[test]
    fn sizing_requires_false_insufficient_buffer_and_nonzero_bytes() {
        assert_eq!(
            super::validate_attribute_list_size(
                false,
                true,
                64,
                io::Error::from_raw_os_error(122),
            )
            .expect("documented sizing result"),
            64
        );
        assert_eq!(
            super::validate_attribute_list_size(true, true, 64, io::Error::from_raw_os_error(122),)
                .expect_err("TRUE is invalid")
                .to_string(),
            "attribute-list sizing call unexpectedly succeeded"
        );
        assert_eq!(
            super::validate_attribute_list_size(false, false, 64, io::Error::from_raw_os_error(5),)
                .expect_err("wrong error is invalid")
                .raw_os_error(),
            Some(5)
        );
        assert_eq!(
            super::validate_attribute_list_size(false, true, 0, io::Error::from_raw_os_error(122),)
                .expect_err("zero bytes is invalid")
                .to_string(),
            "attribute-list sizing call returned a zero size"
        );
    }
}
