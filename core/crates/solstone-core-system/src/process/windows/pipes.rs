// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Anonymous stdio pipes and their inheritance boundary.

use std::io;

use super::handle::PipeEndHandle;

pub(super) trait WindowsPipeApi {
    fn create_pipe(&self) -> io::Result<(PipeEndHandle, PipeEndHandle)>;
    fn set_inherit(&self, end: &PipeEndHandle) -> io::Result<()>;
    fn clear_inherit(&self, end: &PipeEndHandle) -> io::Result<()>;
    fn close_end(&self, end: &mut PipeEndHandle) -> io::Result<()> {
        end.close()
    }
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
        // Every endpoint starts non-inheritable. The three child endpoints are
        // made inheritable only in the final, bounded pre-CreateProcessW window.
        let (child_stdin_read, parent_stdin_write) = api.create_pipe()?;
        let (parent_stdout_read, child_stdout_write) = api.create_pipe()?;
        let (parent_stderr_read, child_stderr_write) = api.create_pipe()?;

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

    pub(super) fn make_child_ends_inheritable(
        &mut self,
        api: &impl WindowsPipeApi,
    ) -> io::Result<()> {
        if let Err(error) = api.set_inherit(self.child_stdin_read()) {
            self.cleanup_after_set_failure(api, error)
        } else if let Err(error) = api.set_inherit(self.child_stdout_write()) {
            self.cleanup_after_set_failure(api, error)
        } else if let Err(error) = api.set_inherit(self.child_stderr_write()) {
            self.cleanup_after_set_failure(api, error)
        } else {
            Ok(())
        }
    }

    fn cleanup_after_set_failure(
        &mut self,
        api: &impl WindowsPipeApi,
        error: io::Error,
    ) -> io::Result<()> {
        // Preserve the set failure that made launch impossible while still
        // attempting every clear and close.
        let _ = self.close_child_ends(api);
        Err(error)
    }

    pub(super) fn close_child_ends(&mut self, api: &impl WindowsPipeApi) -> io::Result<()> {
        let mut first_error = None;
        for end in [
            &mut self.child_stdin_read,
            &mut self.child_stdout_write,
            &mut self.child_stderr_write,
        ] {
            if let Some(mut end) = end.take() {
                preserve_first_error(&mut first_error, api.clear_inherit(&end));
                preserve_first_error(&mut first_error, api.close_end(&mut end));
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[cfg(windows)]
    pub(super) fn close_parent_stdin(&mut self, api: &impl WindowsPipeApi) -> io::Result<()> {
        api.close_end(&mut self.parent_stdin_write)
    }
}

fn preserve_first_error(first_error: &mut Option<io::Error>, result: io::Result<()>) {
    if let Err(error) = result
        && first_error.is_none()
    {
        *first_error = Some(error);
    }
}

#[cfg(windows)]
pub(super) struct SystemWindowsPipeApi;

#[cfg(windows)]
impl WindowsPipeApi for SystemWindowsPipeApi {
    fn create_pipe(&self) -> io::Result<(PipeEndHandle, PipeEndHandle)> {
        use std::ptr::null_mut;

        use windows_sys::Win32::System::Pipes::CreatePipe;

        let mut read = null_mut();
        let mut write = null_mut();
        // SAFETY: both out-pointers are valid for this synchronous call. A null
        // SECURITY_ATTRIBUTES pointer creates both endpoints non-inheritable.
        #[allow(unsafe_code)]
        let created = unsafe { CreatePipe(&raw mut read, &raw mut write, null_mut(), 0) };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((PipeEndHandle::new(read), PipeEndHandle::new(write)))
    }

    fn set_inherit(&self, end: &PipeEndHandle) -> io::Result<()> {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

        // SAFETY: `end` owns a live pipe endpoint for this synchronous call.
        #[allow(unsafe_code)]
        let set =
            unsafe { SetHandleInformation(end.raw(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
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
    use std::collections::BTreeMap;
    use std::io;

    use super::{PipeEndHandle, PipedStdio, WindowsPipeApi};

    struct FakePipeApi {
        next: RefCell<usize>,
        inherited: RefCell<BTreeMap<usize, bool>>,
        set: RefCell<Vec<usize>>,
        cleared: RefCell<Vec<usize>>,
        closed: RefCell<Vec<usize>>,
        fail_set: Option<usize>,
        fail_clear: Option<usize>,
        fail_close: Option<usize>,
    }

    impl WindowsPipeApi for FakePipeApi {
        fn create_pipe(&self) -> io::Result<(PipeEndHandle, PipeEndHandle)> {
            let mut next = self.next.borrow_mut();
            let read = *next;
            *next += 1;
            let write = *next;
            *next += 1;
            self.inherited.borrow_mut().insert(read, false);
            self.inherited.borrow_mut().insert(write, false);
            Ok((
                PipeEndHandle::new(read as *mut _),
                PipeEndHandle::new(write as *mut _),
            ))
        }

        fn set_inherit(&self, end: &PipeEndHandle) -> io::Result<()> {
            let raw = end.raw() as usize;
            self.set.borrow_mut().push(raw);
            if self.fail_set == Some(raw) {
                return Err(io::Error::other(format!("set inherit {raw}")));
            }
            *self
                .inherited
                .borrow_mut()
                .get_mut(&raw)
                .expect("created pipe endpoint") = true;
            Ok(())
        }

        fn clear_inherit(&self, end: &PipeEndHandle) -> io::Result<()> {
            let raw = end.raw() as usize;
            self.cleared.borrow_mut().push(raw);
            if self.fail_clear == Some(raw) {
                return Err(io::Error::other(format!("clear inherit {raw}")));
            }
            *self
                .inherited
                .borrow_mut()
                .get_mut(&raw)
                .expect("created pipe endpoint") = false;
            Ok(())
        }

        fn close_end(&self, end: &mut PipeEndHandle) -> io::Result<()> {
            let raw = end.raw() as usize;
            self.closed.borrow_mut().push(raw);
            if self.fail_close == Some(raw) {
                return Err(io::Error::other(format!("close {raw}")));
            }
            end.release_for_test();
            Ok(())
        }
    }

    fn fake_api() -> FakePipeApi {
        FakePipeApi {
            next: RefCell::new(1),
            inherited: RefCell::new(BTreeMap::new()),
            set: RefCell::new(Vec::new()),
            cleared: RefCell::new(Vec::new()),
            closed: RefCell::new(Vec::new()),
            fail_set: None,
            fail_clear: None,
            fail_close: None,
        }
    }

    #[test]
    fn all_ends_start_noninheritable_and_only_child_bits_are_set_then_cleared() {
        let api = fake_api();
        let mut pipes = PipedStdio::create(&api).expect("pipes are created");
        assert_eq!(
            *api.inherited.borrow(),
            BTreeMap::from([
                (1, false),
                (2, false),
                (3, false),
                (4, false),
                (5, false),
                (6, false)
            ])
        );
        assert_eq!(pipes.child_stdin_read().raw() as usize, 1);
        assert_eq!(pipes.child_stdout_write().raw() as usize, 4);
        assert_eq!(pipes.child_stderr_write().raw() as usize, 6);
        assert_eq!(pipes.parent_stdin_write.raw() as usize, 2);
        assert_eq!(pipes.parent_stdout_read.raw() as usize, 3);
        assert_eq!(pipes.parent_stderr_read.raw() as usize, 5);

        pipes
            .make_child_ends_inheritable(&api)
            .expect("child bits are set");
        assert_eq!(*api.set.borrow(), [1, 4, 6]);
        assert_eq!(
            *api.inherited.borrow(),
            BTreeMap::from([
                (1, true),
                (2, false),
                (3, false),
                (4, true),
                (5, false),
                (6, true)
            ])
        );

        pipes.close_child_ends(&api).expect("children close");
        assert_eq!(*api.cleared.borrow(), [1, 4, 6]);
        assert_eq!(*api.closed.borrow(), [1, 4, 6]);
        assert!(api.inherited.borrow().values().all(|inherited| !inherited));
    }

    #[test]
    fn partial_set_failure_attempts_every_clear_and_close_and_preserves_set_error() {
        let mut api = fake_api();
        api.fail_set = Some(4);
        api.fail_clear = Some(1);
        api.fail_close = Some(4);
        let mut pipes = PipedStdio::create(&api).expect("pipes are created");

        let error = pipes
            .make_child_ends_inheritable(&api)
            .expect_err("second set fails");
        assert_eq!(error.to_string(), "set inherit 4");
        assert_eq!(*api.set.borrow(), [1, 4]);
        assert_eq!(*api.cleared.borrow(), [1, 4, 6]);
        assert_eq!(*api.closed.borrow(), [1, 4, 6]);
    }

    #[test]
    fn child_cleanup_attempts_every_operation_and_returns_its_first_error() {
        let mut api = fake_api();
        api.fail_clear = Some(1);
        api.fail_close = Some(4);
        let mut pipes = PipedStdio::create(&api).expect("pipes are created");

        let error = pipes
            .close_child_ends(&api)
            .expect_err("cleanup reports its first error");
        assert_eq!(error.to_string(), "clear inherit 1");
        assert_eq!(*api.cleared.borrow(), [1, 4, 6]);
        assert_eq!(*api.closed.borrow(), [1, 4, 6]);
    }
}
