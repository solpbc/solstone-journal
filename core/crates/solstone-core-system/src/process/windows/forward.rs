// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The journal CLI's Windows process-replacement boundary.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::time::{Duration, Instant};

use super::bounded::BoundedHelperResources;
use super::bounded_cleanup::Reservation;
use super::job_process::{JOB_HARD_STOP_TIMEOUT, launch_windows_forwarder};
use super::launch_control::{
    AdmittedWindowsLaunch, InstalledTaskLaunchRequest, LaunchControl, signal_stop,
};
use crate::process::{SERVICE_SHUTDOWN_TIMEOUT, SESSION_END_DRAIN_TIMEOUT};

/// Session-end detection for an ordinary (non-service) forwarder process.
///
/// `SetConsoleCtrlHandler` alone is not sufficient: `CTRL_LOGOFF_EVENT` is
/// documented to reach services only ("interactive applications are
/// terminated at logoff, so they are not present when the system sends this
/// signal" -- MS HandlerRoutine docs), and measured behavior on this product
/// shows `CTRL_SHUTDOWN_EVENT` does not reliably reach a Job-hosted console
/// child either: the forwarder and its children run under hidden or
/// terminal-hosted consoles that never receive an end-session console event,
/// so the tree was killed outright with no chance to clear readiness/identity
/// markers. The reliable, documented mechanism for an ordinary interactive
/// process is a top-level window's `WM_QUERYENDSESSION`/`WM_ENDSESSION`
/// messages, delivered by the session broadcast regardless of console
/// attachment. This module runs both: the console handler as a cheap
/// best-effort path, and a dedicated hidden window + message pump as the
/// mechanism that actually closes the logoff/shutdown gap.
mod session_end {
    use std::io;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use crate::process::SESSION_END_DRAIN_TIMEOUT;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Console::{
        CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, GetConsoleWindow,
        SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassExW,
        SW_HIDE, ShowWindow, TranslateMessage, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSEXW,
        WS_OVERLAPPED,
    };

    static REQUESTED: AtomicBool = AtomicBool::new(false);
    /// Set by the forwarding loop once its tree is down (or its session-end
    /// deadline has elapsed). `WM_ENDSESSION` waits on this.
    static DRAINED: AtomicBool = AtomicBool::new(false);
    static INSTALLED: OnceLock<io::Result<()>> = OnceLock::new();

    fn window_class_name() -> &'static [u16] {
        static NAME: OnceLock<Vec<u16>> = OnceLock::new();
        NAME.get_or_init(|| {
            "SolstoneJournalSessionEnd"
                .encode_utf16()
                .chain([0])
                .collect()
        })
    }

    #[allow(unsafe_code)]
    unsafe extern "system" fn handler(control_type: u32) -> windows_sys::core::BOOL {
        match control_type {
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
                REQUESTED.store(true, Ordering::SeqCst);
                // Returning hands the process to the system for termination.
                // The forwarding loop finishes the tree and exits the process.
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            _ => 0,
        }
    }

    /// `WM_QUERYENDSESSION` is the query every top-level window receives
    /// first, whether the window is visible or not; latching here starts the
    /// forwarding loop's drain immediately. Returning nonzero allows the
    /// session to end -- this process does not veto logoff/shutdown.
    ///
    /// 🔴 Latching alone is not enough, and leg six measured why: answering
    /// the query at once lets Windows proceed as soon as every *other* window
    /// has answered too, so the drain gets whatever time the rest of the
    /// session happens to take (about seven seconds on this rig, and no
    /// promise at all on a quiet one) and the tree was killed with the
    /// lifecycle markers still on disk. `WM_ENDSESSION` is the message an
    /// application is documented to finish its cleanup inside, and Windows
    /// waits `WaitToKillAppTimeout` for the reply, so the reply is held here
    /// until the forwarding loop says the tree is down. This is a ceiling,
    /// never a veto: the OS terminates this process when its own timer
    /// expires whatever we do, and the drain is bounded well below it.
    #[allow(unsafe_code)]
    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_QUERYENDSESSION => {
                REQUESTED.store(true, Ordering::SeqCst);
                1
            }
            WM_ENDSESSION => {
                // wparam is FALSE when some other application vetoed the
                // session end; there is nothing to wait for in that case.
                if wparam != 0 {
                    REQUESTED.store(true, Ordering::SeqCst);
                    let deadline = Instant::now() + SESSION_END_DRAIN_TIMEOUT;
                    while !DRAINED.load(Ordering::SeqCst) && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
                0
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    /// Report that the forwarded tree is down, releasing `WM_ENDSESSION`.
    ///
    /// Only a session-end drain counts. The installed forwarder restarts a
    /// crashed supervisor in a loop, and every one of those `forward` returns
    /// calls this; without the guard the first crash restart would latch
    /// DRAINED and a later `WM_ENDSESSION` would wait for nothing.
    pub(super) fn drained() {
        if requested() {
            DRAINED.store(true, Ordering::SeqCst);
        }
    }

    /// Create a hidden top-level window and pump its message loop for the
    /// life of the process. Never returns; runs on its own thread. A failure
    /// to register the class or create the window leaves this process with
    /// only the console-handler path (best effort, logged by the caller).
    #[allow(unsafe_code)]
    fn pump_session_end_window() {
        let class_name = window_class_name().as_ptr();
        #[allow(unsafe_code)]
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class_name,
            ..Default::default()
        };
        #[allow(unsafe_code)]
        if unsafe { RegisterClassExW(&class) } == 0 {
            return;
        }
        #[allow(unsafe_code)]
        let window = unsafe {
            CreateWindowExW(
                0,
                class_name,
                class_name,
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if window.is_null() {
            return;
        }
        let mut message = MSG::default();
        loop {
            #[allow(unsafe_code)]
            let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
            if result <= 0 {
                return;
            }
            #[allow(unsafe_code)]
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    /// Install once per process; a later failure is reported, never retried.
    pub(super) fn install() -> io::Result<()> {
        INSTALLED
            .get_or_init(|| {
                // SAFETY: `handler` is a valid `HandlerRoutine` for the life of
                // the process and is never removed.
                #[allow(unsafe_code)]
                if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                install_window()?;
                // Best effort: hides the console window Task Scheduler
                // otherwise leaves visible at every logon. Session-end
                // detection above does not depend on the console at all, so
                // a failure here is cosmetic, not functional.
                #[allow(unsafe_code)]
                unsafe {
                    let console = GetConsoleWindow();
                    if !console.is_null() {
                        ShowWindow(console, SW_HIDE);
                    }
                }
                Ok(())
            })
            .as_ref()
            .map(|()| ())
            .map_err(|error| io::Error::new(error.kind(), error.to_string()))
    }

    /// Start the hidden session-end window alone, once per process, with no
    /// console handler.
    pub(super) fn install_window() -> io::Result<()> {
        static WINDOW: OnceLock<io::Result<()>> = OnceLock::new();
        WINDOW
            .get_or_init(|| {
                std::thread::Builder::new()
                    .name("session-end-window".to_owned())
                    .spawn(pump_session_end_window)
                    .map(|_| ())
                    .map_err(|error| io::Error::other(error.to_string()))
            })
            .as_ref()
            .map(|()| ())
            .map_err(|error| io::Error::new(error.kind(), error.to_string()))
    }

    pub(super) fn requested() -> bool {
        REQUESTED.load(Ordering::SeqCst)
    }
}

/// Make this process one Windows asks before a sign-out or shutdown ends it.
///
/// 🔴 Windows terminates a console process that owns no window the moment a
/// sign-out begins, before any end-session message is sent: measured on a
/// Windows 11 guest, a windowless console child stopped about 40 ms before
/// its windowed sibling received `WM_QUERYENDSESSION`, and the sibling then
/// held `WM_ENDSESSION` for three seconds and was not killed. The supervisor
/// owns the lifecycle markers and has no window, so it was killed with them
/// still on disk while the forwarder's window -- which is asked -- had nothing
/// left to drain. A process that must clean up at session end owns this
/// hidden window; it holds `WM_ENDSESSION` for at most
/// `SESSION_END_DRAIN_TIMEOUT`, or until the process exits.
pub fn watch_windows_session_end() -> io::Result<()> {
    session_end::install_window()
}

/// Whether Windows has asked this process to end its session.
pub fn windows_session_end_requested() -> bool {
    session_end::requested()
}

/// Run a native sibling with the current standard handles and retain its Job
/// until the entire owned tree is quiescent. An admitted CLI hop reissues its
/// exact provenance, grants and latched stop through a fresh launch transaction.
/// The returned status is the child's status; admission/cleanup failure is Err.
pub fn forward_windows_native_command(
    program: &OsStr,
    arguments: &[OsString],
    admitted: Option<&AdmittedWindowsLaunch>,
) -> io::Result<i32> {
    let mut environment = BTreeMap::new();
    let control = admitted
        .map(|incoming| {
            let mut nonce = [0_u8; 24];
            getrandom::fill(&mut nonce).map_err(|error| io::Error::other(error.to_string()))?;
            let launch_id = nonce
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let mut provenance = incoming.child_launch_provenance(launch_id);
            // Forward the same service command through a new process identity.
            // Ordinary children use child_launch_provenance's service=None instead.
            provenance.service = incoming.service();
            LaunchControl::prepare(&provenance, &mut environment)
        })
        .transpose()?;
    forward(program, arguments, admitted, control, environment)
}

/// How many times the installed forwarder relaunches a supervisor that died
/// abnormally before giving up, and how long it waits between attempts.
///
/// 🔴 This exists because the Scheduler's `RestartOnFailure` is not a crash
/// restart. The installed task registers `Interval=PT1M Count=10`, and it was
/// measured on a Windows 11 test host not to fire at all when the action is
/// terminated: `LastTaskResult` reads `-1`, the task settles to `Ready` with
/// `Next Run Time: N/A`, and nothing restarts in five minutes. Measured twice
/// more on purpose-built probe tasks that copy the installed task's settings
/// exactly, once with `UseUnifiedSchedulingEngine` true and once false --
/// neither restarted, so the engine choice was not the cause and the policy
/// simply does not cover an action that exits non-zero. The resident that
/// holds an owner's capture has to supervise itself.
///
/// The counter resets once a run has stayed up for `RESTART_CREDIT_UPTIME`,
/// so a journal that crashes once a week is always restarted while a journal
/// that cannot start at all stops after a bounded number of attempts.
const INSTALLED_TASK_RESTART_LIMIT: u32 = 10;
const INSTALLED_TASK_RESTART_DELAY: Duration = Duration::from_secs(5);
const RESTART_CREDIT_UPTIME: Duration = Duration::from_secs(300);

/// Forward the exact installed action with no hosted generation grants,
/// restarting it after an abnormal exit.
pub fn forward_windows_installed_task(
    program: &OsStr,
    request: &InstalledTaskLaunchRequest,
) -> io::Result<i32> {
    let arguments = request
        .arguments
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    run_with_restarts(
        || {
            let mut environment = BTreeMap::new();
            let control = LaunchControl::prepare_installed(request, &mut environment)?;
            let started = Instant::now();
            let code = forward(program, &arguments, None, Some(control), environment)?;
            Ok((code, started.elapsed()))
        },
        session_end::requested,
        std::thread::sleep,
    )
}

/// The installed task's restart policy, apart from the process work so it can be
/// tested: a clean exit or a session end returns; anything else is retried up to
/// [`INSTALLED_TASK_RESTART_LIMIT`] times, [`INSTALLED_TASK_RESTART_DELAY`] apart,
/// with a long-lived run resetting the count.
///
/// 🔴 A failed launch is an attempt too, not a terminal error. On the first logon
/// after a boot the forwarder took 57 s to create its child (cold image, scanner
/// still warming), the launch deadline had passed before the child existed, and
/// the error left this loop at once: the task ended 70 and the owner had no
/// journal until they started it by hand. Measured on the Windows checkpoint guest
/// 2026-09-23 with process-creation auditing; the same task started cleanly on
/// demand seconds later.
fn run_with_restarts(
    mut attempt: impl FnMut() -> io::Result<(i32, Duration)>,
    session_ending: impl Fn() -> bool,
    sleep: impl Fn(Duration),
) -> io::Result<i32> {
    let mut attempts = 0;
    loop {
        let outcome = attempt();
        if let Ok((_, uptime)) = &outcome
            && *uptime >= RESTART_CREDIT_UPTIME
        {
            attempts = 0;
        }
        // A clean exit is the resident being asked to stop, and a session end
        // is the OS taking the whole tree; neither is a crash to recover from.
        let finished = matches!(outcome, Ok((0, _)))
            || session_ending()
            || attempts >= INSTALLED_TASK_RESTART_LIMIT;
        if finished {
            return outcome.map(|(code, _)| code);
        }
        attempts += 1;
        sleep(INSTALLED_TASK_RESTART_DELAY);
        if session_ending() {
            return outcome.map(|(code, _)| code);
        }
    }
}

fn forward(
    program: &OsStr,
    arguments: &[OsString],
    admitted: Option<&AdmittedWindowsLaunch>,
    control: Option<LaunchControl>,
    environment: BTreeMap<OsString, OsString>,
) -> io::Result<i32> {
    let mut command = Vec::with_capacity(arguments.len() + 1);
    command.push(program.to_owned());
    command.extend_from_slice(arguments);
    let mut resources = BoundedHelperResources::new();
    if let Some(incoming) = admitted {
        resources.retain(std::sync::Arc::new(incoming.read_file_grants().to_vec()));
    }
    let reservation = Reservation::track_until(Instant::now() + JOB_HARD_STOP_TIMEOUT)
        .map_err(io::Error::other)?;
    // Best effort: a forwarder that cannot register still forwards; it merely
    // keeps the pre-existing immediate-exit behavior at session end.
    let _ = session_end::install();
    let mut owner = match launch_windows_forwarder(&command, &environment) {
        Ok(owner) => owner,
        Err(failure) => {
            return Err(io::Error::other(
                reservation.launch_failure(failure, resources),
            ));
        }
    };
    let outcome = (|| {
        let stop = match (control, admitted) {
            (Some(control), Some(incoming)) => {
                Some(control.admit(&owner, incoming.read_file_grants(), Some(incoming))?)
            }
            (Some(control), None) => Some(control.admit(&owner, &[], None)?),
            (None, None) => None,
            _ => return Err(io::Error::other("inconsistent forwarder admission")),
        };
        let mut stop_deadline = None;
        loop {
            if let Some(code) = owner.poll()? {
                // A root exit does not release the caller's grants while a
                // descendant still owns this generation or writes its output.
                if !owner.is_quiescent()? {
                    owner.hard_stop_until(Instant::now() + JOB_HARD_STOP_TIMEOUT)?;
                }
                return Ok(code);
            }
            if let Some(incoming) = admitted {
                if incoming.stop_requested()? && stop_deadline.is_none() {
                    signal_stop(
                        stop.as_ref()
                            .ok_or_else(|| io::Error::other("missing forwarding stop"))?,
                    )?;
                    stop_deadline = Some(Instant::now() + SERVICE_SHUTDOWN_TIMEOUT);
                }
            }
            if session_end::requested() && stop_deadline.is_none() {
                // Session end: ask the child to stop, then bound the drain to
                // what Windows actually grants. The supervisor watches this
                // same event and answers it with its bounded shutdown, which
                // is what clears the lifecycle markers; the standard
                // fifteen-second budget never finished before the OS killed
                // the tree.
                if let Some(stop) = stop.as_ref() {
                    signal_stop(stop)?;
                }
                stop_deadline = Some(Instant::now() + SESSION_END_DRAIN_TIMEOUT);
            }
            if stop_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return owner.hard_stop_until(Instant::now() + JOB_HARD_STOP_TIMEOUT);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    // Whatever happened, this forwarder is done with its tree: release the
    // held `WM_ENDSESSION` reply so the session ends without waiting out the
    // full ceiling.
    session_end::drained();
    match outcome {
        Ok(code) => Ok(code),
        Err(error) => Err(io::Error::other(reservation.independent_failure(
            owner,
            error.to_string(),
            resources,
        ))),
    }
}

#[cfg(test)]
mod restart_policy_tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn run(
        results: Vec<io::Result<(i32, Duration)>>,
        ending: bool,
    ) -> (io::Result<i32>, usize, usize) {
        let results = RefCell::new(results.into_iter());
        let calls = Cell::new(0);
        let sleeps = Cell::new(0);
        let outcome = run_with_restarts(
            || {
                calls.set(calls.get() + 1);
                results
                    .borrow_mut()
                    .next()
                    .expect("policy asked for one attempt too many")
            },
            || ending,
            |_| sleeps.set(sleeps.get() + 1),
        );
        (outcome, calls.get(), sleeps.get())
    }

    fn failed() -> io::Result<(i32, Duration)> {
        Err(io::Error::other(
            "launch deadline expired before the child existed",
        ))
    }

    #[test]
    fn a_failed_launch_is_retried_until_one_succeeds() {
        let (outcome, calls, sleeps) = run(
            vec![failed(), failed(), Ok((0, Duration::from_secs(1)))],
            false,
        );
        assert_eq!(outcome.unwrap(), 0);
        assert_eq!((calls, sleeps), (3, 2));
    }

    #[test]
    fn a_launch_that_keeps_failing_stops_at_the_limit_with_its_error() {
        let limit = INSTALLED_TASK_RESTART_LIMIT as usize;
        let (outcome, calls, sleeps) = run((0..=limit).map(|_| failed()).collect(), false);
        assert!(outcome.is_err());
        assert_eq!((calls, sleeps), (limit + 1, limit));
    }

    #[test]
    fn a_clean_exit_is_not_restarted() {
        let (outcome, calls, sleeps) = run(vec![Ok((0, Duration::from_secs(1)))], false);
        assert_eq!(outcome.unwrap(), 0);
        assert_eq!((calls, sleeps), (1, 0));
    }

    #[test]
    fn a_session_end_stops_retrying_a_failed_launch() {
        let (outcome, calls, sleeps) = run(vec![failed()], true);
        assert!(outcome.is_err());
        assert_eq!((calls, sleeps), (1, 0));
    }

    #[test]
    fn a_long_run_earns_back_the_restart_budget() {
        let limit = INSTALLED_TASK_RESTART_LIMIT as usize;
        let mut results: Vec<_> = (0..limit * 2)
            .map(|_| Ok((1, RESTART_CREDIT_UPTIME)))
            .collect();
        results.push(Ok((0, Duration::from_secs(1))));
        let (outcome, calls, _) = run(results, false);
        assert_eq!(outcome.unwrap(), 0);
        assert_eq!(calls, limit * 2 + 1);
    }
}
