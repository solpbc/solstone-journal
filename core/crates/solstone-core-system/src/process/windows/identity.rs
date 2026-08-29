// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows PID-reuse-proof process identity sampling.

#[cfg(windows)]
use std::io;

use crate::process::{ExecutionState, InstanceVerdict, ProcessBirth, ProcessInstance};

/// The two DWORDs returned by Windows as a process creation FILETIME.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowsFileTime {
    pub(crate) high: u32,
    pub(crate) low: u32,
}

/// Result of opening the target process with the rights needed for identity sampling.
#[derive(Debug)]
pub(crate) enum WindowsOpenResult<H> {
    Opened(H),
    Exited,
    Unverifiable,
}

/// Result of reading a process's creation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowsProcessTimesResult {
    Creation(WindowsFileTime),
    Unverifiable,
    NotAttempted,
}

/// Result of a zero-timeout process wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowsWaitResult {
    Live,
    Signaled,
    Unverifiable,
    NotAttempted,
}

/// Raw Win32 calls needed for one process identity sample.
///
/// A successful `open_process` result owns its handle. Dropping that value
/// releases the handle on every return path after the open succeeds.
pub(crate) trait WindowsProcessApi {
    type Handle;

    fn open_process(&self, pid: u32) -> WindowsOpenResult<Self::Handle>;
    fn process_times(&self, handle: &Self::Handle) -> WindowsProcessTimesResult;
    fn wait_for_zero(&self, handle: &Self::Handle) -> WindowsWaitResult;
}

/// A complete raw observation before it is compared to a retained identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowsProcessProbe {
    Live(ProcessInstance),
    Exited,
    Unverifiable,
}

/// Combine the Windows FILETIME halves exactly as returned by `GetProcessTimes`.
pub(crate) fn filetime_value(filetime: WindowsFileTime) -> u64 {
    (u64::from(filetime.high) << 32) | u64::from(filetime.low)
}

#[cfg(windows)]
fn windows_filetime_from_raw(
    filetime: windows_sys::Win32::Foundation::FILETIME,
) -> WindowsFileTime {
    WindowsFileTime {
        high: filetime.dwHighDateTime,
        low: filetime.dwLowDateTime,
    }
}

/// Exercise the production raw-FILETIME conversion boundary from native receipts.
#[cfg(all(windows, feature = "test-hooks"))]
#[doc(hidden)]
pub fn windows_filetime_value_from_raw_for_test(high: u32, low: u32) -> u64 {
    filetime_value(windows_filetime_from_raw(
        windows_sys::Win32::Foundation::FILETIME {
            dwLowDateTime: low,
            dwHighDateTime: high,
        },
    ))
}

/// Convert one set of raw Windows API outcomes into a process probe.
pub(crate) fn map_windows_process_outcomes(
    pid: u32,
    open: WindowsOpenResult<()>,
    times: WindowsProcessTimesResult,
    wait: WindowsWaitResult,
) -> WindowsProcessProbe {
    match open {
        WindowsOpenResult::Exited => WindowsProcessProbe::Exited,
        WindowsOpenResult::Unverifiable => WindowsProcessProbe::Unverifiable,
        WindowsOpenResult::Opened(()) => match wait {
            WindowsWaitResult::Signaled => WindowsProcessProbe::Exited,
            WindowsWaitResult::Live => match times {
                WindowsProcessTimesResult::Creation(creation) => {
                    WindowsProcessProbe::Live(ProcessInstance {
                        pid,
                        birth: ProcessBirth::windows(filetime_value(creation)),
                    })
                }
                WindowsProcessTimesResult::Unverifiable
                | WindowsProcessTimesResult::NotAttempted => WindowsProcessProbe::Unverifiable,
            },
            WindowsWaitResult::Unverifiable | WindowsWaitResult::NotAttempted => {
                WindowsProcessProbe::Unverifiable
            }
        },
    }
}

/// Compare a retained Windows identity with a newly sampled probe.
pub(crate) fn verdict_from_windows_probe(
    expected: &ProcessInstance,
    probe: WindowsProcessProbe,
) -> InstanceVerdict {
    let Some(expected_filetime) = expected.birth.windows_filetime() else {
        return InstanceVerdict::Unverifiable;
    };
    match probe {
        WindowsProcessProbe::Exited => InstanceVerdict::NotSameOrExited,
        WindowsProcessProbe::Unverifiable => InstanceVerdict::Unverifiable,
        WindowsProcessProbe::Live(actual) => match actual.birth.windows_filetime() {
            Some(actual_filetime)
                if actual.pid == expected.pid && actual_filetime == expected_filetime =>
            {
                InstanceVerdict::SameLive {
                    execution: ExecutionState::Running,
                }
            }
            Some(_) => InstanceVerdict::NotSameOrExited,
            None => InstanceVerdict::Unverifiable,
        },
    }
}

/// Sample `pid` with an injectable raw Windows API adapter.
pub(crate) fn sample_windows_process_with(
    api: &impl WindowsProcessApi,
    pid: u32,
) -> WindowsProcessProbe {
    // PID 0 is not a process identity that `OpenProcess` can verify.
    if pid == 0 {
        return WindowsProcessProbe::Unverifiable;
    }
    match api.open_process(pid) {
        WindowsOpenResult::Opened(handle) => {
            let times = api.process_times(&handle);
            let wait = api.wait_for_zero(&handle);
            map_windows_process_outcomes(pid, WindowsOpenResult::Opened(()), times, wait)
        }
        WindowsOpenResult::Exited => map_windows_process_outcomes(
            pid,
            WindowsOpenResult::Exited,
            WindowsProcessTimesResult::NotAttempted,
            WindowsWaitResult::NotAttempted,
        ),
        WindowsOpenResult::Unverifiable => map_windows_process_outcomes(
            pid,
            WindowsOpenResult::Unverifiable,
            WindowsProcessTimesResult::NotAttempted,
            WindowsWaitResult::NotAttempted,
        ),
    }
}

#[cfg(windows)]
pub(crate) struct SystemWindowsProcessApi;

#[cfg(windows)]
pub(crate) struct OwnedWindowsHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedWindowsHandle {
    fn drop(&mut self) {
        #[allow(unsafe_code)]
        // SAFETY: this wrapper is constructed only from a non-null handle returned by OpenProcess,
        // and Drop consumes the wrapper exactly once.
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
impl WindowsProcessApi for SystemWindowsProcessApi {
    type Handle = OwnedWindowsHandle;

    fn open_process(&self, pid: u32) -> WindowsOpenResult<Self::Handle> {
        #[allow(unsafe_code)]
        // SAFETY: OpenProcess reads only the scalar access mask, inherit flag, and PID supplied here.
        let handle = unsafe {
            windows_sys::Win32::System::Threading::OpenProcess(
                windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION
                    | windows_sys::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if handle == std::ptr::null_mut() {
            if io::Error::last_os_error().raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER as i32)
            {
                WindowsOpenResult::Exited
            } else {
                WindowsOpenResult::Unverifiable
            }
        } else {
            WindowsOpenResult::Opened(OwnedWindowsHandle(handle))
        }
    }

    fn process_times(&self, handle: &Self::Handle) -> WindowsProcessTimesResult {
        let mut creation = windows_sys::Win32::Foundation::FILETIME::default();
        let mut exit = windows_sys::Win32::Foundation::FILETIME::default();
        let mut kernel = windows_sys::Win32::Foundation::FILETIME::default();
        let mut user = windows_sys::Win32::Foundation::FILETIME::default();
        #[allow(unsafe_code)]
        // SAFETY: the owned process handle is valid while borrowed, and all FILETIME output pointers
        // point to initialized local storage that remains valid for this call.
        let succeeded = unsafe {
            windows_sys::Win32::System::Threading::GetProcessTimes(
                handle.0,
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        };
        if succeeded == 0 {
            WindowsProcessTimesResult::Unverifiable
        } else {
            WindowsProcessTimesResult::Creation(windows_filetime_from_raw(creation))
        }
    }

    fn wait_for_zero(&self, handle: &Self::Handle) -> WindowsWaitResult {
        #[allow(unsafe_code)]
        // SAFETY: the owned process handle is valid while borrowed, and a zero timeout does not retain
        // any borrowed memory after WaitForSingleObject returns.
        let outcome =
            unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(handle.0, 0) };
        match outcome {
            windows_sys::Win32::Foundation::WAIT_TIMEOUT => WindowsWaitResult::Live,
            windows_sys::Win32::Foundation::WAIT_OBJECT_0 => WindowsWaitResult::Signaled,
            _ => WindowsWaitResult::Unverifiable,
        }
    }
}

#[cfg(windows)]
pub(crate) fn current_windows_process_instance() -> Result<ProcessInstance, io::Error> {
    match sample_windows_process_with(&SystemWindowsProcessApi, std::process::id()) {
        WindowsProcessProbe::Live(instance) => Ok(instance),
        WindowsProcessProbe::Exited => Err(io::Error::other("current Windows process is signaled")),
        WindowsProcessProbe::Unverifiable => Err(io::Error::other(
            "current Windows process creation FILETIME is unavailable",
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    struct FakeHandle<'a>(&'a Cell<u32>);

    impl Drop for FakeHandle<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    struct FakeApi<'a> {
        opens: Cell<u32>,
        drops: &'a Cell<u32>,
        open: Cell<Option<WindowsOpenResult<()>>>,
        times: WindowsProcessTimesResult,
        wait: WindowsWaitResult,
    }

    impl<'a> WindowsProcessApi for FakeApi<'a> {
        type Handle = FakeHandle<'a>;

        fn open_process(&self, _pid: u32) -> WindowsOpenResult<Self::Handle> {
            self.opens.set(self.opens.get() + 1);
            match self.open.take().expect("one fake open result") {
                WindowsOpenResult::Opened(()) => WindowsOpenResult::Opened(FakeHandle(self.drops)),
                WindowsOpenResult::Exited => WindowsOpenResult::Exited,
                WindowsOpenResult::Unverifiable => WindowsOpenResult::Unverifiable,
            }
        }

        fn process_times(&self, _handle: &Self::Handle) -> WindowsProcessTimesResult {
            self.times
        }

        fn wait_for_zero(&self, _handle: &Self::Handle) -> WindowsWaitResult {
            self.wait
        }
    }

    fn fake<'a>(
        drops: &'a Cell<u32>,
        open: WindowsOpenResult<()>,
        times: WindowsProcessTimesResult,
        wait: WindowsWaitResult,
    ) -> FakeApi<'a> {
        FakeApi {
            opens: Cell::new(0),
            drops,
            open: Cell::new(Some(open)),
            times,
            wait,
        }
    }

    #[test]
    fn combines_filetime_halves_used_by_the_sampler() {
        assert_eq!(
            filetime_value(WindowsFileTime {
                high: 0x0123_4567,
                low: 0x89ab_cdef,
            }),
            0x0123_4567_89ab_cdef
        );
    }

    #[test]
    fn zero_pid_does_not_open_a_process() {
        let drops = Cell::new(0);
        let api = fake(
            &drops,
            WindowsOpenResult::Opened(()),
            WindowsProcessTimesResult::Creation(WindowsFileTime { high: 0, low: 1 }),
            WindowsWaitResult::Live,
        );
        assert_eq!(
            sample_windows_process_with(&api, 0),
            WindowsProcessProbe::Unverifiable
        );
        assert_eq!(api.opens.get(), 0);
        assert_eq!(drops.get(), 0);
    }

    #[test]
    fn maps_open_times_and_wait_outcomes_and_closes_opened_handle() {
        let drops = Cell::new(0);
        let api = fake(
            &drops,
            WindowsOpenResult::Opened(()),
            WindowsProcessTimesResult::Creation(WindowsFileTime { high: 1, low: 2 }),
            WindowsWaitResult::Live,
        );
        assert_eq!(
            sample_windows_process_with(&api, 42),
            WindowsProcessProbe::Live(ProcessInstance {
                pid: 42,
                birth: ProcessBirth::windows((1_u64 << 32) | 2),
            })
        );
        assert_eq!(drops.get(), 1);

        let api = fake(
            &drops,
            WindowsOpenResult::Opened(()),
            WindowsProcessTimesResult::Unverifiable,
            WindowsWaitResult::Live,
        );
        assert_eq!(
            sample_windows_process_with(&api, 42),
            WindowsProcessProbe::Unverifiable
        );
        assert_eq!(drops.get(), 2);

        let api = fake(
            &drops,
            WindowsOpenResult::Opened(()),
            WindowsProcessTimesResult::Creation(WindowsFileTime { high: 0, low: 1 }),
            WindowsWaitResult::Signaled,
        );
        assert_eq!(
            sample_windows_process_with(&api, 42),
            WindowsProcessProbe::Exited
        );
        assert_eq!(drops.get(), 3);
    }

    #[test]
    fn verdict_preserves_unverifiable_births() {
        let expected = ProcessInstance {
            pid: 7,
            birth: ProcessBirth::windows(10),
        };
        assert_eq!(
            verdict_from_windows_probe(
                &expected,
                WindowsProcessProbe::Live(ProcessInstance {
                    pid: 7,
                    birth: ProcessBirth::windows(10),
                })
            ),
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            }
        );
        assert_eq!(
            verdict_from_windows_probe(
                &expected,
                WindowsProcessProbe::Live(ProcessInstance {
                    pid: 7,
                    birth: ProcessBirth::windows(11),
                })
            ),
            InstanceVerdict::NotSameOrExited
        );
        assert_eq!(
            verdict_from_windows_probe(
                &ProcessInstance {
                    pid: 7,
                    birth: ProcessBirth::linux(1, 1, 1),
                },
                WindowsProcessProbe::Live(expected),
            ),
            InstanceVerdict::Unverifiable
        );
    }
}
