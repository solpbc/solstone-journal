// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private installed-task capture through the sealed operational-log writer.
//! Standard-output locks, synchronous file/pipe writes and diagnostic writes are
//! blocking boundaries. The drain deadline bounds completion observation only;
//! it cannot cancel one of those synchronous operations or process teardown.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset, Local};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, OplogWriter, create_oplog_at},
};
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

const CAPTURE_FAILURE: i32 = 74;
const DRAIN_BOUND: Duration = Duration::from_secs(2);
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>;

struct ServiceCapture {
    original_stdout: HANDLE, // borrowed; never closed by this owner
    original_stderr: HANDLE,
    diagnostic: Option<Arc<Mutex<File>>>,
    writer: Option<File>,
    stdout_installed: bool,
    stderr_installed: bool,
    worker: Option<JoinHandle<Result<(), String>>>,
    previous_panic_hook: Option<PanicHook>,
}

struct CaptureIncomplete {
    capture: ServiceCapture,
    detail: String,
}

enum CaptureStartFailure {
    BeforeCapture(String),
    Incomplete(CaptureIncomplete),
}

impl CaptureIncomplete {
    fn exit(self) -> ! {
        self.capture.diagnose(&self.detail);
        // Keep the installed endpoints and actual worker owned until the OS
        // ends this process. Never close a handle still installed as stdio.
        std::process::exit(CAPTURE_FAILURE)
    }
}

impl Drop for ServiceCapture {
    fn drop(&mut self) {
        if self.stdout_installed || self.stderr_installed || self.worker.is_some() {
            self.diagnose("windows service capture owner exited before capture settled");
            // Explicit terminal fallback for unwinding/forgotten finish. This is
            // not successful cleanup and does not create a detached manager.
            std::process::exit(CAPTURE_FAILURE);
        }
    }
}

fn diagnostic_write(target: &Option<Arc<Mutex<File>>>, message: &str) {
    if let Some(target) = target {
        if let Ok(mut output) = target.lock() {
            // Original stderr only. This synchronous write is not cancellable.
            let _ = writeln!(output, "{message}");
        }
    }
}

fn borrowed_standard(which: u32) -> HANDLE {
    #[allow(unsafe_code)]
    unsafe {
        GetStdHandle(which)
    }
}

fn duplicate_diagnostic(raw: HANDLE) -> io::Result<Option<Arc<Mutex<File>>>> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        return Ok(None);
    }
    let mut copy = std::ptr::null_mut();
    // SAFETY: borrow current process's standard handle; the result is a new
    // noninheritable owned handle with exactly the existing access rights.
    #[allow(unsafe_code)]
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw,
            GetCurrentProcess(),
            &mut copy,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    #[allow(unsafe_code)]
    let file = unsafe { File::from_raw_handle(copy) };
    Ok(Some(Arc::new(Mutex::new(file))))
}

fn pipe() -> io::Result<(File, File)> {
    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    // Null SECURITY_ATTRIBUTES makes both ends noninheritable.
    #[allow(unsafe_code)]
    let ok = unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: each successful output is uniquely owned and wrapped once.
    #[allow(unsafe_code)]
    unsafe {
        Ok((File::from_raw_handle(read), File::from_raw_handle(write)))
    }
}

#[cfg(feature = "test-hooks")]
thread_local! {
    static STANDARD_FAULTS: std::cell::RefCell<(usize, Vec<usize>)> = const { std::cell::RefCell::new((0, Vec::new())) };
}

fn set_standard(which: u32, handle: HANDLE) -> io::Result<()> {
    #[cfg(feature = "test-hooks")]
    if STANDARD_FAULTS.with(|faults| {
        let mut faults = faults.borrow_mut();
        faults.0 += 1;
        faults.1.contains(&faults.0)
    }) {
        return Err(io::Error::other("injected SetStdHandle failure"));
    }

    // SAFETY: capture retains the installed writer, or restores a borrowed
    // original that capture has never closed.
    #[allow(unsafe_code)]
    if unsafe { SetStdHandle(which, handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn open_log(journal: &Path, opened: DateTime<FixedOffset>) -> Result<OplogWriter, String> {
    let root = JournalRoot::open(journal).map_err(|error| error.to_string())?;
    create_oplog_at(root, "service", "supervisor", OplogFormat::Log, opened)
        .map_err(|error| error.to_string())
}

fn drain(
    mut input: File,
    journal: PathBuf,
    mut log: OplogWriter,
    mut day: chrono::NaiveDate,
) -> Result<(), String> {
    let mut bytes = [0_u8; 8192];
    let mut rollover_error = None;
    loop {
        let count = match input.read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
            Err(error) => return Err(format!("windows service capture read failed: {error}")),
        };
        let now = Local::now().fixed_offset();
        if now.date_naive() != day {
            match open_log(&journal, now) {
                Ok(next) => {
                    log = next;
                    day = now.date_naive();
                }
                Err(error) => rollover_error = Some(error), // retain old writer and bytes
            }
        }
        log.write_all(&bytes[..count])
            .map_err(|error| format!("windows service capture write failed: {error}"))?;
    }
    log.flush()
        .map_err(|error| format!("windows service capture flush failed: {error}"))?;
    match rollover_error {
        Some(error) => Err(format!("windows service capture rollover failed: {error}")),
        None => Ok(()),
    }
}

impl ServiceCapture {
    fn diagnose(&self, message: &str) {
        diagnostic_write(&self.diagnostic, message);
    }

    fn start(journal: &Path) -> Result<Self, CaptureStartFailure> {
        let original_stdout = borrowed_standard(STD_OUTPUT_HANDLE);
        let original_stderr = borrowed_standard(STD_ERROR_HANDLE);
        let mut owner = Self {
            original_stdout,
            original_stderr,
            diagnostic: None,
            writer: None,
            stdout_installed: false,
            stderr_installed: false,
            worker: None,
            previous_panic_hook: None,
        };
        let started = (|| {
            owner.diagnostic =
                duplicate_diagnostic(original_stderr).map_err(|error| error.to_string())?;
            let opened = Local::now().fixed_offset();
            let log = open_log(journal, opened)?;
            let (read, write) = pipe().map_err(|error| error.to_string())?;
            owner.writer = Some(write);
            let journal = journal.to_path_buf();
            owner.worker = Some(
                std::thread::Builder::new()
                    .name("service-capture".into())
                    .spawn(move || drain(read, journal, log, opened.date_naive()))
                    .map_err(|error| error.to_string())?,
            );
            // These Rust locks serialize Rust writers, not private CRT fd tables.
            // Lock acquisition is synchronous; no cancellation claim is made.
            let stdout = io::stdout();
            let stderr = io::stderr();
            let _stdout = stdout.lock();
            let _stderr = stderr.lock();
            let writer = owner
                .writer
                .as_ref()
                .expect("writer constructed before mutation")
                .as_raw_handle();
            set_standard(STD_OUTPUT_HANDLE, writer).map_err(|error| error.to_string())?;
            owner.stdout_installed = true;
            set_standard(STD_ERROR_HANDLE, writer).map_err(|error| error.to_string())?;
            owner.stderr_installed = true;
            Ok::<(), String>(())
        })();
        if let Err(detail) = started {
            return match owner.finish(Instant::now() + DRAIN_BOUND) {
                Ok(()) => Err(CaptureStartFailure::BeforeCapture(detail)),
                Err(mut failure) => {
                    failure.detail = format!("{detail}; {}", failure.detail);
                    Err(CaptureStartFailure::Incomplete(failure))
                }
            };
        }
        let diagnostic = owner.diagnostic.clone();
        owner.previous_panic_hook = Some(std::panic::take_hook());
        std::panic::set_hook(Box::new(move |_| {
            diagnostic_write(&diagnostic, "windows installed supervisor panicked");
        }));
        Ok(owner)
    }

    fn finish(mut self, deadline: Instant) -> Result<(), CaptureIncomplete> {
        let restore = (|| {
            if !self.stdout_installed && !self.stderr_installed {
                return Ok(());
            }
            let stdout = io::stdout();
            let stderr = io::stderr();
            let mut stdout = stdout.lock();
            let mut stderr = stderr.lock();
            // Flush is synchronous, including backpressure from a blocked log writer.
            let flushed = stdout.flush().and_then(|()| stderr.flush());
            let mut failed = None;
            if self.stdout_installed {
                match set_standard(STD_OUTPUT_HANDLE, self.original_stdout) {
                    Ok(()) => self.stdout_installed = false,
                    Err(error) => {
                        failed = Some(error);
                    }
                }
            }
            if self.stderr_installed {
                match set_standard(STD_ERROR_HANDLE, self.original_stderr) {
                    Ok(()) => self.stderr_installed = false,
                    Err(error) => {
                        failed.get_or_insert(error);
                    }
                }
            }
            if let Some(error) = failed {
                return Err(error);
            }
            flushed
        })();
        if let Err(error) = restore {
            return Err(CaptureIncomplete {
                capture: self,
                detail: format!("windows service capture restoration failed: {error}"),
            });
        }
        self.writer.take();
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            if Instant::now() >= deadline {
                return Err(CaptureIncomplete {
                    capture: self,
                    detail: "windows service capture drain is incomplete".into(),
                });
            }
            std::thread::sleep(
                Duration::from_millis(1).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        if let Some(worker) = self.worker.take() {
            // Only join after is_finished, never turn an unbounded join into a timer.
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(detail)) => {
                    return Err(CaptureIncomplete {
                        capture: self,
                        detail,
                    });
                }
                Err(_) => {
                    return Err(CaptureIncomplete {
                        capture: self,
                        detail: "windows service capture worker panicked".into(),
                    });
                }
            }
        }
        if let Some(hook) = self.previous_panic_hook.take() {
            std::panic::set_hook(hook);
        }
        Ok(())
    }
}

pub(crate) fn run_installed(journal: &Path, run: impl FnOnce() -> ExitCode) -> ExitCode {
    let capture = match ServiceCapture::start(journal) {
        Ok(capture) => capture,
        Err(CaptureStartFailure::Incomplete(failure)) => failure.exit(),
        Err(CaptureStartFailure::BeforeCapture(detail)) => {
            // Nothing remains redirected here; original stderr is safe to use.
            eprintln!("windows service capture could not start: {detail}");
            return ExitCode::from(CAPTURE_FAILURE as u8);
        }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    if let Err(failure) = capture.finish(Instant::now() + DRAIN_BOUND) {
        failure.exit();
    }
    match result {
        Ok(code) => code,
        Err(_) => ExitCode::from(CAPTURE_FAILURE as u8),
    }
}

// Invoked only in a fresh native receipt process: stdout slots and panic hook
// are process-global and may not be mutated by concurrent test threads.
#[cfg(feature = "test-hooks")]
pub(crate) fn native_control(journal: &Path, mode: &str) -> Result<i32, String> {
    match mode {
        "normal" | "startup-refusal" | "panic" => {
            let outcome = run_installed(journal, || {
                println!("capture stdout witness");
                eprintln!("capture stderr witness");
                if mode == "panic" {
                    panic!("capture panic control");
                }
                if mode == "startup-refusal" {
                    ExitCode::from(75)
                } else {
                    ExitCode::SUCCESS
                }
            });
            let expected = match mode {
                "panic" => 74,
                "startup-refusal" => 75,
                _ => 0,
            };
            if outcome != ExitCode::from(expected) {
                return Err("capture changed the supervisor outcome".into());
            }
            Ok(i32::from(expected))
        }
        "second-install" | "rollback-failure" => {
            STANDARD_FAULTS.with(|faults| {
                *faults.borrow_mut() = (
                    0,
                    if mode == "second-install" {
                        vec![2]
                    } else {
                        vec![2, 3]
                    },
                );
            });
            match ServiceCapture::start(journal) {
                Err(CaptureStartFailure::BeforeCapture(_)) if mode == "second-install" => Ok(74),
                Err(CaptureStartFailure::Incomplete(failure)) if mode == "rollback-failure" => {
                    if !failure.capture.stdout_installed
                        || failure.capture.writer.is_none()
                        || failure.capture.worker.is_none()
                    {
                        return Err("rollback failure lost its actual capture owner".into());
                    }
                    failure
                        .capture
                        .diagnose("CAPTURE_CONTROL_rollback-failure=PASS");
                    failure.exit()
                }
                _ => Err("capture install failure control missed its boundary".into()),
            }
        }
        "restore-failure" => {
            STANDARD_FAULTS.with(|faults| *faults.borrow_mut() = (0, vec![3]));
            let capture = ServiceCapture::start(journal).map_err(|failure| match failure {
                CaptureStartFailure::BeforeCapture(detail) => detail,
                CaptureStartFailure::Incomplete(failure) => failure.exit(),
            })?;
            match capture.finish(Instant::now() + DRAIN_BOUND) {
                Err(failure) => {
                    if !failure.capture.stdout_installed
                        || failure.capture.writer.is_none()
                        || failure.capture.worker.is_none()
                    {
                        return Err("restore failure lost its actual installed owner".into());
                    }
                    failure
                        .capture
                        .diagnose("CAPTURE_CONTROL_restore-failure=PASS");
                    failure.exit()
                }
                Ok(()) => Err("restore failure was reported complete".into()),
            }
        }
        "rollover" => {
            use solstone_core_journal_io::operational_log::validate_oplog_admission;

            let yesterday = Local::now().fixed_offset() - chrono::Duration::days(1);
            let mut log = open_log(journal, yesterday)?;
            let old = journal
                .join("chronicle")
                .join(yesterday.format("%Y%m%d").to_string())
                .join("health")
                .join(log.leaf_name());
            log.write_all(b"before rollover")
                .map_err(|error| error.to_string())?;
            let (read, mut write) = pipe().map_err(|error| error.to_string())?;
            write
                .write_all(b"after rollover")
                .map_err(|error| error.to_string())?;
            drop(write);
            drain(read, journal.to_path_buf(), log, yesterday.date_naive())?;
            let old_bytes = std::fs::read(&old).map_err(|error| error.to_string())?;
            let old_admission = validate_oplog_admission(
                old.file_name().ok_or("rollover old leaf missing")?,
                &old_bytes,
            )
            .map_err(|error| error.to_string())?;
            if &old_bytes[old_admission.header_len()..] != b"before rollover" {
                return Err("rollover changed the old log".into());
            }
            let mut found = false;
            for day in
                std::fs::read_dir(journal.join("chronicle")).map_err(|error| error.to_string())?
            {
                let health = day
                    .map_err(|error| error.to_string())?
                    .path()
                    .join("health");
                for entry in std::fs::read_dir(health).map_err(|error| error.to_string())? {
                    let path = entry.map_err(|error| error.to_string())?.path();
                    if path != old && path.extension().is_some_and(|extension| extension == "log") {
                        let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
                        let admission = validate_oplog_admission(
                            path.file_name().ok_or("rollover next leaf missing")?,
                            &bytes,
                        )
                        .map_err(|error| error.to_string())?;
                        found |= &bytes[admission.header_len()..] == b"after rollover";
                    }
                }
            }
            if !found {
                return Err("rollover did not create the next canonical log".into());
            }
            Ok(0)
        }
        "withheld-writer" => {
            let capture = ServiceCapture::start(journal).map_err(|failure| match failure {
                CaptureStartFailure::BeforeCapture(detail) => detail,
                CaptureStartFailure::Incomplete(failure) => failure.exit(),
            })?;
            let held = capture
                .writer
                .as_ref()
                .ok_or("capture writer missing")?
                .try_clone()
                .map_err(|error| error.to_string())?;
            let began = Instant::now();
            let failure = match capture.finish(began + Duration::from_millis(100)) {
                Err(failure) => failure,
                Ok(()) => return Err("withheld writer falsely reported EOF".into()),
            };
            if began.elapsed() > Duration::from_secs(1)
                || failure.capture.stdout_installed
                || failure.capture.stderr_installed
                || failure.capture.worker.is_none()
            {
                return Err("withheld writer did not return the bounded original worker".into());
            }
            drop(held);
            match failure.capture.finish(Instant::now() + DRAIN_BOUND) {
                Ok(()) => Ok(0),
                Err(failure) => failure.exit(),
            }
        }
        _ => Err("unknown native capture control".into()),
    }
}
