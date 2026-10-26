// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared, argv-sensitive restore fixtures for dependent crate tests.

use std::io;
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;
use solstone_core_backup::BackupError;

use crate::engine::RestoreRecorder;
use crate::runner::{ToolOutput, ToolRequest, ToolRunner};

const RESTIC_VERSION: &str = "0.19.0";

const REFUSED_LATEST_EXIT: i32 = 97;
const REFUSED_UNEXPECTED_EXIT: i32 = 98;

/// A startup-only lookup fixture for an exact test in a fresh Android process.
/// The production permanent owner, busy slot, deadline and transport remain in use.
#[cfg(target_os = "android")]
pub fn android_dns_lookup_before_start(
    lookup: fn(&str, u16, ureq::config::IpFamily) -> io::Result<Vec<std::net::SocketAddr>>,
) -> Result<(), &'static str> {
    crate::controlled_http::android_dns::fixture_lookup_before_start(lookup)
}

#[cfg(target_os = "android")]
pub fn android_dns_lookup_idle() -> bool {
    crate::controlled_http::android_dns::fixture_idle()
}

#[cfg(target_os = "android")]
thread_local! {
    static TEARDOWN_RESTIC_FIXTURE: std::cell::RefCell<Option<Box<dyn ToolRunner>>> =
        const { std::cell::RefCell::new(None) };
}

/// Fixture restic responses after the coordinator's real native-tool admission.
/// HTTP, operation budget, coverage and local effect owners stay in production.
#[cfg(target_os = "android")]
pub fn with_teardown_restic_fixture<T>(
    runner: impl ToolRunner + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    TEARDOWN_RESTIC_FIXTURE.with(|slot| {
        assert!(slot.borrow().is_none());
        *slot.borrow_mut() = Some(Box::new(runner));
    });
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            TEARDOWN_RESTIC_FIXTURE.with(|slot| {
                slot.borrow_mut().take();
            });
        }
    }
    let _guard = Guard;
    operation()
}

#[cfg(target_os = "android")]
pub(crate) fn teardown_restic_fixture(request: &ToolRequest<'_>) -> Option<io::Result<ToolOutput>> {
    TEARDOWN_RESTIC_FIXTURE.with(|slot| slot.borrow().as_ref().map(|runner| runner.run(request)))
}

/// Install the actual packaged closure for a native libtest process. This uses
/// the same immutable pin, manifest, receipt and load-closure checks as startup.
#[cfg(target_os = "android")]
pub fn install_packaged_android_test_tools(native: &Path, assets: &Path) {
    solstone_core_spp_attest::android_service_tools::install_android_service_tools(
        native,
        &assets.join("resources"),
        &std::fs::read(assets.join("manifest.json")).unwrap(),
        &std::fs::read(assets.join("mobile-native-build.json")).unwrap(),
    )
    .unwrap();
}

type OwnedReadAdmissionHook = Box<dyn FnOnce()>;
thread_local! {
    static OWNED_READ_ADMISSION_HOOK: std::cell::RefCell<Option<OwnedReadAdmissionHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Run on the admission thread; the cut follows its real Version/Config children.
pub fn run_with_owned_read_admission_hook<T>(
    hook: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    OWNED_READ_ADMISSION_HOOK.with(|slot| {
        assert!(slot.borrow().is_none());
        *slot.borrow_mut() = Some(Box::new(hook));
    });
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            OWNED_READ_ADMISSION_HOOK.with(|slot| {
                slot.borrow_mut().take();
            });
        }
    }
    let _guard = Guard;
    let result = operation();
    let consumed = OWNED_READ_ADMISSION_HOOK.with(|slot| slot.borrow().is_none());
    (result, consumed)
}

pub(crate) fn after_owned_read_admission() {
    let hook = OWNED_READ_ADMISSION_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(test)]
mod owned_read_admission_hook_tests {
    use super::{after_owned_read_admission, run_with_owned_read_admission_hook};

    #[test]
    fn unwind_clears_the_hook_before_the_next_admission() {
        let panic = std::panic::catch_unwind(|| {
            run_with_owned_read_admission_hook(
                || panic!("actual admission-cut unwind"),
                after_owned_read_admission,
            );
        });
        assert!(panic.is_err());
        let observed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let captured = std::sync::Arc::clone(&observed);
        let ((), consumed) = run_with_owned_read_admission_hook(
            move || captured.store(true, std::sync::atomic::Ordering::SeqCst),
            after_owned_read_admission,
        );
        assert!(consumed);
        assert!(observed.load(std::sync::atomic::Ordering::SeqCst));

        let unused = std::panic::catch_unwind(|| {
            run_with_owned_read_admission_hook(
                || panic!("unused callback must be cleared"),
                || panic!("before admission"),
            );
        });
        assert!(unused.is_err());
        after_owned_read_admission();
    }
}

pub struct ArgvResticFixture {
    catalog: String,
    restore: ToolOutput,
    check: ToolOutput,
    calls: Mutex<Vec<Vec<String>>>,
    refusals: Mutex<Vec<Vec<String>>>,
}

impl ArgvResticFixture {
    pub fn new(catalog: impl Into<String>, restore: ToolOutput, check: ToolOutput) -> Self {
        Self {
            catalog: catalog.into(),
            restore,
            check,
            calls: Mutex::new(vec![]),
            refusals: Mutex::new(vec![]),
        }
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().expect("fixture calls lock").clone()
    }

    pub fn refusals(&self) -> Vec<Vec<String>> {
        self.refusals.lock().expect("fixture refusals lock").clone()
    }

    fn refuse(&self, argv: Vec<String>, code: i32) -> io::Result<ToolOutput> {
        self.refusals
            .lock()
            .expect("fixture refusals lock")
            .push(argv);
        Ok(ToolOutput {
            returncode: code,
            stdout: vec![],
            stderr: vec![],
        })
    }
}

impl ToolRunner for ArgvResticFixture {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let argv = request
            .argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        self.calls
            .lock()
            .expect("fixture calls lock")
            .push(argv.clone());
        match argv.first().map(String::as_str) {
            Some("version") if argv == ["version"] => Ok(ToolOutput {
                returncode: 0,
                stdout: format!("restic {RESTIC_VERSION}\n").into_bytes(),
                stderr: vec![],
            }),
            Some("snapshots") if argv.iter().any(|argument| argument == "latest") => {
                self.refuse(argv, REFUSED_LATEST_EXIT)
            }
            Some("snapshots") if argv == ["snapshots", "--json"] => Ok(ToolOutput {
                returncode: 0,
                stdout: self.catalog.as_bytes().to_vec(),
                stderr: vec![],
            }),
            Some("restore") if admitted_restore_argv(&argv) => Ok(self.restore.clone()),
            Some("check") if argv == ["check"] => Ok(self.check.clone()),
            _ => self.refuse(argv, REFUSED_UNEXPECTED_EXIT),
        }
    }

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        _cancellation: &solstone_core_backup::archive_admission::ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        self.run(request)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RestoreRecord {
    pub status: String,
    pub time: Value,
    pub reason: Value,
    pub scope: String,
    pub day: Value,
    pub segments_selected: Value,
    pub segments_restored: Value,
    pub files_expected: Value,
    pub files_restored: Value,
    pub bytes_expected: Value,
    pub bytes_restored: Value,
}

pub struct RestoreRecorderSpy {
    fail: bool,
    calls: Mutex<Vec<RestoreRecord>>,
}

impl RestoreRecorderSpy {
    pub fn new() -> Self {
        Self {
            fail: false,
            calls: Mutex::new(vec![]),
        }
    }

    pub fn failing() -> Self {
        Self {
            fail: true,
            calls: Mutex::new(vec![]),
        }
    }

    pub fn calls(&self) -> Vec<RestoreRecord> {
        self.calls.lock().expect("restore recorder lock").clone()
    }
}

impl Default for RestoreRecorderSpy {
    fn default() -> Self {
        Self::new()
    }
}

impl RestoreRecorder for RestoreRecorderSpy {
    fn record(
        &self,
        _: &Path,
        status: &str,
        time: Value,
        reason: Value,
        scope: &str,
        day: Value,
        segments_selected: Value,
        segments_restored: Value,
        files_expected: Value,
        files_restored: Value,
        bytes_expected: Value,
        bytes_restored: Value,
    ) -> Result<(), BackupError> {
        self.calls
            .lock()
            .expect("restore recorder lock")
            .push(RestoreRecord {
                status: status.to_owned(),
                time,
                reason,
                scope: scope.to_owned(),
                day,
                segments_selected,
                segments_restored,
                files_expected,
                files_restored,
                bytes_expected,
                bytes_restored,
            });
        if self.fail {
            Err(BackupError::InvalidRestoreStatus)
        } else {
            Ok(())
        }
    }
}

/// A restore argv as the journal builds it: the snapshot tree onto a target,
/// keeping this machine's resident identity, then the optional lock exclude.
fn admitted_restore_argv(argv: &[String]) -> bool {
    let [command, snapshot, target, _, rest @ ..] = argv else {
        return false;
    };
    if command != "restore" || !snapshot.contains(':') || target != "--target" {
        return false;
    }
    let mut expected: Vec<&str> = crate::restore::RESTORE_KEEPS_LOCAL
        .iter()
        .flat_map(|pattern| ["--exclude", *pattern])
        .collect();
    expected.extend(["--exclude", "/health/whole-journal-restore.json"]);
    expected.extend(["--exclude", "/health/!solstone-claim-00000000-*"]);
    let keeps = expected.len();
    expected.push("--json");
    let with_lock = {
        let mut with_lock = expected[..keeps].to_vec();
        with_lock.extend(["--exclude", "backup-activity.lock", "--json"]);
        with_lock
    };
    rest == expected.as_slice() || rest == with_lock.as_slice()
}
