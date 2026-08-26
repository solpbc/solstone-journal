// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Output};
use std::time::Duration;

use thiserror::Error;

use super::spawn::{ManagedProcess, SpawnError};
use super::terminate::SERVICE_SHUTDOWN_TIMEOUT;

/// How a launched child is owned and when it is expected to end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    IndependentLongLived,
    IndependentBoundedHelper { timeout: Duration },
    InheritedParentScope,
    ExplicitlyUnowned { reason: String },
}

/// Boundary-facing launch and termination failures.
#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("host capability unavailable: {needed}")]
    CapabilityUnavailable { needed: &'static str },
    #[error("ExplicitlyUnowned reason must be nonempty")]
    EmptyUnownedReason,
    #[error("failed to spawn child: {0}")]
    Spawn(#[source] io::Error),
    #[error(transparent)]
    SpawnManaged(SpawnError),
    #[error("post-spawn confirmation failed for pid {pid}: {source}")]
    ConfirmationFailed {
        pid: u32,
        #[source]
        source: io::Error,
    },
    #[error("failed to terminate child: {0}")]
    Terminate(#[source] io::Error),
    #[error("child output is unavailable")]
    OutputUnavailable,
}

pub type BoxedTerminateFn = Box<dyn FnMut(&mut Child, Duration) -> Result<(), LaunchError> + Send>;

enum Inner {
    Managed(ManagedProcess),
    Raw {
        child: Child,
        terminate_fn: BoxedTerminateFn,
    },
}

/// Retained termination authority for one launched child.
pub struct LaunchAuthority {
    inner: Option<Inner>,
    disposition: Disposition,
}

impl fmt::Debug for LaunchAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("LaunchAuthority");
        if self.inner.is_some() {
            debug.field("pid", &self.pid());
        }
        debug.field("disposition", &self.disposition).finish()
    }
}

impl LaunchAuthority {
    pub fn pid(&self) -> u32 {
        match self.inner() {
            Inner::Managed(process) => process.pid(),
            Inner::Raw { child, .. } => child.id(),
        }
    }

    pub fn disposition(&self) -> &Disposition {
        &self.disposition
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        match self.inner_mut() {
            Inner::Managed(process) => process.poll(),
            Inner::Raw { child, .. } => child
                .try_wait()
                .map(|status| status.map(|value| super::signal_aware_exit_code(&value))),
        }
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        match self.inner_mut() {
            Inner::Managed(process) => process.wait(),
            Inner::Raw { child, .. } => child
                .wait()
                .map(|status| super::signal_aware_exit_code(&status)),
        }
    }

    pub fn terminate(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        match self.inner_mut() {
            Inner::Managed(process) => process
                .terminate(timeout)
                .map(|_| ())
                .map_err(|error| LaunchError::Terminate(io::Error::other(error))),
            Inner::Raw {
                child,
                terminate_fn,
            } => {
                let signaled = terminate_fn(child, timeout);
                let waited = child.wait().map(|_| ()).map_err(LaunchError::Terminate);
                match (signaled, waited) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(error), _) => Err(error),
                    (Ok(()), Err(error)) => Err(error),
                }
            }
        }
    }

    pub fn terminate_exact(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        match self.inner_mut() {
            Inner::Managed(process) => process
                .terminate_exact(timeout)
                .map(|_| ())
                .map_err(|error| LaunchError::Terminate(io::Error::other(error))),
            Inner::Raw { .. } => Err(LaunchError::CapabilityUnavailable {
                needed: "birth-bound managed process termination",
            }),
        }
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        match self.inner_mut() {
            Inner::Managed(_) => None,
            Inner::Raw { child, .. } => child.stdin.take(),
        }
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        match self.inner_mut() {
            Inner::Managed(_) => None,
            Inner::Raw { child, .. } => child.stdout.take(),
        }
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        match self.inner_mut() {
            Inner::Managed(_) => None,
            Inner::Raw { child, .. } => child.stderr.take(),
        }
    }

    pub fn wait_with_output(mut self) -> Result<Output, LaunchError> {
        match self.inner.take() {
            Some(Inner::Managed(process)) => {
                drop(process);
                Err(LaunchError::OutputUnavailable)
            }
            Some(Inner::Raw { child, .. }) => {
                child.wait_with_output().map_err(LaunchError::Terminate)
            }
            None => Err(LaunchError::OutputUnavailable),
        }
    }

    pub fn cleanup(&mut self) {
        if let Inner::Managed(process) = self.inner_mut() {
            process.cleanup();
        }
    }

    fn inner(&self) -> &Inner {
        self.inner.as_ref().expect("launch authority inner")
    }

    fn inner_mut(&mut self) -> &mut Inner {
        self.inner.as_mut().expect("launch authority inner")
    }
}

impl Drop for LaunchAuthority {
    fn drop(&mut self) {
        let Some(Inner::Raw {
            mut child,
            mut terminate_fn,
        }) = self.inner.take()
        else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = terminate_fn(&mut child, SERVICE_SHUTDOWN_TIMEOUT);
            let _ = child.wait();
        }
    }
}

/// Launch a raw child after capability and post-spawn confirmation checks.
pub fn launch<F>(
    disposition: Disposition,
    spawn: F,
    terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> io::Result<Child>,
{
    launch_with(
        disposition,
        spawn,
        terminate_fn,
        production_capability_probe,
        production_confirm,
    )
}

/// Wrap an already-atomic `ManagedProcess::spawn` after the same pre-spawn checks.
pub fn launch_managed<F>(disposition: Disposition, spawn: F) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> Result<ManagedProcess, SpawnError>,
{
    launch_managed_with(disposition, spawn, production_capability_probe)
}

pub fn launch_with<F, Cap, Conf>(
    disposition: Disposition,
    spawn: F,
    mut terminate_fn: BoxedTerminateFn,
    capability: Cap,
    confirm: Conf,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> io::Result<Child>,
    Cap: FnOnce(&Disposition) -> Result<(), LaunchError>,
    Conf: FnOnce(u32) -> io::Result<()>,
{
    reject_empty_unowned_reason(&disposition)?;
    if claims_independent_scope(&disposition) {
        capability(&disposition)?;
    }
    let mut child = spawn().map_err(LaunchError::Spawn)?;
    if claims_independent_scope(&disposition) {
        let pid = child.id();
        if let Err(source) = confirm(pid) {
            let _ = terminate_fn(&mut child, SERVICE_SHUTDOWN_TIMEOUT);
            let _ = child.wait();
            return Err(LaunchError::ConfirmationFailed { pid, source });
        }
    }
    Ok(LaunchAuthority {
        inner: Some(Inner::Raw {
            child,
            terminate_fn,
        }),
        disposition,
    })
}

pub fn launch_managed_with<F, Cap>(
    disposition: Disposition,
    spawn: F,
    capability: Cap,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> Result<ManagedProcess, SpawnError>,
    Cap: FnOnce(&Disposition) -> Result<(), LaunchError>,
{
    reject_empty_unowned_reason(&disposition)?;
    if claims_independent_scope(&disposition) {
        capability(&disposition)?;
    }
    let process = spawn().map_err(LaunchError::SpawnManaged)?;
    Ok(LaunchAuthority {
        inner: Some(Inner::Managed(process)),
        disposition,
    })
}

fn reject_empty_unowned_reason(disposition: &Disposition) -> Result<(), LaunchError> {
    match disposition {
        Disposition::ExplicitlyUnowned { reason } if reason.trim().is_empty() => {
            Err(LaunchError::EmptyUnownedReason)
        }
        _ => Ok(()),
    }
}

const fn claims_independent_scope(disposition: &Disposition) -> bool {
    matches!(
        disposition,
        Disposition::IndependentLongLived | Disposition::IndependentBoundedHelper { .. }
    )
}

fn production_capability_probe(disposition: &Disposition) -> Result<(), LaunchError> {
    if !claims_independent_scope(disposition) {
        return Ok(());
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(LaunchError::CapabilityUnavailable {
            needed: "process-groups",
        })
    }
}

fn production_confirm(pid: u32) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let pid = i32::try_from(pid).map_err(|_| io::Error::other("invalid child pid"))?;
        nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(pid)))
            .map(|_| ())
            .map_err(io::Error::other)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "process groups unavailable on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::super::spawn::SpawnOptions;

    fn process_is_gone(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return true;
        };
        matches!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        )
    }

    fn wait_until_gone(pid: u32) {
        for _ in 0..200 {
            if process_is_gone(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("pid {pid} still alive");
    }

    fn panic_if_spawned() -> io::Result<Child> {
        panic!("spawn must not run");
    }

    fn kill_child(child: &mut Child, _: Duration) -> Result<(), LaunchError> {
        child.kill().map_err(LaunchError::Terminate)
    }

    fn unwrap_launch_err<T>(result: Result<T, LaunchError>, what: &str) -> LaunchError {
        match result {
            Err(error) => error,
            Ok(_) => panic!("{what}: expected error"),
        }
    }

    struct JournalBed {
        root: PathBuf,
    }

    impl JournalBed {
        fn new(name: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = PathBuf::from("/var/tmp").join(format!("solstone-authority-{name}-{stamp}"));
            fs::create_dir_all(&root).expect("temporary journal");
            Self { root }
        }
    }

    impl Drop for JournalBed {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn ac1_four_dispositions_are_constructible_and_empty_unowned_reason_rejects_before_spawn() {
        let _ = Disposition::IndependentLongLived;
        let _ = Disposition::IndependentBoundedHelper {
            timeout: Duration::from_secs(1),
        };
        let _ = Disposition::InheritedParentScope;
        let _ = Disposition::ExplicitlyUnowned {
            reason: "host diagnostic".to_owned(),
        };

        for reason in ["", "   ", "\t\n"] {
            let spawned = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&spawned);
            let error = unwrap_launch_err(
                launch(
                    Disposition::ExplicitlyUnowned {
                        reason: reason.to_owned(),
                    },
                    move || {
                        flag.store(true, Ordering::SeqCst);
                        panic_if_spawned()
                    },
                    Box::new(kill_child),
                ),
                "empty unowned reason",
            );
            assert!(
                matches!(error, LaunchError::EmptyUnownedReason),
                "{error:?}"
            );
            assert!(!spawned.load(Ordering::SeqCst));

            let spawned = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&spawned);
            let error = unwrap_launch_err(
                launch_managed(
                    Disposition::ExplicitlyUnowned {
                        reason: reason.to_owned(),
                    },
                    move || {
                        flag.store(true, Ordering::SeqCst);
                        panic!("managed spawn must not run");
                    },
                ),
                "empty unowned reason",
            );
            assert!(
                matches!(error, LaunchError::EmptyUnownedReason),
                "{error:?}"
            );
            assert!(!spawned.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn ac2_capability_failure_does_not_spawn_or_downgrade() {
        for disposition in [
            Disposition::IndependentLongLived,
            Disposition::IndependentBoundedHelper {
                timeout: Duration::from_secs(5),
            },
        ] {
            let spawned = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&spawned);
            let error = unwrap_launch_err(
                launch_with(
                    disposition,
                    move || {
                        flag.store(true, Ordering::SeqCst);
                        panic_if_spawned()
                    },
                    Box::new(kill_child),
                    |_| {
                        Err(LaunchError::CapabilityUnavailable {
                            needed: "process-groups",
                        })
                    },
                    |_| Ok(()),
                ),
                "capability",
            );
            assert!(
                matches!(
                    error,
                    LaunchError::CapabilityUnavailable {
                        needed: "process-groups"
                    }
                ),
                "{error:?}"
            );
            assert!(!spawned.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn launch_managed_delegates_pid_poll_and_terminate() {
        let bed = JournalBed::new("managed");
        let mut authority = launch_managed(Disposition::IndependentLongLived, || {
            ManagedProcess::spawn(
                vec!["/bin/sleep".to_owned(), "5".to_owned()],
                SpawnOptions {
                    journal_root: bed.root.clone(),
                    reference: "authority-managed".to_owned(),
                    day: None,
                    sink: None,
                    environment: BTreeMap::new(),
                },
            )
        })
        .expect("launch managed");
        let pid = authority.pid();
        assert!(pid > 1);
        assert!(matches!(authority.poll(), Ok(None)));
        authority
            .terminate(Duration::from_secs(2))
            .expect("terminate");
        wait_until_gone(pid);
        authority.cleanup();
    }
}
