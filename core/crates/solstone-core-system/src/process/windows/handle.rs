// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Handle ownership types for the Windows Job/process primitive.

use std::ffi::c_void;
use std::io;

pub(super) type RawWindowsHandle = *mut c_void;

/// The common closer behind the semantic handle types below.
///
/// This intentionally has no public meaning of its own.  In particular, the
/// `JobHandle` wrapper makes its kill-on-last-close behavior visible at every
/// call site instead of treating it like an observer handle.
pub(super) struct RawOwnedHandle {
    raw: Option<RawWindowsHandle>,
}

impl RawOwnedHandle {
    pub(super) fn new(raw: RawWindowsHandle) -> Self {
        Self { raw: Some(raw) }
    }

    pub(super) fn raw(&self) -> RawWindowsHandle {
        self.raw.expect("owned Windows handle is present")
    }

    #[cfg(test)]
    pub(super) fn into_raw(mut self) -> RawWindowsHandle {
        self.raw.take().expect("owned Windows handle is present")
    }

    #[cfg(any(test, all(windows, feature = "test-hooks")))]
    pub(super) fn take_raw(&mut self) -> RawWindowsHandle {
        self.raw.take().expect("owned Windows handle is present")
    }

    pub(super) fn close(&mut self) -> io::Result<()> {
        let raw = self.raw.take().expect("owned Windows handle is present");
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;

            // SAFETY: `raw` is owned exactly once by this wrapper.
            #[allow(unsafe_code)]
            let closed = unsafe { CloseHandle(raw) };
            if closed == 0 {
                self.raw = Some(raw);
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(windows))]
        let _ = raw;
        Ok(())
    }
}

impl Drop for RawOwnedHandle {
    fn drop(&mut self) {
        if self.raw.is_some() {
            let _ = self.close();
        }
    }
}

macro_rules! semantic_handle {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        pub(super) struct $name(RawOwnedHandle);

        impl $name {
            pub(super) fn new(raw: RawWindowsHandle) -> Self {
                Self(RawOwnedHandle::new(raw))
            }

            pub(super) fn raw(&self) -> RawWindowsHandle {
                self.0.raw()
            }
        }
    };
}

semantic_handle!(
    JobHandle,
    "An owning Job handle; dropping the final one is the kill-on-close signal."
);
semantic_handle!(
    RootProcessHandle,
    "An owning handle to the launched root process."
);
semantic_handle!(
    PrimaryThreadHandle,
    "An owning handle to the launched primary thread."
);
semantic_handle!(
    PipeEndHandle,
    "An owning endpoint of an anonymous stdio pipe."
);

impl PrimaryThreadHandle {
    #[cfg(windows)]
    pub(super) fn close(&mut self) -> io::Result<()> {
        self.0.close()
    }
}

impl PipeEndHandle {
    pub(super) fn close(&mut self) -> io::Result<()> {
        self.0.close()
    }

    #[cfg(all(windows, feature = "test-hooks"))]
    pub(super) fn take_raw(&mut self) -> RawWindowsHandle {
        self.0.take_raw()
    }

    #[cfg(test)]
    pub(super) fn release_for_test(&mut self) {
        let _ = self.0.take_raw();
    }
}

#[cfg(test)]
mod tests {
    use super::{RawOwnedHandle, RawWindowsHandle};

    #[test]
    fn raw_handle_can_be_released_without_a_second_close() {
        let raw = 1usize as RawWindowsHandle;
        let handle = RawOwnedHandle::new(raw);
        assert_eq!(handle.into_raw(), raw);
    }
}
