// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
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
}

#[derive(Debug)]
pub struct UnexpectedChildFailure {
    pub status: ChildStatus,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
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
                "protocol {} ({}): {}",
                failure.error.reason, failure.status, failure.error.detail
            ),
            Self::UnexpectedChild(failure) => {
                write!(
                    formatter,
                    "unexpected child ({}): {}",
                    failure.status, failure.stderr
                )
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
    stdin: Option<JoinHandle<io::Result<StdinWrite>>>,
    stdout: Option<JoinHandle<io::Result<CapturedStream>>>,
    stderr: Option<JoinHandle<io::Result<CapturedStream>>>,
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

fn collect_bounded<R: Read>(mut reader: R, limit: usize) -> io::Result<CapturedStream> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8_192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
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
    Ok(CapturedStream { bytes, truncated })
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

fn join_thread<T>(handle: JoinHandle<io::Result<T>>, name: &str) -> Result<T, String> {
    match handle.join() {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!("{name} thread panicked")),
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
    exit: ExitStatus,
    stdout: CapturedStream,
    stderr: CapturedStream,
) -> Result<GenerateResponse, ClientError> {
    let status = child_status(&exit);
    if exit.success() {
        if stdout.truncated {
            return Err(ClientError::Decode(format!(
                "stdout truncated at {STDOUT_LIMIT} bytes"
            )));
        }
        let text = std::str::from_utf8(&stdout.bytes)
            .map_err(|error| ClientError::Decode(error.to_string()))?;
        decode_one_shot_response(text).map_err(ClientError::Decode)
    } else if let Some(error) = try_protocol(&stderr) {
        Err(ClientError::Protocol(Box::new(ProtocolFailure {
            error,
            status,
            stdout,
            stderr,
        })))
    } else {
        Err(ClientError::UnexpectedChild(Box::new(
            UnexpectedChildFailure {
                status,
                stdout,
                stderr,
            },
        )))
    }
}

fn reap_after_failure(
    child: &mut Child,
    workers: Option<&mut PipeWorkers>,
    primary: String,
) -> ClientError {
    let _ = child.kill();
    if let Some(workers) = workers {
        workers.join_remaining();
    }
    match child.wait() {
        Ok(_) => ClientError::Io {
            primary,
            cleanup: None,
        },
        Err(error) => ClientError::Io {
            primary,
            cleanup: Some(error.to_string()),
        },
    }
}

impl PipeWorkers {
    fn spawn(
        stdin: std::process::ChildStdin,
        stdout: std::process::ChildStdout,
        stderr: std::process::ChildStderr,
        input: Vec<u8>,
    ) -> Self {
        Self {
            stdin: Some(thread::spawn(move || write_request(stdin, &input))),
            stdout: Some(thread::spawn(move || collect_bounded(stdout, STDOUT_LIMIT))),
            stderr: Some(thread::spawn(move || collect_bounded(stderr, STDERR_LIMIT))),
        }
    }

    fn join_stdin(&mut self) -> Result<StdinWrite, String> {
        join_thread(
            self.stdin.take().expect("stdin thread joined once"),
            "stdin",
        )
    }

    fn join_stdout(&mut self) -> Result<CapturedStream, String> {
        join_thread(
            self.stdout.take().expect("stdout thread joined once"),
            "stdout",
        )
    }

    fn join_stderr(&mut self) -> Result<CapturedStream, String> {
        join_thread(
            self.stderr.take().expect("stderr thread joined once"),
            "stderr",
        )
    }

    fn join_remaining(&mut self) {
        if let Some(handle) = self.stdin.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stdout.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PipeWorkers {
    fn drop(&mut self) {
        self.join_remaining();
    }
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
        let mut child = Command::new(&self.executable)
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
        let Some(stdin) = child.stdin.take() else {
            return Err(reap_after_failure(
                &mut child,
                None,
                "wire stdin is unavailable".to_owned(),
            ));
        };
        let Some(stdout) = child.stdout.take() else {
            return Err(reap_after_failure(
                &mut child,
                None,
                "wire stdout is unavailable".to_owned(),
            ));
        };
        let Some(stderr) = child.stderr.take() else {
            return Err(reap_after_failure(
                &mut child,
                None,
                "wire stderr is unavailable".to_owned(),
            ));
        };
        let mut workers = PipeWorkers::spawn(stdin, stdout, stderr, input.into_bytes());
        execute_with_workers(&mut child, &mut workers)
    }
}

fn execute_with_workers(
    child: &mut Child,
    workers: &mut PipeWorkers,
) -> Result<GenerateResponse, ClientError> {
    match workers.join_stdin() {
        Ok(StdinWrite::Complete | StdinWrite::BrokenPipe) => {}
        Err(primary) => return Err(reap_after_failure(child, Some(workers), primary)),
    }
    let stdout = match workers.join_stdout() {
        Ok(stdout) => stdout,
        Err(primary) => return Err(reap_after_failure(child, Some(workers), primary)),
    };
    let stderr = match workers.join_stderr() {
        Ok(stderr) => stderr,
        Err(primary) => return Err(reap_after_failure(child, Some(workers), primary)),
    };
    match child.wait() {
        Ok(status) => classify(status, stdout, stderr),
        Err(error) => Err(reap_after_failure(child, Some(workers), error.to_string())),
    }
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
        let stream = collect_bounded(&b"abc"[..], 8).expect("read");
        assert_eq!(stream.bytes, b"abc");
        assert!(!stream.truncated);
    }

    #[test]
    fn collect_bounded_flags_truncation_and_keeps_the_cap() {
        let data = vec![b'x'; 16];
        let stream = collect_bounded(data.as_slice(), 8).expect("read");
        assert_eq!(stream.bytes, vec![b'x'; 8]);
        assert!(stream.truncated);
    }

    #[test]
    fn collect_bounded_reads_empty_input() {
        let stream = collect_bounded(&b""[..], 8).expect("read");
        assert!(stream.bytes.is_empty());
        assert!(!stream.truncated);
    }

    #[test]
    fn collect_bounded_accepts_one_byte_reads() {
        let stream = collect_bounded(
            ByteAtATime {
                data: b"xyz",
                offset: 0,
            },
            8,
        )
        .expect("read");
        assert_eq!(stream.bytes, b"xyz");
        assert!(!stream.truncated);
    }

    #[test]
    fn collect_bounded_surfaces_injected_read_errors() {
        let error = collect_bounded(
            ErrorAfter {
                data: b"abcdef".to_vec(),
                offset: 0,
                fail_at: 3,
            },
            8,
        )
        .expect_err("injected failure");
        assert_eq!(error.to_string(), "injected read failure");
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
            }))
            .to_string(),
            "unexpected child (exit 64): Usage:\\n"
        );
    }
}
