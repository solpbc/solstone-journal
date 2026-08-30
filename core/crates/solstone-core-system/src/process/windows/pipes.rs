// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Anonymous stdio pipes and their inheritance boundary.

use std::io;

use super::handle::PipeEndHandle;

pub(super) trait WindowsPipeApi {
    fn create_inheritable_pipe(&self) -> io::Result<(PipeEndHandle, PipeEndHandle)>;
    fn clear_inherit(&self, end: &PipeEndHandle) -> io::Result<()>;
}

/// The parent and child views of the three stdio pipes.
pub(super) struct PipedStdio {
    pub(super) parent_stdin_write: PipeEndHandle,
    pub(super) parent_stdout_read: PipeEndHandle,
    pub(super) parent_stderr_read: PipeEndHandle,
    child_stdin_read: Option<PipeEndHandle>,
    child_stdout_write: Option<PipeEndHandle>,
    child_stderr_write: Option<PipeEndHandle>,
}

impl PipedStdio {
    pub(super) fn create(api: &impl WindowsPipeApi) -> io::Result<Self> {
        let (child_stdin_read, parent_stdin_write) = api.create_inheritable_pipe()?;
        let (parent_stdout_read, child_stdout_write) = api.create_inheritable_pipe()?;
        let (parent_stderr_read, child_stderr_write) = api.create_inheritable_pipe()?;

        // Only these parent-side endpoints are retained by the owner.  The
        // child-side endpoints remain inheritable until CreateProcessW.
        api.clear_inherit(&parent_stdin_write)?;
        api.clear_inherit(&parent_stdout_read)?;
        api.clear_inherit(&parent_stderr_read)?;

        // A future drain-thread design will use two bounded ~2s joins (~4s
        // total, matching process/common.rs's DRAIN_JOIN_TIMEOUT =
        // Duration::from_secs(2) x2), accepting tail-line loss as the bounded
        // cleanup cost. This task creates no drain threads; containment, Job
        // identity, and the hard-stop ladder do not depend on pipe EOF.
        Ok(Self {
            parent_stdin_write,
            parent_stdout_read,
            parent_stderr_read,
            child_stdin_read: Some(child_stdin_read),
            child_stdout_write: Some(child_stdout_write),
            child_stderr_write: Some(child_stderr_write),
        })
    }

    pub(super) fn child_stdin_read(&self) -> &PipeEndHandle {
        self.child_stdin_read
            .as_ref()
            .expect("child stdin endpoint is retained until launch completes")
    }

    pub(super) fn child_stdout_write(&self) -> &PipeEndHandle {
        self.child_stdout_write
            .as_ref()
            .expect("child stdout endpoint is retained until launch completes")
    }

    pub(super) fn child_stderr_write(&self) -> &PipeEndHandle {
        self.child_stderr_write
            .as_ref()
            .expect("child stderr endpoint is retained until launch completes")
    }

    pub(super) fn close_child_ends(&mut self, api: &impl WindowsPipeApi) -> io::Result<()> {
        for end in [
            &mut self.child_stdin_read,
            &mut self.child_stdout_write,
            &mut self.child_stderr_write,
        ] {
            if let Some(mut end) = end.take() {
                api.clear_inherit(&end)?;
                end.close()?;
            }
        }
        Ok(())
    }
}

pub(super) struct SystemWindowsPipeApi;

#[cfg(windows)]
impl WindowsPipeApi for SystemWindowsPipeApi {
    fn create_inheritable_pipe(&self) -> io::Result<(PipeEndHandle, PipeEndHandle)> {
        use std::mem::size_of;
        use std::ptr::null_mut;

        use windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT;
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
        use windows_sys::Win32::System::Pipes::CreatePipe;

        let mut read = null_mut();
        let mut write = null_mut();
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        // SAFETY: both out-pointers and the security attributes are valid for
        // this synchronous pipe-creation call.
        #[allow(unsafe_code)]
        let created =
            unsafe { CreatePipe(&raw mut read, &raw mut write, &raw const attributes, 0) };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        debug_assert_ne!(HANDLE_FLAG_INHERIT, 0);
        Ok((PipeEndHandle::new(read), PipeEndHandle::new(write)))
    }

    fn clear_inherit(&self, end: &PipeEndHandle) -> io::Result<()> {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

        // SAFETY: `end` owns a live pipe endpoint for this synchronous call.
        #[allow(unsafe_code)]
        let cleared = unsafe { SetHandleInformation(end.raw(), HANDLE_FLAG_INHERIT, 0) };
        if cleared == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;

    use super::{PipeEndHandle, PipedStdio, WindowsPipeApi};

    struct FakePipeApi {
        next: RefCell<usize>,
        cleared: RefCell<Vec<usize>>,
    }

    impl WindowsPipeApi for FakePipeApi {
        fn create_inheritable_pipe(&self) -> io::Result<(PipeEndHandle, PipeEndHandle)> {
            let mut next = self.next.borrow_mut();
            let read = *next;
            *next += 1;
            let write = *next;
            *next += 1;
            Ok((
                PipeEndHandle::new(read as *mut _),
                PipeEndHandle::new(write as *mut _),
            ))
        }

        fn clear_inherit(&self, end: &PipeEndHandle) -> io::Result<()> {
            self.cleared.borrow_mut().push(end.raw() as usize);
            Ok(())
        }
    }

    #[test]
    fn only_parent_ends_are_cleared_before_launch() {
        let api = FakePipeApi {
            next: RefCell::new(1),
            cleared: RefCell::new(Vec::new()),
        };
        let pipes = PipedStdio::create(&api).expect("pipes are created");
        assert_eq!(*api.cleared.borrow(), vec![2, 3, 5]);
        assert_eq!(pipes.child_stdin_read().raw() as usize, 1);
        assert_eq!(pipes.child_stdout_write().raw() as usize, 4);
        assert_eq!(pipes.child_stderr_write().raw() as usize, 6);
    }
}
