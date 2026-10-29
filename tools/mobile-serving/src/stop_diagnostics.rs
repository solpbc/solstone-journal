// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Content-free observations of existing Stop waits. A phase returning is not
//! retirement completion, successful settlement, or permission to restart.

const TARGET: &str = "solstone_stop_phase";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    NativeJoin,
    RetirementCallback,
    OwnerDrop,
    ForegroundTransition,
    DiscoveryJoin,
    QueueShutdown,
    CallableJoin,
    SenseJoin,
    ScheduleJoin,
    RequestJoin,
    ServiceOwnerLock,
    ImportRetire,
    OwnerResponseDrain,
    ServiceTasksJoin,
    ShellDrain,
    BackupLifetimeDrain,
    TargetUseDrain,
    BusStop,
}

impl Phase {
    fn code(self) -> &'static str {
        match self {
            Self::NativeJoin => "native_join",
            Self::RetirementCallback => "retirement_callback",
            Self::OwnerDrop => "owner_drop",
            Self::ForegroundTransition => "foreground_transition",
            Self::DiscoveryJoin => "discovery_join",
            Self::QueueShutdown => "queue_shutdown",
            Self::CallableJoin => "callable_join",
            Self::SenseJoin => "sense_join",
            Self::ScheduleJoin => "schedule_join",
            Self::RequestJoin => "request_join",
            Self::ServiceOwnerLock => "service_owner_lock",
            Self::ImportRetire => "import_retire",
            Self::OwnerResponseDrain => "owner_response_drain",
            Self::ServiceTasksJoin => "service_tasks_join",
            Self::ShellDrain => "shell_drain",
            Self::BackupLifetimeDrain => "backup_lifetime_drain",
            Self::TargetUseDrain => "target_use_drain",
            Self::BusStop => "bus_stop",
        }
    }

    fn entered(self) {
        emit(self, false);
    }

    fn returned(self) {
        emit(self, true);
    }

    /// Deliberately no Drop guard: panic or an unfinished wait cannot emit a
    /// returned event. Returning an error remains distinct from Stop success.
    pub fn run<T>(self, work: impl FnOnce() -> T) -> T {
        self.entered();
        let result = work();
        self.returned();
        result
    }
}

fn emit(phase: Phase, returned: bool) {
    #[cfg(target_os = "android")]
    android::install();
    let state = if returned { "returned" } else { "entered" };
    log::info!(target: TARGET, "event=stop_phase phase={} state={state}", phase.code());
    #[cfg(all(test, feature = "full-tests"))]
    OBSERVER.with(|observer| {
        if let Some(observer) = observer.borrow().as_ref() {
            let _ = observer.send((phase, returned));
        }
    });
}

// The JNI host does not install a Rust logger. Install only if none exists;
// retain any host logger and its filter. This sink admits this target alone,
// so enabling Stop observations cannot expose other Rust records or content.
#[cfg(target_os = "android")]
mod android {
    use super::TARGET;
    use std::ffi::{CString, c_char, c_int};
    use std::sync::Once;

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    struct StopLogger;
    static LOGGER: StopLogger = StopLogger;
    static INSTALL: Once = Once::new();

    impl log::Log for StopLogger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.target() == TARGET && metadata.level() <= log::Level::Info
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                // Only the closed phase/state pair above reaches this sink.
                if let Ok(text) = CString::new(record.args().to_string()) {
                    unsafe { __android_log_write(4, c"solstone-stop".as_ptr(), text.as_ptr()) };
                }
            }
        }

        fn flush(&self) {}
    }

    pub(super) fn install() {
        INSTALL.call_once(|| {
            if log::set_logger(&LOGGER).is_ok() {
                log::set_max_level(log::LevelFilter::Info);
            }
        });
    }
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static OBSERVER: std::cell::RefCell<Option<std::sync::mpsc::Sender<(Phase, bool)>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn observe<T>(
    observer: std::sync::mpsc::Sender<(Phase, bool)>,
    work: impl FnOnce() -> T,
) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            OBSERVER.with(|observer| *observer.borrow_mut() = None);
        }
    }
    OBSERVER.with(|slot| *slot.borrow_mut() = Some(observer));
    let _clear = Clear;
    work()
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn phase_return_waits_for_the_actual_join() {
        let (release, held) = mpsc::channel();
        let worker = std::thread::spawn(move || held.recv().unwrap());
        let (events, observed) = mpsc::channel();
        let (joining, join_started) = mpsc::channel();
        let joiner = std::thread::spawn(move || {
            observe(events, || {
                Phase::CallableJoin.run(|| {
                    joining.send(()).unwrap();
                    worker.join().unwrap()
                })
            });
        });
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(5)).unwrap(),
            (Phase::CallableJoin, false)
        );
        // If a return were emitted before work, it must already be queued by
        // this handshake. Do not infer absence from the joiner's scheduling.
        join_started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(observed.try_recv(), Err(mpsc::TryRecvError::Empty));
        release.send(()).unwrap();
        joiner.join().unwrap();
        assert_eq!(observed.recv().unwrap(), (Phase::CallableJoin, true));
    }

    #[test]
    fn phase_return_waits_for_the_actual_destructor() {
        struct Owner {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Drop for Owner {
            fn drop(&mut self) {
                self.entered.send(()).unwrap();
                self.release.recv().unwrap();
            }
        }
        let (entered, dropping) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let (events, observed) = mpsc::channel();
        let owner = Owner {
            entered,
            release: held,
        };
        let worker = std::thread::spawn(move || {
            observe(events, || Phase::OwnerDrop.run(|| drop(owner)));
        });
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(5)).unwrap(),
            (Phase::OwnerDrop, false)
        );
        dropping.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(observed.try_recv(), Err(mpsc::TryRecvError::Empty));
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(observed.recv().unwrap(), (Phase::OwnerDrop, true));
    }

    #[test]
    fn returned_error_is_preserved_and_is_not_stop_success() {
        let (events, observed) = mpsc::channel();
        let result: Result<(), &str> = observe(events, || {
            Phase::RetirementCallback.run(|| Err("retained failure"))
        });
        assert_eq!(result, Err("retained failure"));
        assert_eq!(
            observed.into_iter().collect::<Vec<_>>(),
            vec![
                (Phase::RetirementCallback, false),
                (Phase::RetirementCallback, true)
            ]
        );
    }

    #[test]
    fn unwinding_does_not_report_a_returned_phase() {
        let (events, observed) = mpsc::channel();
        let result = std::panic::catch_unwind(|| {
            observe(events, || {
                Phase::OwnerDrop.run(|| panic!("owned destructor control"));
            });
        });
        assert!(result.is_err());
        assert_eq!(observed.recv().unwrap(), (Phase::OwnerDrop, false));
        assert_eq!(observed.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }
}
