// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::{
    GenerateRequest, GenerateResponse, ProtocolError, decode_one_shot_response,
    decode_protocol_error, encode_one_shot_request,
};

pub const STDOUT_LIMIT: usize = 1_048_576;
pub const STDERR_LIMIT: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildStatus {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

impl fmt::Display for ChildStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.exit_code, self.signal) {
            (Some(code), Some(signal)) => write!(formatter, "exit {code} signal {signal}"),
            (Some(code), None) => write!(formatter, "exit {code}"),
            (None, Some(signal)) => write!(formatter, "signal {signal}"),
            (None, None) => write!(formatter, "status unknown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedStream {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

impl CapturedStream {
    pub fn empty() -> Self {
        Self {
            bytes: Vec::new(),
            truncated: false,
        }
    }
}

impl fmt::Display for CapturedStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &byte in &self.bytes {
            match byte {
                b'\\' => formatter.write_str("\\\\")?,
                b'\n' => formatter.write_str("\\n")?,
                b'\r' => formatter.write_str("\\r")?,
                b'\t' => formatter.write_str("\\t")?,
                0x20..=0x7e => write!(formatter, "{}", byte as char)?,
                _ => write!(formatter, "\\x{byte:02x}")?,
            }
        }
        if self.truncated {
            formatter.write_str(" [truncated]")?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ProtocolFailure {
    pub error: ProtocolError,
    pub status: ChildStatus,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
    pub stdin_closed_early: bool,
}

#[derive(Debug)]
pub struct UnexpectedChildFailure {
    pub status: ChildStatus,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
    pub stdin_closed_early: bool,
}

#[derive(Debug)]
pub struct InvalidResponseFailure {
    pub detail: String,
    pub status: ChildStatus,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
    pub stdin_closed_early: bool,
}

#[derive(Debug)]
pub struct ProcessFailure {
    pub primary: String,
    pub cleanup: Vec<String>,
    pub status: Option<ChildStatus>,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
    pub stdin_closed_early: bool,
}

#[derive(Debug)]
pub enum ClientError {
    Resolve(String),
    Io {
        primary: String,
        cleanup: Option<String>,
    },
    Protocol(Box<ProtocolFailure>),
    UnexpectedChild(Box<UnexpectedChildFailure>),
    InvalidResponse(Box<InvalidResponseFailure>),
    ProcessIo(Box<ProcessFailure>),
    Decode(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolve(detail) | Self::Decode(detail) => formatter.write_str(detail),
            Self::Io {
                primary,
                cleanup: None,
            } => formatter.write_str(primary),
            Self::Io {
                primary,
                cleanup: Some(cleanup),
            } => write!(formatter, "{primary} (cleanup: {cleanup})"),
            Self::Protocol(failure) => write!(
                formatter,
                "protocol {} ({}): {}; stdin_closed_early={}; stdout={}; stderr={}",
                failure.error.reason,
                failure.status,
                failure.error.detail,
                failure.stdin_closed_early,
                failure.stdout,
                failure.stderr
            ),
            Self::UnexpectedChild(failure) => {
                write!(
                    formatter,
                    "unexpected child ({}); stdin_closed_early={}; stdout={}; stderr={}",
                    failure.status, failure.stdin_closed_early, failure.stdout, failure.stderr
                )
            }
            Self::InvalidResponse(failure) => write!(
                formatter,
                "invalid child response ({}): {}; stdin_closed_early={}; stdout={}; stderr={}",
                failure.status,
                failure.detail,
                failure.stdin_closed_early,
                failure.stdout,
                failure.stderr
            ),
            Self::ProcessIo(failure) => {
                write!(
                    formatter,
                    "process I/O failed: {}; status={}; stdin_closed_early={}; stdout={}; stderr={}",
                    failure.primary,
                    failure
                        .status
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "unavailable".to_owned()),
                    failure.stdin_closed_early,
                    failure.stdout,
                    failure.stderr
                )?;
                if !failure.cleanup.is_empty() {
                    write!(formatter, "; cleanup={}", failure.cleanup.join("; "))?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ClientError {}

#[derive(Clone)]
pub struct OneShotClient {
    executable: PathBuf,
    prefix_arguments: Vec<OsString>,
    environment: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq, Eq)]
enum StdinWrite {
    Complete,
    BrokenPipe,
}

struct PipeWorkers {
    receiver: Receiver<PipeCompletion>,
    handles: Vec<(&'static str, JoinHandle<()>)>,
}

struct StreamCollection {
    captured: CapturedStream,
    error: Option<String>,
}

enum PipeCompletion {
    Stdin(Result<StdinWrite, String>),
    Stdout(StreamCollection),
    Stderr(StreamCollection),
}

struct PipeReport {
    stdin_write: Option<StdinWrite>,
    stdin_closed_early: bool,
    stdout: CapturedStream,
    stderr: CapturedStream,
    failures: Vec<String>,
    status: Option<ChildStatus>,
}

struct ChildGuard {
    child: Child,
    status: Option<ChildStatus>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self {
            child,
            status: None,
        }
    }
}

trait ChildControl {
    fn try_wait_status(&mut self) -> io::Result<Option<ChildStatus>>;
    fn terminate(&mut self) -> io::Result<()>;
    fn wait_status(&mut self) -> io::Result<ChildStatus>;
}

impl ChildControl for ChildGuard {
    fn try_wait_status(&mut self) -> io::Result<Option<ChildStatus>> {
        if let Some(status) = &self.status {
            return Ok(Some(status.clone()));
        }
        let status = self.child.try_wait()?.map(|exit| child_status(&exit));
        if let Some(status) = &status {
            self.status = Some(status.clone());
        }
        Ok(status)
    }

    fn terminate(&mut self) -> io::Result<()> {
        self.child.kill()
    }

    fn wait_status(&mut self) -> io::Result<ChildStatus> {
        if let Some(status) = &self.status {
            return Ok(status.clone());
        }
        let status = child_status(&self.child.wait()?);
        self.status = Some(status.clone());
        Ok(status)
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn resolve_sibling_executable(current: &Path) -> Result<PathBuf, ClientError> {
    let parent = current
        .parent()
        .ok_or_else(|| ClientError::Resolve("current executable has no parent".to_owned()))?;
    let path = parent.join("solstone-core");
    if Path::new(&path).is_file() {
        Ok(path)
    } else {
        Err(ClientError::Resolve(format!(
            "missing sibling executable {}",
            path.display()
        )))
    }
}

pub fn sibling_executable() -> Result<PathBuf, ClientError> {
    let current = env::current_exe().map_err(|error| ClientError::Resolve(error.to_string()))?;
    resolve_sibling_executable(&current)
}

fn collect_bounded<R: Read>(mut reader: R, limit: usize) -> StreamCollection {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8_192];
    let mut truncated = false;
    let mut error = None;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(read_error) => {
                error = Some(read_error.to_string());
                break;
            }
        };
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        if truncated {
            continue;
        }
        let remaining = limit.saturating_sub(bytes.len());
        if chunk.len() <= remaining {
            bytes.extend_from_slice(chunk);
        } else {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
        }
    }
    StreamCollection {
        captured: CapturedStream { bytes, truncated },
        error,
    }
}

fn write_request<W: Write>(mut writer: W, bytes: &[u8]) -> io::Result<StdinWrite> {
    match writer.write_all(bytes).and_then(|()| writer.flush()) {
        Ok(()) => Ok(StdinWrite::Complete),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(StdinWrite::BrokenPipe),
        Err(error) => Err(error),
    }
}

fn child_status(status: &ExitStatus) -> ChildStatus {
    ChildStatus {
        exit_code: status.code(),
        signal: {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                status.signal()
            }
            #[cfg(not(unix))]
            {
                None
            }
        },
    }
}

fn try_protocol(stderr: &CapturedStream) -> Option<ProtocolError> {
    if stderr.truncated {
        return None;
    }
    let text = std::str::from_utf8(&stderr.bytes).ok()?;
    decode_protocol_error(text).ok()
}

fn classify(
    status: ChildStatus,
    stdout: CapturedStream,
    stderr: CapturedStream,
    stdin_write: StdinWrite,
) -> Result<GenerateResponse, ClientError> {
    let stdin_closed_early = stdin_write == StdinWrite::BrokenPipe;
    if status.exit_code == Some(0) {
        let decoded = if stdout.truncated {
            Err(format!("stdout truncated at {STDOUT_LIMIT} bytes"))
        } else {
            std::str::from_utf8(&stdout.bytes)
                .map_err(|error| error.to_string())
                .and_then(decode_one_shot_response)
        };
        match decoded {
            Ok(response) => Ok(response),
            Err(detail) => Err(ClientError::InvalidResponse(Box::new(
                InvalidResponseFailure {
                    detail,
                    status,
                    stdout,
                    stderr,
                    stdin_closed_early,
                },
            ))),
        }
    } else if let Some(error) = try_protocol(&stderr) {
        Err(ClientError::Protocol(Box::new(ProtocolFailure {
            error,
            status,
            stdout,
            stderr,
            stdin_closed_early,
        })))
    } else {
        Err(ClientError::UnexpectedChild(Box::new(
            UnexpectedChildFailure {
                status,
                stdout,
                stderr,
                stdin_closed_early,
            },
        )))
    }
}

fn terminate_if_running<C: ChildControl>(
    child: &mut C,
    status: &mut Option<ChildStatus>,
) -> Vec<String> {
    let mut failures = Vec::new();
    match child.try_wait_status() {
        Ok(Some(exit)) => *status = Some(exit),
        Ok(None) => {
            if let Err(error) = child.terminate() {
                failures.push(format!("terminate child: {error}"));
            }
        }
        Err(error) => {
            failures.push(format!("inspect child before termination: {error}"));
            if let Err(error) = child.terminate() {
                failures.push(format!("terminate child: {error}"));
            }
        }
    }
    failures
}

fn process_failure_without_workers<C: ChildControl>(child: &mut C, primary: String) -> ClientError {
    let mut status = None;
    let mut cleanup = terminate_if_running(child, &mut status);
    if status.is_none() {
        match child.wait_status() {
            Ok(exit) => status = Some(exit),
            Err(error) => {
                cleanup.push(format!("reap child: {error}"));
                cleanup.extend(terminate_if_running(child, &mut status));
                if status.is_none() {
                    match child.wait_status() {
                        Ok(exit) => status = Some(exit),
                        Err(error) => cleanup.push(format!("retry reap child: {error}")),
                    }
                }
            }
        }
    }
    ClientError::ProcessIo(Box::new(ProcessFailure {
        primary,
        cleanup,
        status,
        stdout: CapturedStream::empty(),
        stderr: CapturedStream::empty(),
        stdin_closed_early: false,
    }))
}

impl PipeWorkers {
    fn spawn(
        stdin: std::process::ChildStdin,
        stdout: std::process::ChildStdout,
        stderr: std::process::ChildStderr,
        input: Vec<u8>,
    ) -> Result<Self, (String, Self)> {
        Self::spawn_tasks(
            move || write_request(stdin, &input),
            move || collect_bounded(stdout, STDOUT_LIMIT),
            move || collect_bounded(stderr, STDERR_LIMIT),
        )
    }

    fn spawn_tasks<SI, SO, SE>(stdin: SI, stdout: SO, stderr: SE) -> Result<Self, (String, Self)>
    where
        SI: FnOnce() -> io::Result<StdinWrite> + Send + 'static,
        SO: FnOnce() -> StreamCollection + Send + 'static,
        SE: FnOnce() -> StreamCollection + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let mut workers = Self {
            receiver,
            handles: Vec::new(),
        };
        match spawn_stdin_worker(sender.clone(), stdin) {
            Ok(handle) => workers.handles.push(("stdin", handle)),
            Err(error) => return Err((format!("start stdin worker: {error}"), workers)),
        }
        match spawn_stream_worker(sender.clone(), stdout, PipeCompletion::Stdout, "stdout") {
            Ok(handle) => workers.handles.push(("stdout", handle)),
            Err(error) => return Err((format!("start stdout worker: {error}"), workers)),
        }
        match spawn_stream_worker(sender, stderr, PipeCompletion::Stderr, "stderr") {
            Ok(handle) => workers.handles.push(("stderr", handle)),
            Err(error) => return Err((format!("start stderr worker: {error}"), workers)),
        }
        Ok(workers)
    }

    fn collect<C: ChildControl>(
        mut self,
        child: &mut C,
        initial_failure: Option<String>,
    ) -> PipeReport {
        let mut report = PipeReport::new();
        let mut terminated = initial_failure.is_some();
        if let Some(failure) = initial_failure {
            report.failures.push(failure);
            report
                .failures
                .extend(terminate_if_running(child, &mut report.status));
        }
        let expected = self.handles.len();
        for _ in 0..expected {
            let failure = match self.receiver.recv() {
                Ok(completion) => report.apply(completion, terminated),
                Err(error) => Some(format!("pipe worker completion channel: {error}")),
            };
            if let Some(failure) = failure {
                report.failures.push(failure);
                if !terminated {
                    report
                        .failures
                        .extend(terminate_if_running(child, &mut report.status));
                    terminated = true;
                }
            }
        }
        for (name, handle) in self.handles.drain(..) {
            if handle.join().is_err() {
                report
                    .failures
                    .push(format!("{name} worker join: thread panicked"));
            }
        }
        if report.status.is_none() {
            match child.wait_status() {
                Ok(exit) => report.status = Some(exit),
                Err(error) => {
                    report.failures.push(format!("reap child: {error}"));
                    report
                        .failures
                        .extend(terminate_if_running(child, &mut report.status));
                    if report.status.is_none() {
                        match child.wait_status() {
                            Ok(exit) => report.status = Some(exit),
                            Err(error) => {
                                report.failures.push(format!("retry reap child: {error}"));
                            }
                        }
                    }
                }
            }
        }
        report
    }
}

impl PipeReport {
    fn new() -> Self {
        Self {
            stdin_write: None,
            stdin_closed_early: false,
            stdout: CapturedStream::empty(),
            stderr: CapturedStream::empty(),
            failures: Vec::new(),
            status: None,
        }
    }

    fn apply(&mut self, completion: PipeCompletion, cleanup_started: bool) -> Option<String> {
        match completion {
            PipeCompletion::Stdin(Ok(result)) => {
                self.stdin_closed_early = result == StdinWrite::BrokenPipe && !cleanup_started;
                self.stdin_write = Some(result);
                None
            }
            PipeCompletion::Stdin(Err(error)) => Some(format!("stdin writer: {error}")),
            PipeCompletion::Stdout(result) => {
                self.stdout = result.captured;
                result
                    .error
                    .map(|error| format!("stdout collector: {error}"))
            }
            PipeCompletion::Stderr(result) => {
                self.stderr = result.captured;
                result
                    .error
                    .map(|error| format!("stderr collector: {error}"))
            }
        }
    }
}

fn spawn_stdin_worker<SI>(sender: Sender<PipeCompletion>, task: SI) -> io::Result<JoinHandle<()>>
where
    SI: FnOnce() -> io::Result<StdinWrite> + Send + 'static,
{
    thread::Builder::new()
        .name("solstone-generate-stdin".to_owned())
        .spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(task))
                .map_err(|_| "thread panicked".to_owned())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = sender.send(PipeCompletion::Stdin(result));
        })
}

fn spawn_stream_worker<SO>(
    sender: Sender<PipeCompletion>,
    task: SO,
    completion: fn(StreamCollection) -> PipeCompletion,
    name: &'static str,
) -> io::Result<JoinHandle<()>>
where
    SO: FnOnce() -> StreamCollection + Send + 'static,
{
    thread::Builder::new()
        .name(format!("solstone-generate-{name}"))
        .spawn(move || {
            let result =
                panic::catch_unwind(AssertUnwindSafe(task)).unwrap_or_else(|_| StreamCollection {
                    captured: CapturedStream::empty(),
                    error: Some("thread panicked".to_owned()),
                });
            let _ = sender.send(completion(result));
        })
}

impl OneShotClient {
    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self {
            executable: path.into(),
            prefix_arguments: Vec::new(),
            environment: BTreeMap::new(),
        }
    }

    pub fn with_prefix_arguments(mut self, arguments: impl IntoIterator<Item = OsString>) -> Self {
        self.prefix_arguments.extend(arguments);
        self
    }

    pub fn with_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    pub fn sibling() -> Result<Self, ClientError> {
        sibling_executable()
            .map(|path| Self::at_path(path).with_prefix_arguments([OsString::from("generate")]))
    }

    #[cfg(test)]
    pub(crate) fn prefix_arguments(&self) -> &[OsString] {
        &self.prefix_arguments
    }

    pub fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        let input = encode_one_shot_request(request).map_err(ClientError::Decode)?;
        let child = Command::new(&self.executable)
            .args(&self.prefix_arguments)
            .arg("--one-shot")
            .envs(&self.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ClientError::Io {
                primary: error.to_string(),
                cleanup: None,
            })?;
        let mut child = ChildGuard::new(child);
        let Some(stdin) = child.child.stdin.take() else {
            return Err(process_failure_without_workers(
                &mut child,
                "wire stdin is unavailable".to_owned(),
            ));
        };
        let Some(stdout) = child.child.stdout.take() else {
            return Err(process_failure_without_workers(
                &mut child,
                "wire stdout is unavailable".to_owned(),
            ));
        };
        let Some(stderr) = child.child.stderr.take() else {
            return Err(process_failure_without_workers(
                &mut child,
                "wire stderr is unavailable".to_owned(),
            ));
        };
        match PipeWorkers::spawn(stdin, stdout, stderr, input.into_bytes()) {
            Ok(workers) => execute_with_workers(&mut child, workers, None),
            Err((primary, workers)) => execute_with_workers(&mut child, workers, Some(primary)),
        }
    }
}

fn execute_with_workers<C: ChildControl>(
    child: &mut C,
    workers: PipeWorkers,
    initial_failure: Option<String>,
) -> Result<GenerateResponse, ClientError> {
    let mut report = workers.collect(child, initial_failure);
    if !report.failures.is_empty() {
        let primary = report.failures.remove(0);
        return Err(ClientError::ProcessIo(Box::new(ProcessFailure {
            primary,
            cleanup: report.failures,
            status: report.status,
            stdout: report.stdout,
            stderr: report.stderr,
            stdin_closed_early: report.stdin_closed_early,
        })));
    }
    let status = report
        .status
        .expect("a failure-free pipe report has a child status");
    let stdin_write = report
        .stdin_write
        .expect("a failure-free pipe report has a stdin result");
    classify(status, report.stdout, report.stderr, stdin_write)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let base = PathBuf::from("/var/tmp");
            loop {
                let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = base.join(format!(
                    "solstone-core-generate-client-{}-{counter}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self { path },
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create temporary test directory: {error}"),
                }
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct ByteAtATime<'a> {
        data: &'a [u8],
        offset: usize,
    }

    impl Read for ByteAtATime<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.offset >= self.data.len() || buf.is_empty() {
                return Ok(0);
            }
            buf[0] = self.data[self.offset];
            self.offset += 1;
            Ok(1)
        }
    }

    struct ErrorAfter {
        data: Vec<u8>,
        offset: usize,
        fail_at: usize,
    }

    impl Read for ErrorAfter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.offset >= self.fail_at {
                return Err(io::Error::other("injected read failure"));
            }
            if self.offset >= self.data.len() {
                return Ok(0);
            }
            let available = (self.fail_at - self.offset)
                .min(self.data.len() - self.offset)
                .min(buf.len());
            buf[..available].copy_from_slice(&self.data[self.offset..self.offset + available]);
            self.offset += available;
            Ok(available)
        }
    }

    struct FailWrite {
        kind: io::ErrorKind,
    }

    impl Write for FailWrite {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.kind, "injected write failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Sink;

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn resolve_sibling_executable_selects_regular_sibling_file() {
        let temp = TempDir::new();
        let current = temp.path.join("journal");
        let sibling = temp.path.join("solstone-core");
        fs::write(&current, "journal executable").expect("write current executable fixture");
        fs::write(&sibling, "solstone-core executable").expect("write sibling executable fixture");

        assert_eq!(
            resolve_sibling_executable(&current).expect("resolve sibling executable"),
            sibling
        );
    }

    #[test]
    fn resolve_sibling_executable_requires_sibling_file() {
        let temp = TempDir::new();
        let current = temp.path.join("journal");
        fs::write(&current, "journal executable").expect("write current executable fixture");

        assert!(matches!(
            resolve_sibling_executable(&current),
            Err(ClientError::Resolve(_))
        ));
    }

    #[test]
    fn at_path_has_no_prefix() {
        assert!(OneShotClient::at_path("x").prefix_arguments().is_empty());
    }

    #[test]
    fn prefix_arguments_are_additive_and_keep_generate_first() {
        let client = OneShotClient::at_path("x")
            .with_prefix_arguments([OsString::from("generate")])
            .with_prefix_arguments([OsString::from("extra")]);
        assert_eq!(
            client.prefix_arguments(),
            &[OsString::from("generate"), OsString::from("extra")]
        );
    }

    #[test]
    fn collect_bounded_keeps_bytes_under_the_cap() {
        let result = collect_bounded(&b"abc"[..], 8);
        assert_eq!(result.captured.bytes, b"abc");
        assert!(!result.captured.truncated);
        assert!(result.error.is_none());
    }

    #[test]
    fn collect_bounded_flags_truncation_and_keeps_the_cap() {
        let data = vec![b'x'; 16];
        let result = collect_bounded(data.as_slice(), 8);
        assert_eq!(result.captured.bytes, vec![b'x'; 8]);
        assert!(result.captured.truncated);
        assert!(result.error.is_none());
    }

    #[test]
    fn collect_bounded_reads_empty_input() {
        let result = collect_bounded(&b""[..], 8);
        assert!(result.captured.bytes.is_empty());
        assert!(!result.captured.truncated);
        assert!(result.error.is_none());
    }

    #[test]
    fn collect_bounded_accepts_one_byte_reads() {
        let result = collect_bounded(
            ByteAtATime {
                data: b"xyz",
                offset: 0,
            },
            8,
        );
        assert_eq!(result.captured.bytes, b"xyz");
        assert!(!result.captured.truncated);
        assert!(result.error.is_none());
    }

    #[test]
    fn collect_bounded_surfaces_injected_read_errors() {
        let result = collect_bounded(
            ErrorAfter {
                data: b"abcdef".to_vec(),
                offset: 0,
                fail_at: 3,
            },
            8,
        );
        assert_eq!(result.captured.bytes, b"abc");
        assert!(!result.captured.truncated);
        assert_eq!(result.error.as_deref(), Some("injected read failure"));
    }

    #[test]
    fn write_request_completes_a_full_write() {
        assert_eq!(
            write_request(Sink, b"payload").expect("write"),
            StdinWrite::Complete
        );
    }

    #[test]
    fn write_request_maps_broken_pipe() {
        assert_eq!(
            write_request(
                FailWrite {
                    kind: io::ErrorKind::BrokenPipe
                },
                b"payload"
            )
            .expect("broken pipe is expected"),
            StdinWrite::BrokenPipe
        );
    }

    #[test]
    fn write_request_surfaces_other_write_errors() {
        let error = write_request(
            FailWrite {
                kind: io::ErrorKind::PermissionDenied,
            },
            b"payload",
        )
        .expect_err("other write errors fail");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn captured_stream_display_escapes_controls_and_marks_truncation() {
        let stream = CapturedStream {
            bytes: vec![b'a', b'\\', b'b', b'\n', 0xff],
            truncated: true,
        };
        assert_eq!(stream.to_string(), "a\\\\b\\n\\xff [truncated]");
    }

    #[test]
    fn client_error_display_formats_io_cleanup_and_unexpected_child() {
        assert_eq!(
            ClientError::Io {
                primary: "write failed".to_owned(),
                cleanup: Some("wait failed".to_owned()),
            }
            .to_string(),
            "write failed (cleanup: wait failed)"
        );
        assert_eq!(
            ClientError::UnexpectedChild(Box::new(UnexpectedChildFailure {
                status: ChildStatus {
                    exit_code: Some(64),
                    signal: None,
                },
                stdout: CapturedStream::empty(),
                stderr: CapturedStream {
                    bytes: b"Usage:\n".to_vec(),
                    truncated: false,
                },
                stdin_closed_early: false,
            }))
            .to_string(),
            "unexpected child (exit 64); stdin_closed_early=false; stdout=; stderr=Usage:\\n"
        );
    }

    #[test]
    fn protocol_display_preserves_status_and_bounded_stream_evidence() {
        let error = ClientError::Protocol(Box::new(ProtocolFailure {
            error: ProtocolError {
                id: None,
                reason: "fixture_failure".to_owned(),
                detail: "provider rejected output".to_owned(),
            },
            status: ChildStatus {
                exit_code: None,
                signal: Some(9),
            },
            stdout: CapturedStream {
                bytes: vec![0xff, b'\n'],
                truncated: true,
            },
            stderr: CapturedStream {
                bytes: b"diagnostic\n".to_vec(),
                truncated: false,
            },
            stdin_closed_early: true,
        }));

        assert_eq!(
            error.to_string(),
            "protocol fixture_failure (signal 9): provider rejected output; stdin_closed_early=true; stdout=\\xff\\n [truncated]; stderr=diagnostic\\n"
        );
    }

    #[test]
    fn collector_failure_interrupts_blocked_stdin_and_preserves_partial_evidence() {
        struct FakeChild {
            release: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
            terminated: bool,
        }

        impl ChildControl for FakeChild {
            fn try_wait_status(&mut self) -> io::Result<Option<ChildStatus>> {
                Ok(self.terminated.then_some(ChildStatus {
                    exit_code: None,
                    signal: Some(9),
                }))
            }

            fn terminate(&mut self) -> io::Result<()> {
                self.terminated = true;
                let (released, wake) = &*self.release;
                *released.lock().expect("release lock") = true;
                wake.notify_all();
                Ok(())
            }

            fn wait_status(&mut self) -> io::Result<ChildStatus> {
                Ok(ChildStatus {
                    exit_code: None,
                    signal: Some(9),
                })
            }
        }

        let release =
            std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let stdin_release = release.clone();
        let workers = match PipeWorkers::spawn_tasks(
            move || {
                let (released, wake) = &*stdin_release;
                let mut released = released.lock().expect("release lock");
                while !*released {
                    released = wake.wait(released).expect("release wait");
                }
                Ok(StdinWrite::BrokenPipe)
            },
            move || StreamCollection {
                captured: CapturedStream {
                    bytes: b"partial".to_vec(),
                    truncated: false,
                },
                error: Some("injected read failure".to_owned()),
            },
            move || StreamCollection {
                captured: CapturedStream::empty(),
                error: None,
            },
        ) {
            Ok(workers) => workers,
            Err((error, _)) => panic!("fixture workers start: {error}"),
        };
        let mut child = FakeChild {
            release,
            terminated: false,
        };

        let started = std::time::Instant::now();
        let error = execute_with_workers(&mut child, workers, None)
            .expect_err("collector failure must fail execution");
        let ClientError::ProcessIo(failure) = error else {
            panic!("expected process I/O error, got {error:?}");
        };
        assert_eq!(failure.primary, "stdout collector: injected read failure");
        assert_eq!(failure.stdout.bytes, b"partial");
        assert!(
            !failure.stdin_closed_early,
            "cleanup-induced BrokenPipe must not be attributed to the child"
        );
        assert!(failure.status.is_some());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(child.terminated);
    }
}
