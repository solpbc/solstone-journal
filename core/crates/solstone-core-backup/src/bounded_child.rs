// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The concrete Unix bounded child. Public results carry output, not writer authority.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, pipe};

use crate::archive_admission::ArchiveCancellation;

const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A command running archive work stops it on the first SIGINT or SIGTERM: its
/// children are killed and reaped and its result is still reported. A second
/// signal takes the default action.
pub fn stop_archive_work_on_signals() {
    extern "C" fn stop(_: nix::libc::c_int) {
        crate::archive_admission::stop_archive_work();
    }
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
    let action = SigAction::new(
        SigHandler::Handler(stop),
        SaFlags::SA_RESETHAND | SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    for signal in [Signal::SIGINT, Signal::SIGTERM] {
        // SAFETY: the handler performs one atomic store, which is async-signal-safe.
        #[allow(unsafe_code)]
        unsafe {
            let _ = sigaction(signal, &action);
        }
    }
}

/// Cancelled by its operation, or the whole process is stopping.
fn stop_requested(cancellation: Option<&ArchiveCancellation>) -> bool {
    crate::archive_admission::archive_work_stopping()
        || cancellation.is_some_and(ArchiveCancellation::is_cancelled)
}

pub struct Request<'a> {
    pub program: OsString,
    pub argv: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub timeout: Option<Duration>,
    pub pass_fds: Vec<BorrowedFd<'a>>,
}

pub struct Output {
    pub returncode: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(crate) struct CompletedChild {
    output: Output,
}

impl CompletedChild {
    pub(crate) fn successful(&self) -> bool {
        self.output.returncode == 0
    }

    pub(crate) fn output(&self) -> &Output {
        &self.output
    }

    pub(crate) fn into_output(self) -> Output {
        self.output
    }
}

pub(crate) struct ChildFailure {
    error: io::Error,
    settled: bool,
}

impl ChildFailure {
    fn into_error(self) -> io::Error {
        self.error
    }

    /// No child of this run is alive: none was spawned, or the failure came
    /// after the group was killed and the root reaped.
    pub(crate) fn settled(&self) -> bool {
        self.settled
    }
}

pub fn run(request: &Request<'_>) -> io::Result<Output> {
    run_owned(request, None, None)
        .map(CompletedChild::into_output)
        .map_err(ChildFailure::into_error)
}

pub fn run_cancellable(
    request: &Request<'_>,
    cancellation: &ArchiveCancellation,
) -> io::Result<Output> {
    run_owned(request, None, Some(cancellation))
        .map(CompletedChild::into_output)
        .map_err(ChildFailure::into_error)
}

/// Only actual execution creates completion. A capture bound drains excess
/// bytes rather than leaving a child blocked on its output pipe.
pub(crate) fn run_owned(
    request: &Request<'_>,
    capture_limit: Option<usize>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<CompletedChild, ChildFailure> {
    execute(request, capture_limit, cancellation)
}

/// Fixed creator requests retain the same private completion and owned joins.
#[cfg(not(target_os = "macos"))]
pub(crate) fn run_owned_archive_creator(
    request: &Request<'_>,
    capture_limit: Option<usize>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<CompletedChild, ChildFailure> {
    execute_request(
        request,
        capture_limit,
        cancellation,
        ChildSetup::Creator(()),
    )
}

/// The creator starts inside `anchor`, a held directory, and reads its
/// sources relative to it. No descriptor is passed.
#[cfg(target_os = "macos")]
pub(crate) fn run_owned_archive_creator_at(
    request: &Request<'_>,
    anchor: BorrowedFd<'_>,
    capture_limit: Option<usize>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<CompletedChild, ChildFailure> {
    execute_request(
        request,
        capture_limit,
        cancellation,
        ChildSetup::Creator(anchor.as_raw_fd()),
    )
}

#[cfg(target_os = "macos")]
type CreatorAnchor = std::os::fd::RawFd;
#[cfg(not(target_os = "macos"))]
type CreatorAnchor = ();

#[derive(Clone, Copy)]
enum ChildSetup {
    Ordinary,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Dns(Instant),
    Creator(CreatorAnchor),
}

/// The DNS leaf supplies only an admitted executable, fixed DNS arguments and
/// the original absolute end. No ambient environment or descriptors are passed.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn run_owned_dns(
    program: &std::ffi::OsStr,
    argv: &[OsString],
    deadline: Instant,
    cancellation: &ArchiveCancellation,
    capture_limit: usize,
) -> io::Result<CompletedChild> {
    let request = Request {
        program: program.to_owned(),
        argv: argv.to_vec(),
        env: BTreeMap::new(),
        timeout: None,
        pass_fds: Vec::new(),
    };
    execute_request(
        &request,
        Some(capture_limit),
        Some(cancellation),
        ChildSetup::Dns(deadline),
    )
    .map_err(ChildFailure::into_error)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Observe,
    GroupCleanup,
    CloseOwned,
    Reap,
    CollectReaders,
}

trait SessionIo {
    fn observe_exit_without_reap(&mut self) -> io::Result<bool>;
    fn group_cleanup(&mut self) -> io::Result<()>;
    fn close_owned_endpoints(&mut self);
    fn reap_root(&mut self) -> io::Result<i32>;
    fn stop_readers(&self);
    fn collect_readers(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)>;
}

fn complete_session<S: SessionIo>(
    session: &mut S,
    mut trace: Option<&mut Vec<Step>>,
) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    let mut record = |step| {
        if let Some(trace) = &mut trace {
            trace.push(step);
        }
    };
    record(Step::Observe);
    let observation_error = session.observe_exit_without_reap().err();
    record(Step::GroupCleanup);
    let cleanup_error = session.group_cleanup().err();
    record(Step::CloseOwned);
    session.close_owned_endpoints();
    record(Step::Reap);
    let reaped = session.reap_root();
    if observation_error.is_some() || cleanup_error.is_some() || reaped.is_err() {
        session.stop_readers();
    }
    record(Step::CollectReaders);
    let readers = session.collect_readers();
    if let Some(error) = observation_error.or(cleanup_error) {
        return Err(error);
    }
    let status = reaped?;
    let (stdout, stderr) = readers?;
    Ok((status, stdout, stderr))
}

type OutputReader = thread::JoinHandle<io::Result<Vec<u8>>>;

#[derive(Default)]
struct ReaderSignals {
    stop: AtomicBool,
    failed: AtomicBool,
}

struct ReaderFailureGuard {
    signals: Arc<ReaderSignals>,
    succeeded: bool,
}

impl Drop for ReaderFailureGuard {
    fn drop(&mut self) {
        if !self.succeeded {
            self.signals.failed.store(true, Ordering::Release);
            self.signals.stop.store(true, Ordering::Release);
        }
    }
}

struct SpawnedChild {
    child: Child,
    pgid: Pid,
    reaped: bool,
    stdout_w: Option<OwnedFd>,
    stderr_w: Option<OwnedFd>,
    stdout_reader: Option<OutputReader>,
    stderr_reader: Option<OutputReader>,
    reader_signals: Arc<ReaderSignals>,
}

impl Drop for SpawnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            // The owned Child remains armed immediately after spawn, including
            // group confirmation and every later error/unwind path.
            let _ = kill_group(self.pgid);
            let _ = self.child.kill();
            self.close_owned_endpoints();
            let _ = self.child.wait();
        }
        self.close_owned_endpoints();
        self.stop_readers();
        let _ = self.collect_readers();
    }
}

impl SessionIo for SpawnedChild {
    fn observe_exit_without_reap(&mut self) -> io::Result<bool> {
        observe_exit_without_reap(self.pgid)
    }

    fn group_cleanup(&mut self) -> io::Result<()> {
        let result = kill_group(self.pgid);
        // Keep the retained root stoppable even if it left its original group.
        let _ = self.child.kill();
        result
    }

    fn close_owned_endpoints(&mut self) {
        drop(self.stdout_w.take());
        drop(self.stderr_w.take());
    }

    fn reap_root(&mut self) -> io::Result<i32> {
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status
            .code()
            .unwrap_or_else(|| -status.signal().unwrap_or(1)))
    }

    fn collect_readers(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)> {
        // Join both even when the first reader failed.
        let stdout = self.stdout_reader.take().map(OutputReader::join);
        let stderr = self.stderr_reader.take().map(OutputReader::join);
        let joined = |reader: Option<thread::Result<io::Result<Vec<u8>>>>| {
            reader
                .unwrap_or_else(|| Ok(Ok(Vec::new())))
                .map_err(|_| io::Error::other("child output reader panicked"))?
        };
        Ok((joined(stdout)?, joined(stderr)?))
    }

    fn stop_readers(&self) {
        self.reader_signals.stop.store(true, Ordering::Release);
    }
}

fn kill_group(pgid: Pid) -> io::Result<()> {
    match killpg(pgid, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        #[cfg(target_os = "macos")]
        Err(Errno::EPERM) if matches!(nix::unistd::getpgid(Some(pgid)), Err(Errno::ESRCH)) => {
            Ok(())
        }
        Err(error) => Err(io::Error::from(error)),
    }
}

fn observe_exit_without_reap(pid: Pid) -> io::Result<bool> {
    let pid = rustix::process::Pid::from_raw(pid.as_raw())
        .ok_or_else(|| io::Error::other("invalid child pid"))?;
    rustix::process::waitid(
        rustix::process::WaitId::Pid(pid),
        rustix::process::WaitIdOptions::EXITED
            | rustix::process::WaitIdOptions::NOHANG
            | rustix::process::WaitIdOptions::NOWAIT,
    )
    .map(|status| status.is_some())
    .map_err(io::Error::from)
}

fn pipe_cloexec() -> io::Result<(OwnedFd, OwnedFd)> {
    let (read, write) = pipe().map_err(io::Error::from)?;
    for fd in [read.as_fd(), write.as_fd()] {
        let flags = fcntl(fd, FcntlArg::F_GETFD).map_err(io::Error::from)?;
        fcntl(
            fd,
            FcntlArg::F_SETFD(FdFlag::from_bits_truncate(flags) | FdFlag::FD_CLOEXEC),
        )
        .map_err(io::Error::from)?;
    }
    Ok((read, write))
}

fn inherit_in_child(command: &mut Command, descriptors: &[BorrowedFd<'_>]) -> io::Result<()> {
    let mut inherited = Vec::with_capacity(descriptors.len());
    for fd in descriptors {
        // Validate every descriptor before spawning; never mutate parent flags.
        let flags = fcntl(*fd, FcntlArg::F_GETFD).map_err(io::Error::from)?;
        inherited.push((fd.as_raw_fd(), flags & !FdFlag::FD_CLOEXEC.bits()));
    }
    // SAFETY: all allocation and validation happen above. The child hook calls
    // only async-signal-safe fcntl and constructs raw OS errors on failure.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || {
            for &(fd, flags) in &inherited {
                if nix::libc::fcntl(fd, nix::libc::F_SETFD, flags) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn prepare_dns_child(command: &mut Command) {
    let expected_parent = std::process::id() as nix::libc::pid_t;
    #[cfg(target_os = "linux")]
    let close_range_cloexec = nix::libc::CLOSE_RANGE_CLOEXEC;
    // Android's libc crate exposes the syscall but not this Linux UAPI flag.
    #[cfg(target_os = "android")]
    let close_range_cloexec: nix::libc::c_uint = 1 << 2;
    // SAFETY: this child-local hook allocates nothing. Only direct Linux OS
    // calls run between fork and exec; no parent descriptor flags are changed.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || {
            // The raw syscall, not libc's close_range(): the release targets are musl, whose
            // libc bindings declare no close_range wrapper (glibc's is a thin wrapper too).
            if nix::libc::syscall(
                nix::libc::SYS_close_range,
                nix::libc::c_long::from(3_u8),
                nix::libc::c_long::from(nix::libc::c_uint::MAX),
                nix::libc::c_long::from(close_range_cloexec),
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            if nix::libc::getppid() != expected_parent {
                nix::libc::kill(nix::libc::getpid(), nix::libc::SIGKILL);
                return Err(io::Error::from_raw_os_error(nix::libc::ECANCELED));
            }
            Ok(())
        });
    }
}

/// No descriptor survives exec, and a child whose parent already left is killed
/// before exec. macOS has neither `close_range` nor a parent-death signal: the
/// sweep marks every descriptor below the soft limit close-on-exec, and the DNS
/// entry itself watches its parent before resolving anything.
#[cfg(target_os = "macos")]
fn prepare_dns_child(command: &mut Command) {
    // An unbounded or unreadable limit still sweeps a large fixed range.
    const SWEEP_CEILING: nix::libc::c_int = 1 << 20;
    let expected_parent = std::process::id() as nix::libc::pid_t;
    let ceiling = nix::sys::resource::getrlimit(nix::sys::resource::Resource::RLIMIT_NOFILE)
        .ok()
        .and_then(|(soft, _)| nix::libc::c_int::try_from(soft).ok())
        .map_or(SWEEP_CEILING, |soft| soft.min(SWEEP_CEILING));
    // SAFETY: the bound is computed above. Between fork and exec only fcntl,
    // getppid, getpid and kill run, all async-signal-safe; nothing allocates and
    // no parent descriptor flag changes.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || {
            for fd in 3..ceiling {
                let flags = nix::libc::fcntl(fd, nix::libc::F_GETFD);
                if flags >= 0
                    && flags & nix::libc::FD_CLOEXEC == 0
                    && nix::libc::fcntl(fd, nix::libc::F_SETFD, flags | nix::libc::FD_CLOEXEC) != 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
            if nix::libc::getppid() != expected_parent {
                nix::libc::kill(nix::libc::getpid(), nix::libc::SIGKILL);
                return Err(io::Error::from_raw_os_error(nix::libc::ECANCELED));
            }
            Ok(())
        });
    }
}

fn read_output(
    fd: OwnedFd,
    limit: Option<usize>,
    signals: &ReaderSignals,
    start: Instant,
    timeout: Option<Duration>,
    cancellation: Option<&ArchiveCancellation>,
) -> io::Result<Vec<u8>> {
    let flags = fcntl(fd.as_fd(), FcntlArg::F_GETFL).map_err(io::Error::from)?;
    fcntl(
        fd.as_fd(),
        FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
    )
    .map_err(io::Error::from)?;
    let mut file = File::from(fd);
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut exceeded = false;
    loop {
        if signals.stop.load(Ordering::Acquire)
            || stop_requested(cancellation)
            || timeout.is_some_and(|timeout| start.elapsed() >= timeout)
        {
            break;
        }
        let count = match file.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(POLL_INTERVAL);
                continue;
            }
            result => result?,
        };
        if count == 0 {
            break;
        }
        let retained = limit.map_or(count, |limit| count.min(limit.saturating_sub(bytes.len())));
        bytes.extend_from_slice(&buffer[..retained]);
        exceeded |= retained != count;
    }
    if exceeded {
        return Err(io::Error::other("child output exceeded capture bound"));
    }
    Ok(bytes)
}

fn spawn_reader(
    fd: OwnedFd,
    limit: Option<usize>,
    signals: Arc<ReaderSignals>,
    start: Instant,
    timeout: Option<Duration>,
    cancellation: Option<ArchiveCancellation>,
) -> io::Result<OutputReader> {
    thread::Builder::new().spawn(move || {
        let mut guard = ReaderFailureGuard {
            signals,
            succeeded: false,
        };
        let result = read_output(
            fd,
            limit,
            &guard.signals,
            start,
            timeout,
            cancellation.as_ref(),
        );
        guard.succeeded = result.is_ok();
        result
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Exited,
    TimedOut,
    Cancelled,
    ReaderFailed,
}

fn await_exit(
    session: &mut SpawnedChild,
    start: Instant,
    timeout: Option<Duration>,
    cancellation: Option<&ArchiveCancellation>,
) -> io::Result<StopReason> {
    loop {
        if stop_requested(cancellation) {
            return Ok(StopReason::Cancelled);
        }
        if session.reader_signals.failed.load(Ordering::Acquire) {
            return Ok(StopReason::ReaderFailed);
        }
        if timeout.is_some_and(|timeout| start.elapsed() >= timeout) {
            return Ok(StopReason::TimedOut);
        }
        if session.observe_exit_without_reap()? {
            return Ok(StopReason::Exited);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn execute(
    request: &Request<'_>,
    capture_limit: Option<usize>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<CompletedChild, ChildFailure> {
    execute_request(request, capture_limit, cancellation, ChildSetup::Ordinary)
}

fn execute_request(
    request: &Request<'_>,
    capture_limit: Option<usize>,
    cancellation: Option<&ArchiveCancellation>,
    setup: ChildSetup,
) -> Result<CompletedChild, ChildFailure> {
    let mut settled = true;
    execute_tracked(request, capture_limit, cancellation, setup, &mut settled)
        .map_err(|error| ChildFailure { error, settled })
}

fn execute_tracked(
    request: &Request<'_>,
    capture_limit: Option<usize>,
    cancellation: Option<&ArchiveCancellation>,
    setup: ChildSetup,
    settled: &mut bool,
) -> io::Result<CompletedChild> {
    let dns_deadline: Option<Instant> = match setup {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        ChildSetup::Dns(deadline) => Some(deadline),
        _ => None,
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "android")))]
    if matches!(setup, ChildSetup::Creator(_)) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "archive creator containment is unavailable",
        ));
    }
    let start = Instant::now();
    let timeout = dns_deadline
        .map(|deadline| deadline.saturating_duration_since(start))
        .or(request.timeout);
    let cancelled = || io::Error::new(io::ErrorKind::Interrupted, "archive operation cancelled");
    if stop_requested(cancellation) {
        return Err(cancelled());
    }
    if dns_deadline.is_some_and(|deadline| start >= deadline) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "HTTP DNS deadline reached",
        ));
    }
    let (stdout_r, stdout_w) = pipe_cloexec()?;
    let (stderr_r, stderr_w) = pipe_cloexec()?;
    let mut command = Command::new(&request.program);
    command.args(&request.argv).env_clear().envs(&request.env);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_w.try_clone()?))
        .stderr(Stdio::from(stderr_w.try_clone()?));
    command.process_group(0);
    #[cfg(target_os = "linux")]
    match setup {
        ChildSetup::Dns(_) => prepare_dns_child(&mut command),
        ChildSetup::Creator(()) => {
            let mut seen = std::collections::BTreeSet::new();
            let mut allowed = Vec::new();
            for descriptor in &request.pass_fds {
                if descriptor.as_raw_fd() < 3 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid archive descriptor",
                    ));
                }
                if seen.insert(descriptor.as_raw_fd()) {
                    allowed.push(*descriptor);
                }
            }
            // Scrub every extra descriptor first, then allow only retained capabilities.
            prepare_dns_child(&mut command);
            inherit_in_child(&mut command, &allowed)?;
        }
        ChildSetup::Ordinary => {
            prepare_dns_child(&mut command);
            inherit_in_child(&mut command, &request.pass_fds)?;
        }
    }
    // Scrub every extra descriptor first, then allow only the passed ones.
    #[cfg(target_os = "macos")]
    match setup {
        ChildSetup::Dns(_) => prepare_dns_child(&mut command),
        ChildSetup::Creator(anchor) => {
            if anchor < 3 || !request.pass_fds.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid archive anchor",
                ));
            }
            prepare_dns_child(&mut command);
            // SAFETY: fchdir is async-signal-safe and allocates nothing. The
            // anchor stays open in the parent until this synchronous call returns.
            #[allow(unsafe_code)]
            unsafe {
                command.pre_exec(move || {
                    if nix::libc::fchdir(anchor) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        ChildSetup::Ordinary => {
            prepare_dns_child(&mut command);
            inherit_in_child(&mut command, &request.pass_fds)?;
        }
    }
    #[cfg(target_os = "android")]
    match setup {
        ChildSetup::Creator(()) => {
            let mut seen = std::collections::BTreeSet::new();
            let mut allowed = Vec::new();
            for descriptor in &request.pass_fds {
                if descriptor.as_raw_fd() < 3 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid archive descriptor",
                    ));
                }
                if seen.insert(descriptor.as_raw_fd()) {
                    allowed.push(*descriptor);
                }
            }
            // Same contained lifetime as the Linux creator: close from 3, then
            // inherit only the retained descriptors. Ordinary backup stays below.
            prepare_dns_child(&mut command);
            inherit_in_child(&mut command, &allowed)?;
        }
        ChildSetup::Ordinary => {
            prepare_dns_child(&mut command);
            inherit_in_child(&mut command, &request.pass_fds)?;
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
    inherit_in_child(&mut command, &request.pass_fds)?;
    if stop_requested(cancellation) {
        return Err(cancelled());
    }
    if dns_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "HTTP DNS deadline reached",
        ));
    }
    let child = command.spawn()?;
    // Declared before the session, so it is released after the session's drop reaps.
    let _live = crate::archive_admission::LiveArchiveChild::enter();
    *settled = false;
    let pgid = Pid::from_raw(child.id() as i32);
    let mut session = SpawnedChild {
        child,
        pgid,
        reaped: false,
        stdout_w: Some(stdout_w),
        stderr_w: Some(stderr_w),
        stdout_reader: None,
        stderr_reader: None,
        reader_signals: Arc::default(),
    };
    // Command owns cloned write ends until dropped, even after spawn.
    drop(command);
    nix::unistd::getpgid(Some(pgid)).map_err(io::Error::other)?;
    session.stdout_reader = Some(spawn_reader(
        stdout_r,
        capture_limit,
        session.reader_signals.clone(),
        start,
        timeout,
        cancellation.cloned(),
    )?);
    session.stderr_reader = Some(spawn_reader(
        stderr_r,
        capture_limit,
        session.reader_signals.clone(),
        start,
        timeout,
        cancellation.cloned(),
    )?);
    let stopped = await_exit(&mut session, start, timeout, cancellation);
    if stopped.is_err() || matches!(&stopped, Ok(StopReason::ReaderFailed)) {
        session.stop_readers();
    }
    let (status, stdout, stderr) = complete_session(&mut session, None)?;
    // A restic that joined the creator entry's group while that group was being
    // stopped outlives its reaped leader; the group id stays its own until it exits.
    #[cfg(target_os = "macos")]
    if matches!(setup, ChildSetup::Creator(_)) {
        let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
    }
    // The group was killed and the root reaped; later refusals leave nothing alive.
    *settled = true;
    let stopped = stopped?;
    if stop_requested(cancellation) {
        return Err(cancelled());
    }
    if stopped == StopReason::ReaderFailed {
        return Err(io::Error::other("child output reader failed"));
    }
    Ok(CompletedChild {
        output: Output {
            returncode: if stopped == StopReason::TimedOut
                || timeout.is_some_and(|timeout| start.elapsed() >= timeout)
            {
                124
            } else if status >= 0 {
                status
            } else {
                1
            },
            stdout,
            stderr,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(target_os = "linux", feature = "full-tests"))]
    mod cooperation;

    #[test]
    fn completion_reaps_before_collecting_and_preserves_cleanup_error() {
        struct Fake {
            cleanup_fails: bool,
        }
        impl SessionIo for Fake {
            fn observe_exit_without_reap(&mut self) -> io::Result<bool> {
                Ok(true)
            }
            fn group_cleanup(&mut self) -> io::Result<()> {
                if self.cleanup_fails {
                    Err(io::Error::other("cleanup"))
                } else {
                    Ok(())
                }
            }
            fn close_owned_endpoints(&mut self) {}
            fn reap_root(&mut self) -> io::Result<i32> {
                Ok(0)
            }
            fn stop_readers(&self) {}
            fn collect_readers(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)> {
                Ok((vec![], vec![]))
            }
        }
        for fails in [false, true] {
            let mut steps = Vec::new();
            let result = complete_session(
                &mut Fake {
                    cleanup_fails: fails,
                },
                Some(&mut steps),
            );
            let mut expected = vec![
                Step::Observe,
                Step::GroupCleanup,
                Step::CloseOwned,
                Step::Reap,
            ];
            expected.push(Step::CollectReaders);
            assert_eq!(steps, expected);
            assert_eq!(result.is_err(), fails);
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn dns_child_inherits_no_descriptor_its_parent_left_open() {
        let (read, write) = pipe().unwrap();
        for fd in [read.as_fd(), write.as_fd()] {
            fcntl(fd, FcntlArg::F_SETFD(FdFlag::empty())).unwrap();
        }
        let probe = format!(
            "for fd in {} {}; do [ -e /dev/fd/$fd ] && echo leaked; done; echo done",
            read.as_raw_fd(),
            write.as_raw_fd()
        );
        // Control: an ordinary spawn sees both descriptors, so the probe detects a leak.
        let control = Command::new("/bin/sh")
            .args(["-c", &probe])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(control.stdout, b"leaked\nleaked\ndone\n");
        let request = Request {
            program: "/bin/sh".into(),
            argv: vec!["-c".into(), probe.into()],
            env: BTreeMap::new(),
            timeout: None,
            pass_fds: Vec::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        let completed = execute_request(
            &request,
            Some(4096),
            Some(&ArchiveCancellation::new()),
            ChildSetup::Dns(deadline),
        )
        .map_err(ChildFailure::into_error)
        .unwrap();
        assert_eq!(completed.output().stdout, b"done\n");
        // The parent's own descriptors are untouched.
        assert_eq!(fcntl(read.as_fd(), FcntlArg::F_GETFD).unwrap(), 0);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn ordinary_child_sees_only_its_passed_descriptor() {
        let (read, write) = pipe().unwrap();
        for fd in [read.as_fd(), write.as_fd()] {
            fcntl(fd, FcntlArg::F_SETFD(FdFlag::empty())).unwrap();
        }
        let probe = format!(
            "[ -e /dev/fd/{} ] && echo leaked; [ -e /dev/fd/{} ] && echo passed; echo done",
            read.as_raw_fd(),
            write.as_raw_fd()
        );
        let request = Request {
            program: "/bin/sh".into(),
            argv: vec!["-c".into(), probe.into()],
            env: BTreeMap::new(),
            timeout: Some(Duration::from_secs(30)),
            pass_fds: vec![write.as_fd()],
        };
        let completed = execute_request(&request, Some(4096), None, ChildSetup::Ordinary)
            .map_err(ChildFailure::into_error)
            .unwrap();
        assert_eq!(completed.output().stdout, b"passed\ndone\n");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(target_os = "macos")]
    #[test]
    fn creator_child_starts_in_its_anchor_and_inherits_no_descriptor() {
        let temp = tempfile::tempdir().unwrap();
        let anchor = File::open(temp.path()).unwrap();
        fcntl(anchor.as_fd(), FcntlArg::F_SETFD(FdFlag::empty())).unwrap();
        let probe = format!(
            "[ -e /dev/fd/{} ] && echo leaked; /bin/pwd -P",
            anchor.as_raw_fd()
        );
        let request = Request {
            program: "/bin/sh".into(),
            argv: vec!["-c".into(), probe.into()],
            env: BTreeMap::new(),
            timeout: Some(Duration::from_secs(30)),
            pass_fds: Vec::new(),
        };
        let completed = run_owned_archive_creator_at(&request, anchor.as_fd(), Some(4096), None)
            .map_err(ChildFailure::into_error)
            .unwrap();
        let expected = format!("{}\n", temp.path().canonicalize().unwrap().display());
        assert_eq!(completed.output().stdout, expected.as_bytes());
        // A creator that would pass descriptors is refused before spawn.
        let mut passing = request;
        passing.pass_fds = vec![anchor.as_fd()];
        let refused = run_owned_archive_creator_at(&passing, anchor.as_fd(), Some(4096), None)
            .err()
            .expect("descriptors are refused");
        assert!(refused.settled());
    }

    /// Run in its own process: the stop is process-wide.
    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    #[ignore = "private child of process_stop_kills_and_reaps_running_children"]
    fn process_stop_child() {
        let started = Instant::now();
        let worker = std::thread::spawn(|| {
            let request = Request {
                program: "/bin/sleep".into(),
                argv: vec!["30".into()],
                env: BTreeMap::new(),
                timeout: None,
                pass_fds: Vec::new(),
            };
            execute(&request, Some(64), None)
        });
        std::thread::sleep(Duration::from_millis(300));
        crate::archive_admission::stop_archive_work();
        let failure = worker
            .join()
            .unwrap()
            .err()
            .expect("a stopped child is not a success");
        assert!(failure.settled(), "the stopped child was reaped");
        assert!(crate::archive_admission::wait_for_archive_children(
            Duration::from_secs(5)
        ));
        assert!(started.elapsed() < Duration::from_secs(10));
        // A later operation sees the stop before it launches anything.
        assert!(crate::archive_admission::ArchiveCancellation::new().is_cancelled());
        println!("PROCESS_STOP_CHILD_OK");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn process_stop_kills_and_reaps_running_children() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bounded_child::tests::process_stop_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("PROCESS_STOP_CHILD_OK"), "{stdout}");
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn live_test_session() -> SpawnedChild {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 60"]).process_group(0);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command.spawn().unwrap();
        let pgid = Pid::from_raw(child.id() as i32);
        SpawnedChild {
            child,
            pgid,
            reaped: false,
            stdout_w: None,
            stderr_w: None,
            stdout_reader: None,
            stderr_reader: None,
            reader_signals: Arc::default(),
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn reader_error_or_panic_stops_live_root_and_joins_open_sibling_in_either_order() {
        use std::sync::Barrier;
        for fault_stdout in [true, false] {
            for panics in [false, true] {
                let mut session = live_test_session();
                let (blocked_r, held_writer) = pipe_cloexec().unwrap();
                let started = Instant::now();
                let sibling = spawn_reader(
                    blocked_r,
                    None,
                    session.reader_signals.clone(),
                    started,
                    None,
                    None,
                )
                .unwrap();
                let barrier = Arc::new(Barrier::new(2));
                let fault_barrier = barrier.clone();
                let signals = session.reader_signals.clone();
                let fault = thread::spawn(move || {
                    let _guard = ReaderFailureGuard {
                        signals,
                        succeeded: false,
                    };
                    fault_barrier.wait();
                    assert!(!panics, "injected reader panic");
                    Err(io::Error::other("injected reader error"))
                });
                if fault_stdout {
                    session.stdout_reader = Some(fault);
                    session.stderr_reader = Some(sibling);
                } else {
                    session.stdout_reader = Some(sibling);
                    session.stderr_reader = Some(fault);
                }
                assert!(!session.observe_exit_without_reap().unwrap());
                barrier.wait();
                assert!(matches!(
                    await_exit(&mut session, started, Some(Duration::from_secs(5)), None).unwrap(),
                    StopReason::ReaderFailed
                ));
                assert!(complete_session(&mut session, None).is_err());
                assert!(session.reaped);
                assert!(session.stdout_reader.is_none() && session.stderr_reader.is_none());
                assert_eq!(
                    nix::sys::wait::waitpid(
                        session.pgid,
                        Some(nix::sys::wait::WaitPidFlag::WNOHANG)
                    ),
                    Err(Errno::ECHILD)
                );
                // Still open: EOF could not have released the sibling reader.
                drop(held_writer);
            }
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn drop_reaps_and_joins_when_only_first_reader_was_created() {
        let mut session = live_test_session();
        let pid = session.pgid;
        let (blocked_r, held_writer) = pipe_cloexec().unwrap();
        let signals = session.reader_signals.clone();
        let (finished, received) = std::sync::mpsc::channel();
        session.stdout_reader = Some(thread::spawn(move || {
            let bytes = read_output(blocked_r, None, &signals, Instant::now(), None, None);
            finished.send(()).unwrap();
            bytes
        }));
        drop(session);
        received.try_recv().unwrap();
        assert_eq!(
            nix::sys::wait::waitpid(pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)),
            Err(Errno::ECHILD)
        );
        drop(held_writer);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn collection_keeps_the_original_monotonic_timeout_after_root_exit() {
        let (reader, held_writer) = pipe_cloexec().unwrap();
        let started = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        let output = read_output(
            reader,
            None,
            &ReaderSignals::default(),
            started,
            Some(Duration::from_millis(100)),
            None,
        )
        .unwrap();
        assert!(output.is_empty());
        drop(held_writer);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn child_setup_failure_after_first_descriptor_leaves_parent_flags_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let first = File::create(directory.path().join("first")).unwrap();
        let second = File::create(directory.path().join("second")).unwrap();
        let second_raw = second.as_raw_fd();
        let first_flags = fcntl(first.as_fd(), FcntlArg::F_GETFD).unwrap();
        let second_flags = fcntl(second.as_fd(), FcntlArg::F_GETFD).unwrap();
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("exit 0");
        // SAFETY: this test fault closes only the child copy of a retained FD.
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(move || {
                nix::libc::close(second_raw);
                Ok(())
            });
        }
        inherit_in_child(&mut command, &[first.as_fd(), second.as_fd()]).unwrap();
        assert!(command.spawn().is_err());
        assert_eq!(
            fcntl(first.as_fd(), FcntlArg::F_GETFD).unwrap(),
            first_flags
        );
        assert_eq!(
            fcntl(second.as_fd(), FcntlArg::F_GETFD).unwrap(),
            second_flags
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn unspawned_and_cancelled_reaped_runs_report_nothing_alive() {
        let request = |program: &str, argv: &[&str]| Request {
            program: program.into(),
            argv: argv.iter().map(|value| (*value).into()).collect(),
            env: BTreeMap::new(),
            timeout: Some(Duration::from_secs(30)),
            pass_fds: Vec::new(),
        };
        let missing = run_owned(&request("/nonexistent/program", &[]), None, None)
            .err()
            .unwrap();
        assert!(missing.settled());
        let cancellation = ArchiveCancellation::new();
        let signal = cancellation.clone();
        let started = Instant::now();
        let cancelled = thread::scope(|scope| {
            scope.spawn(move || {
                thread::sleep(Duration::from_millis(200));
                signal.cancel();
            });
            run_owned(&request("/bin/sleep", &["30"]), None, Some(&cancellation))
        })
        .err()
        .unwrap();
        assert_eq!(cancelled.error.kind(), io::ErrorKind::Interrupted);
        assert!(cancelled.settled());
        assert!(started.elapsed() < Duration::from_secs(20));
    }
}
