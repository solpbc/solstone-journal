// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use solstone_core_cli::ServiceLogsArgs;
use solstone_core_system_health::{
    sanitize_for_terminal, sanitize_os_bytes_for_terminal, unsafe_ranges,
};

const TAIL: &str = "/usr/bin/tail";
const TTY_FOLLOW_REFUSAL: &str =
    "service logs: this terminal can't safely show raw follow output; use 'journal health logs -f'";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostPlatform {
    Linux,
    Darwin,
    Unsupported(&'static str),
}

impl HostPlatform {
    fn current() -> Self {
        match std::env::consts::OS {
            "linux" => Self::Linux,
            "macos" => Self::Darwin,
            other => Self::Unsupported(other),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Presence {
    Present,
    Missing,
}

trait ServiceLogFs {
    type Reader: Read;

    fn probe(&self, path: &Path) -> io::Result<Presence>;
    fn open(&self, path: &Path) -> io::Result<Self::Reader>;
}

struct StdServiceLogFs;

impl ServiceLogFs for StdServiceLogFs {
    type Reader = File;

    fn probe(&self, path: &Path) -> io::Result<Presence> {
        match std::fs::metadata(path) {
            Ok(_) => Ok(Presence::Present),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(Presence::Missing)
            }
            Err(error) => Err(error),
        }
    }

    fn open(&self, path: &Path) -> io::Result<Self::Reader> {
        File::open(path)
    }
}

trait ExecTail {
    fn exec(&self, executable: &Path, arguments: &[OsString]) -> io::Error;
}

struct StdExecTail;

impl ExecTail for StdExecTail {
    fn exec(&self, executable: &Path, arguments: &[OsString]) -> io::Error {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;

            ProcessCommand::new(executable).args(arguments).exec()
        }
        #[cfg(not(unix))]
        {
            let _ = (executable, arguments);
            io::Error::new(io::ErrorKind::Unsupported, "exec is unavailable")
        }
    }
}

pub(super) fn run(args: ServiceLogsArgs) -> ExitCode {
    let platform = HostPlatform::current();
    {
        let mut stderr = io::stderr().lock();
        if let Some(exit) = unsupported_platform(platform, &mut stderr) {
            return exit;
        }
    }
    let journal = match super::resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return super::print_journal_error(error),
    };
    run_resolved(
        args,
        &StdServiceLogFs,
        journal,
        || io::stdout().is_terminal(),
        &StdExecTail,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
}

#[cfg(test)]
struct RunOutputs<'a, O, D> {
    stdout: &'a mut O,
    stderr: &'a mut D,
}

#[cfg(test)]
fn run_with<R, F, T, E, O, D>(
    args: ServiceLogsArgs,
    platform: HostPlatform,
    resolve_journal: R,
    filesystem: &F,
    is_tty: T,
    executor: &E,
    outputs: RunOutputs<'_, O, D>,
) -> ExitCode
where
    R: FnOnce() -> Result<PathBuf, ExitCode>,
    F: ServiceLogFs,
    T: FnOnce() -> bool,
    E: ExecTail,
    O: Write,
    D: Write,
{
    if let Some(exit) = unsupported_platform(platform, outputs.stderr) {
        return exit;
    }

    let journal = match resolve_journal() {
        Ok(journal) => journal,
        Err(exit) => return exit,
    };
    run_resolved(
        args,
        filesystem,
        journal,
        is_tty,
        executor,
        outputs.stdout,
        outputs.stderr,
    )
}

fn unsupported_platform(platform: HostPlatform, stderr: &mut impl Write) -> Option<ExitCode> {
    let HostPlatform::Unsupported(platform) = platform else {
        return None;
    };
    let message = format!(
        "Error: unsupported platform '{}'\n",
        sanitize_for_terminal(platform)
    );
    let _ = stderr.write_all(message.as_bytes());
    Some(ExitCode::FAILURE)
}

fn run_resolved<F, T, E, O, D>(
    args: ServiceLogsArgs,
    filesystem: &F,
    journal: PathBuf,
    is_tty: T,
    executor: &E,
    stdout: &mut O,
    stderr: &mut D,
) -> ExitCode
where
    F: ServiceLogFs,
    T: FnOnce() -> bool,
    E: ExecTail,
    O: Write,
    D: Write,
{
    let service_log = journal.join("health").join("service.log");
    let presence = match filesystem.probe(&service_log) {
        Ok(presence) => presence,
        Err(source) => {
            return write_failure(
                stderr,
                SafeDiagnostic::path_source("metadata", &service_log, &source),
            );
        }
    };

    match (args.follow, presence) {
        (false, Presence::Missing) => {
            match stdout.write_all(b"=== service.log === (not found)\n") {
                Ok(()) => ExitCode::SUCCESS,
                Err(source) => write_failure(stderr, SafeDiagnostic::source("stdout", &source)),
            }
        }
        (true, Presence::Missing) => {
            let _ = stderr.write_all(b"No service log file found\n");
            ExitCode::FAILURE
        }
        (false, Presence::Present) => {
            run_one_shot(filesystem, &service_log, is_tty(), stdout, stderr)
        }
        (true, Presence::Present) => run_follow(&service_log, is_tty, executor, stdout, stderr),
    }
}

fn run_one_shot<F, O, D>(
    filesystem: &F,
    service_log: &Path,
    is_tty: bool,
    stdout: &mut O,
    stderr: &mut D,
) -> ExitCode
where
    F: ServiceLogFs,
    O: Write,
    D: Write,
{
    let mut reader = match filesystem.open(service_log) {
        Ok(reader) => reader,
        Err(source) => {
            return write_failure(
                stderr,
                SafeDiagnostic::path_source("open", service_log, &source),
            );
        }
    };
    let mut bytes = Vec::new();
    if let Err(source) = reader.read_to_end(&mut bytes) {
        return write_failure(
            stderr,
            SafeDiagnostic::path_source("read", service_log, &source),
        );
    }

    let decoded = String::from_utf8_lossy(&bytes);
    let normalized = decoded.replace("\r\n", "\n").replace('\r', "\n");
    let tail = final_codepoints(&normalized, 10_000);
    let body = if is_tty {
        sanitize_preserving_lf(tail)
    } else {
        tail.to_owned()
    };
    let mut staged = Vec::with_capacity("=== service.log ===\n".len() + body.len() + 1);
    staged.extend_from_slice(b"=== service.log ===\n");
    staged.extend_from_slice(body.as_bytes());
    staged.push(b'\n');
    if let Err(source) = stdout.write_all(&staged) {
        return write_failure(stderr, SafeDiagnostic::source("stdout", &source));
    }
    ExitCode::SUCCESS
}

fn run_follow<T, E, O, D>(
    service_log: &Path,
    is_tty: T,
    executor: &E,
    _stdout: &mut O,
    stderr: &mut D,
) -> ExitCode
where
    T: FnOnce() -> bool,
    E: ExecTail,
    O: Write,
    D: Write,
{
    if let Err(message) = validate_follow_path(service_log) {
        return write_failure(stderr, message);
    }
    if is_tty() {
        let _ = writeln!(stderr, "{TTY_FOLLOW_REFUSAL}");
        return ExitCode::FAILURE;
    }

    let arguments = [
        OsString::from("-f"),
        OsString::from("--"),
        service_log.as_os_str().to_owned(),
    ];
    let source = executor.exec(Path::new(TAIL), &arguments);
    write_failure(
        stderr,
        SafeDiagnostic::source("exec /usr/bin/tail", &source),
    )
}

fn final_codepoints(value: &str, count: usize) -> &str {
    value
        .char_indices()
        .rev()
        .nth(count.saturating_sub(1))
        .map_or(value, |(offset, _)| &value[offset..])
}

fn sanitize_preserving_lf(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut start = 0;
    for (offset, scalar) in value.char_indices() {
        if scalar == '\n' {
            output.push_str(&sanitize_for_terminal(&value[start..offset]));
            output.push('\n');
            start = offset + 1;
        }
    }
    output.push_str(&sanitize_for_terminal(&value[start..]));
    output
}

fn validate_follow_path(path: &Path) -> Result<(), SafeDiagnostic> {
    let Some(value) = path.to_str() else {
        return Err(SafeDiagnostic::path("unsafe follow path", path));
    };
    if value.chars().any(|scalar| {
        unsafe_ranges()
            .iter()
            .any(|(start, end)| (*start..=*end).contains(&(scalar as u32)))
    }) {
        return Err(SafeDiagnostic::path("unsafe follow path", path));
    }
    Ok(())
}

fn write_failure(stderr: &mut impl Write, message: SafeDiagnostic) -> ExitCode {
    let _ = writeln!(stderr, "service logs: {}", message.0);
    ExitCode::FAILURE
}

struct SafeDiagnostic(String);

impl SafeDiagnostic {
    fn path(operation: &str, path: &Path) -> Self {
        Self(format!("{operation}: {}", terminal_path(path)))
    }

    fn path_source(operation: &str, path: &Path, source: &dyn std::fmt::Display) -> Self {
        Self(format!(
            "{operation} failed for {}: {}",
            terminal_path(path),
            sanitize_for_terminal(&source.to_string())
        ))
    }

    fn source(operation: &str, source: &dyn std::fmt::Display) -> Self {
        Self(format!(
            "{operation} failed: {}",
            sanitize_for_terminal(&source.to_string())
        ))
    }
}

fn terminal_path(path: &Path) -> String {
    sanitize_os_bytes_for_terminal(path.as_os_str().as_encoded_bytes())
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::rc::Rc;

    use super::*;

    struct FakeFs {
        presence: io::Result<Presence>,
        open: RefCell<Option<io::Result<TrackedReader>>>,
        probes: Cell<usize>,
        opens: Cell<usize>,
    }

    impl FakeFs {
        fn with_bytes(bytes: Vec<u8>) -> Self {
            Self {
                presence: Ok(Presence::Present),
                open: RefCell::new(Some(Ok(TrackedReader::new(bytes)))),
                probes: Cell::new(0),
                opens: Cell::new(0),
            }
        }

        fn missing() -> Self {
            Self {
                presence: Ok(Presence::Missing),
                open: RefCell::new(None),
                probes: Cell::new(0),
                opens: Cell::new(0),
            }
        }

        fn metadata_error() -> Self {
            Self {
                presence: Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "metadata\\\n\x1b\u{202e}",
                )),
                open: RefCell::new(None),
                probes: Cell::new(0),
                opens: Cell::new(0),
            }
        }

        fn open_error() -> Self {
            Self {
                presence: Ok(Presence::Present),
                open: RefCell::new(Some(Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "open\\\n\x1b\u{202e}",
                )))),
                probes: Cell::new(0),
                opens: Cell::new(0),
            }
        }
    }

    impl ServiceLogFs for FakeFs {
        type Reader = TrackedReader;

        fn probe(&self, _path: &Path) -> io::Result<Presence> {
            self.probes.set(self.probes.get() + 1);
            self.presence
                .as_ref()
                .copied()
                .map_err(|error| io::Error::new(error.kind(), error.to_string()))
        }

        fn open(&self, _path: &Path) -> io::Result<Self::Reader> {
            self.opens.set(self.opens.get() + 1);
            self.open.borrow_mut().take().expect("one open")
        }
    }

    struct TrackedReader {
        cursor: Cursor<Vec<u8>>,
        drops: Rc<Cell<usize>>,
        fail_after: Option<usize>,
    }

    impl TrackedReader {
        fn new(bytes: Vec<u8>) -> Self {
            Self {
                cursor: Cursor::new(bytes),
                drops: Rc::new(Cell::new(0)),
                fail_after: None,
            }
        }
    }

    impl Read for TrackedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self
                .fail_after
                .is_some_and(|limit| self.cursor.position() as usize >= limit)
            {
                return Err(io::Error::other("read\\\n\x1b\u{202e}"));
            }
            self.cursor.read(buffer)
        }
    }

    impl Drop for TrackedReader {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[derive(Default)]
    struct FakeExec {
        calls: RefCell<Vec<(PathBuf, Vec<OsString>)>>,
    }

    impl ExecTail for FakeExec {
        fn exec(&self, executable: &Path, arguments: &[OsString]) -> io::Error {
            self.calls
                .borrow_mut()
                .push((executable.to_owned(), arguments.to_vec()));
            io::Error::other("exec\\\n\x1b\u{202e}")
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("stdout\\\n\x1b\u{202e}"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn invoke(
        args: ServiceLogsArgs,
        platform: HostPlatform,
        journal: PathBuf,
        filesystem: &FakeFs,
        is_tty: bool,
        executor: &FakeExec,
    ) -> (ExitCode, Vec<u8>, Vec<u8>) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_with(
            args,
            platform,
            || Ok(journal),
            filesystem,
            || is_tty,
            executor,
            RunOutputs {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        (exit, stdout, stderr)
    }

    #[test]
    fn unsupported_platform_precedes_resolver_and_every_body_capability() {
        let resolved = Cell::new(false);
        let filesystem = FakeFs::with_bytes(b"hidden".to_vec());
        let executor = FakeExec::default();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_with(
            ServiceLogsArgs { follow: true },
            HostPlatform::Unsupported("host\\\n\x1b"),
            || {
                resolved.set(true);
                Ok(PathBuf::from("/journal"))
            },
            &filesystem,
            || false,
            &executor,
            RunOutputs {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        assert_eq!(exit, ExitCode::FAILURE);
        assert!(!resolved.get());
        assert_eq!(filesystem.probes.get(), 0);
        assert_eq!(filesystem.opens.get(), 0);
        assert!(executor.calls.borrow().is_empty());
        assert!(stdout.is_empty());
        assert_eq!(stderr, b"Error: unsupported platform 'host\\\\\\n\\x1b'\n");
    }

    #[test]
    fn missing_outputs_are_exact_and_never_open_or_exec() {
        let filesystem = FakeFs::missing();
        let executor = FakeExec::default();
        let (_, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: false },
            HostPlatform::Linux,
            PathBuf::from("/journal"),
            &filesystem,
            false,
            &executor,
        );
        assert_eq!(stdout, b"=== service.log === (not found)\n");
        assert!(stderr.is_empty());

        let (_, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: true },
            HostPlatform::Darwin,
            PathBuf::from("/journal"),
            &filesystem,
            false,
            &executor,
        );
        assert!(stdout.is_empty());
        assert_eq!(stderr, b"No service log file found\n");
        assert_eq!(filesystem.opens.get(), 0);
        assert!(executor.calls.borrow().is_empty());
    }

    #[test]
    fn missing_one_shot_stdout_failure_is_visible_and_not_success() {
        let filesystem = FakeFs::missing();
        let executor = FakeExec::default();
        let mut stdout = FailingWriter;
        let mut stderr = Vec::new();
        let exit = run_with(
            ServiceLogsArgs { follow: false },
            HostPlatform::Linux,
            || Ok(PathBuf::from("/journal")),
            &filesystem,
            || false,
            &executor,
            RunOutputs {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        assert_eq!(exit, ExitCode::FAILURE);
        assert_eq!(
            stderr,
            b"service logs: stdout failed: stdout\\\\\\n\\x1b\\u{202e}\n"
        );
        assert_eq!(filesystem.opens.get(), 0);
        assert!(executor.calls.borrow().is_empty());
    }

    #[test]
    fn missing_and_unsafe_follow_paths_precede_tty_observation() {
        let executor = FakeExec::default();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_with(
            ServiceLogsArgs { follow: true },
            HostPlatform::Linux,
            || Ok(PathBuf::from("bad\n\x1b\u{202e}")),
            &FakeFs::missing(),
            || panic!("missing path must not observe TTY state"),
            &executor,
            RunOutputs {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        assert_eq!(exit, ExitCode::FAILURE);
        assert_eq!(stderr, b"No service log file found\n");

        stdout.clear();
        stderr.clear();
        let exit = run_with(
            ServiceLogsArgs { follow: true },
            HostPlatform::Linux,
            || Ok(PathBuf::from("bad\n\x1b\u{202e}")),
            &FakeFs::with_bytes(Vec::new()),
            || panic!("unsafe path must fail before TTY state"),
            &executor,
            RunOutputs {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        assert_eq!(exit, ExitCode::FAILURE);
        assert!(stderr.starts_with(b"service logs: unsafe follow path:"));
        assert!(executor.calls.borrow().is_empty());
    }

    #[test]
    fn metadata_and_open_failures_are_visible_without_partial_stdout() {
        for (filesystem, operation) in [
            (FakeFs::metadata_error(), "metadata"),
            (FakeFs::open_error(), "open"),
        ] {
            let executor = FakeExec::default();
            let (exit, stdout, stderr) = invoke(
                ServiceLogsArgs { follow: false },
                HostPlatform::Linux,
                PathBuf::from("/journal\\\n\x1b\u{202e}"),
                &filesystem,
                false,
                &executor,
            );
            assert_eq!(exit, ExitCode::FAILURE);
            assert!(stdout.is_empty());
            let stderr = String::from_utf8(stderr).unwrap();
            assert!(stderr.starts_with(&format!("service logs: {operation} failed for ")));
            assert_eq!(stderr.lines().count(), 1);
            assert!(stderr.contains("/journal\\\\\\n\\x1b\\u{202e}/health/service.log"));
            assert!(executor.calls.borrow().is_empty());
        }
    }

    #[test]
    fn read_failure_closes_once_and_never_leaks_the_staged_header() {
        let drops = Rc::new(Cell::new(0));
        let reader = TrackedReader {
            cursor: Cursor::new(vec![b'x'; 128]),
            drops: drops.clone(),
            fail_after: Some(1),
        };
        let filesystem = FakeFs {
            presence: Ok(Presence::Present),
            open: RefCell::new(Some(Ok(reader))),
            probes: Cell::new(0),
            opens: Cell::new(0),
        };
        let executor = FakeExec::default();
        let (exit, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: false },
            HostPlatform::Linux,
            PathBuf::from("/journal"),
            &filesystem,
            false,
            &executor,
        );
        assert_eq!(exit, ExitCode::FAILURE);
        assert!(stdout.is_empty());
        assert_eq!(drops.get(), 1);
        assert_eq!(
            stderr,
            b"service logs: read failed for /journal/health/service.log: read\\\\\\n\\x1b\\u{202e}\n"
        );
    }

    #[test]
    fn one_shot_replaces_utf8_normalizes_newlines_and_slices_codepoints() {
        let mut bytes = "x".repeat(9_998).into_bytes();
        bytes.extend_from_slice("界".as_bytes());
        bytes.push(0xff);
        bytes.extend_from_slice(b"\r\nend\r");
        let drops = Rc::new(Cell::new(0));
        let filesystem = FakeFs {
            presence: Ok(Presence::Present),
            open: RefCell::new(Some(Ok(TrackedReader {
                cursor: Cursor::new(bytes),
                drops: drops.clone(),
                fail_after: None,
            }))),
            probes: Cell::new(0),
            opens: Cell::new(0),
        };
        let executor = FakeExec::default();
        let (_, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: false },
            HostPlatform::Linux,
            PathBuf::from("/journal"),
            &filesystem,
            false,
            &executor,
        );
        assert!(stderr.is_empty());
        let body = String::from_utf8(stdout).unwrap();
        assert!(body.starts_with("=== service.log ===\n"));
        let payload = body.strip_prefix("=== service.log ===\n").unwrap();
        assert_eq!(payload.chars().count(), 10_001);
        assert!(payload.ends_with("界�\nend\n\n"));
        assert_eq!(drops.get(), 1, "successful read must close exactly once");
    }

    #[test]
    fn tty_one_shot_sanitizes_non_lf_runs_once_and_keeps_structural_lf() {
        let filesystem = FakeFs::with_bytes(b"ordinary\\\x1b\r\nnext\x00\n".to_vec());
        let executor = FakeExec::default();
        let (_, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: false },
            HostPlatform::Linux,
            PathBuf::from("/journal"),
            &filesystem,
            true,
            &executor,
        );
        assert!(stderr.is_empty());
        assert_eq!(
            stdout,
            b"=== service.log ===\nordinary\\\\\\x1b\nnext\\u{0}\n\n"
        );
    }

    #[test]
    fn tty_one_shot_routes_the_complete_unsafe_union_through_the_shared_sanitizer() {
        let mut input = String::from("ordinary\\");
        for (start, end) in unsafe_ranges() {
            for scalar in *start..=*end {
                let value = char::from_u32(scalar).unwrap();
                if value != '\n' {
                    input.push(value);
                }
            }
        }
        input.push('\n');
        let filesystem = FakeFs::with_bytes(input.as_bytes().to_vec());
        let executor = FakeExec::default();
        let (_, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: false },
            HostPlatform::Linux,
            PathBuf::from("/journal"),
            &filesystem,
            true,
            &executor,
        );
        assert!(stderr.is_empty());
        let rendered = String::from_utf8(stdout).unwrap();
        assert!(rendered.contains("ordinary\\\\"));
        for scalar in rendered.chars() {
            if scalar == '\n' {
                continue;
            }
            assert!(
                !unsafe_ranges()
                    .iter()
                    .any(|(start, end)| (*start..=*end).contains(&(scalar as u32))),
                "raw unsafe scalar U+{:04X}",
                scalar as u32
            );
        }
    }

    #[test]
    fn follow_tty_refusal_and_non_tty_exec_are_opposite_twins() {
        let filesystem = FakeFs::with_bytes(Vec::new());
        let executor = FakeExec::default();
        let (_, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: true },
            HostPlatform::Linux,
            PathBuf::from("/journal"),
            &filesystem,
            true,
            &executor,
        );
        assert!(stdout.is_empty());
        assert_eq!(stderr, format!("{TTY_FOLLOW_REFUSAL}\n").as_bytes());
        assert!(executor.calls.borrow().is_empty());

        let (_, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: true },
            HostPlatform::Linux,
            PathBuf::from("relative-journal"),
            &filesystem,
            false,
            &executor,
        );
        assert!(stdout.is_empty());
        assert_eq!(
            stderr,
            b"service logs: exec /usr/bin/tail failed: exec\\\\\\n\\x1b\\u{202e}\n"
        );
        assert_eq!(
            executor.calls.borrow().as_slice(),
            &[(
                PathBuf::from(TAIL),
                vec![
                    OsString::from("-f"),
                    OsString::from("--"),
                    OsString::from("relative-journal/health/service.log"),
                ],
            )]
        );
    }

    #[test]
    fn follow_unsafe_paths_fail_after_presence_and_before_tty_or_exec() {
        let filesystem = FakeFs::with_bytes(Vec::new());
        let executor = FakeExec::default();
        let (_, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: true },
            HostPlatform::Linux,
            PathBuf::from("bad\n\x1b\u{202e}"),
            &filesystem,
            false,
            &executor,
        );
        assert!(stdout.is_empty());
        assert_eq!(
            stderr,
            b"service logs: unsafe follow path: bad\\n\\x1b\\u{202e}/health/service.log\n"
        );
        assert_eq!(filesystem.probes.get(), 1);
        assert!(executor.calls.borrow().is_empty());
    }

    #[test]
    fn final_codepoint_boundaries_cover_empty_exact_and_over_limit() {
        assert_eq!(final_codepoints("", 10_000), "");
        assert_eq!(
            final_codepoints(&"界".repeat(9_999), 10_000)
                .chars()
                .count(),
            9_999
        );
        assert_eq!(
            final_codepoints(&"界".repeat(10_000), 10_000)
                .chars()
                .count(),
            10_000
        );
        assert_eq!(
            final_codepoints(&("a".repeat(10_000) + "界"), 10_000),
            "a".repeat(9_999) + "界"
        );
    }

    #[test]
    fn replacement_before_at_and_after_the_cut_has_exact_order() {
        let cases = [
            (
                [vec![0xff], vec![b'a'; 10_001]].concat(),
                "a".repeat(10_000),
            ),
            (
                [vec![0xff], vec![b'a'; 9_999]].concat(),
                format!("�{}", "a".repeat(9_999)),
            ),
            (
                [vec![b'a'; 9_999], vec![0xff]].concat(),
                format!("{}�", "a".repeat(9_999)),
            ),
        ];
        for (bytes, expected) in cases {
            let decoded = String::from_utf8_lossy(&bytes);
            assert_eq!(final_codepoints(&decoded, 10_000), expected);
        }
    }

    #[test]
    fn unsafe_table_is_exhaustive_for_follow_and_printable_neighbors_pass() {
        let mut cases = VecDeque::new();
        for (start, end) in unsafe_ranges() {
            for scalar in *start..=*end {
                cases.push_back(scalar);
            }
        }
        for scalar in cases {
            let value = char::from_u32(scalar).unwrap();
            assert!(validate_follow_path(Path::new(&format!("x{value}y"))).is_err());
        }
        for (start, end) in unsafe_ranges() {
            for neighbor in [start.checked_sub(1), end.checked_add(1)]
                .into_iter()
                .flatten()
            {
                let Some(value) = char::from_u32(neighbor) else {
                    continue;
                };
                let still_unsafe = unsafe_ranges()
                    .iter()
                    .any(|(left, right)| (*left..=*right).contains(&neighbor));
                if !still_unsafe {
                    assert!(validate_follow_path(Path::new(&format!("x{value}y"))).is_ok());
                }
            }
        }
        for scalar in [' ', '\\', 'é', '界', '\u{1f642}'] {
            assert!(validate_follow_path(Path::new(&format!("x{scalar}y"))).is_ok());
        }
    }

    #[cfg(unix)]
    #[test]
    fn invalid_path_bytes_are_reversible_and_never_exec() {
        use std::os::unix::ffi::OsStringExt as _;

        let filesystem = FakeFs::with_bytes(Vec::new());
        let executor = FakeExec::default();
        let journal = PathBuf::from(OsString::from_vec(b"bad\\\xff".to_vec()));
        let (_, stdout, stderr) = invoke(
            ServiceLogsArgs { follow: true },
            HostPlatform::Linux,
            journal,
            &filesystem,
            false,
            &executor,
        );
        assert!(stdout.is_empty());
        assert_eq!(
            stderr,
            b"service logs: unsafe follow path: bad\\\\\\xff/health/service.log\n"
        );
        assert!(executor.calls.borrow().is_empty());
    }
}
