// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::CallosumEnvelope;

/// Failure to deliver a one-shot Callosum line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallosumOneShotError {
    /// The local Callosum transport cannot be used.
    Unavailable,
}

impl fmt::Display for CallosumOneShotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Callosum transport unavailable")
    }
}

impl Error for CallosumOneShotError {}

/// Synchronous one-shot writer for an already framed Callosum line.
#[derive(Clone, Debug)]
pub struct CallosumOneShotSender {
    socket_path: PathBuf,
    timeout: Duration,
}

impl CallosumOneShotSender {
    /// Construct a one-shot sender for `socket_path`.
    pub fn new(socket_path: impl AsRef<Path>, timeout: Duration) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            timeout,
        }
    }

    /// Connect, write one already newline-framed line, and close the socket.
    pub fn send_line(&self, line: &str) -> Result<(), CallosumOneShotError> {
        self.send_line_inner(line, None)
    }

    /// Keep the sender alive until the bus echoes its complete envelope.
    ///
    /// Confirmation proves publication on the bus, not execution by a consumer.
    /// Connection, writing, and confirmation share one deadline. Other broadcasts
    /// are drained without changing which packet confirms this send.
    pub fn send_line_confirmed(&self, line: &str) -> Result<(), CallosumOneShotError> {
        let expected = serde_json::from_str::<CallosumEnvelope>(line)
            .map_err(|_| CallosumOneShotError::Unavailable)?;
        self.send_line_inner(line, Some(expected))
    }

    fn send_line_inner(
        &self,
        line: &str,
        expected: Option<CallosumEnvelope>,
    ) -> Result<(), CallosumOneShotError> {
        let deadline = Instant::now() + self.timeout;
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::net::UnixStream;

            let mut stream = if expected.is_some() {
                let socket =
                    socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)
                        .map_err(|_| CallosumOneShotError::Unavailable)?;
                let address = socket2::SockAddr::unix(&self.socket_path)
                    .map_err(|_| CallosumOneShotError::Unavailable)?;
                socket
                    .connect_timeout(&address, self.timeout)
                    .map_err(|_| CallosumOneShotError::Unavailable)?;
                let descriptor: std::os::fd::OwnedFd = socket.into();
                UnixStream::from(descriptor)
            } else {
                UnixStream::connect(&self.socket_path)
                    .map_err(|_| CallosumOneShotError::Unavailable)?
            };
            stream
                .set_write_timeout(Some(self.timeout))
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            stream
                .set_read_timeout(Some(self.timeout))
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            if let Some(expected) = expected {
                stream
                    .set_nonblocking(true)
                    .map_err(|_| CallosumOneShotError::Unavailable)?;
                write_all_before(&mut stream, line.as_bytes(), deadline)
                    .and_then(|()| read_echo_before(&mut stream, &expected, deadline, false))
                    .map_err(|_| CallosumOneShotError::Unavailable)
            } else {
                stream
                    .write_all(line.as_bytes())
                    .map_err(|_| CallosumOneShotError::Unavailable)
            }
        }
        #[cfg(windows)]
        {
            use std::io::{ErrorKind, Read};
            use std::thread;

            use interprocess::ConnectWaitMode;
            use interprocess::local_socket::{ConnectOptions, ToFsName};
            use interprocess::os::windows::local_socket::NamedPipe;

            use crate::windows::{PIPE_HANDSHAKE_LEN, client_proof, pipe_name, read_secret};

            fn retry_io<T>(
                deadline: Instant,
                mut operation: impl FnMut() -> std::io::Result<T>,
            ) -> std::io::Result<T> {
                loop {
                    match operation() {
                        Ok(value) => return Ok(value),
                        Err(error)
                            if error.kind() == ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(error) => return Err(error),
                    }
                }
            }

            fn read_exact_deadline(
                stream: &mut interprocess::local_socket::Stream,
                bytes: &mut [u8],
                deadline: Instant,
            ) -> std::io::Result<()> {
                let mut offset = 0;
                while offset < bytes.len() {
                    let read = retry_io(deadline, || stream.read(&mut bytes[offset..]))?;
                    if read == 0 {
                        // A nonblocking named-pipe client can observe a transient zero-length
                        // read immediately after connect_sync() returns, before the server's
                        // async accept has resumed far enough to write the greeting. That is not
                        // peer closure. Only report EOF once no bytes have arrived by deadline.
                        if Instant::now() < deadline {
                            thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        return Err(std::io::Error::new(
                            ErrorKind::UnexpectedEof,
                            "Callosum pipe closed",
                        ));
                    }
                    offset += read;
                }
                Ok(())
            }

            let name = pipe_name(&self.socket_path)
                .and_then(|name| name.to_fs_name::<NamedPipe>())
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            let mut stream = ConnectOptions::new()
                .name(name)
                .wait_mode(ConnectWaitMode::Timeout(self.timeout))
                .nonblocking_stream(true)
                .connect_sync()
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            let secret =
                read_secret(&self.socket_path).map_err(|_| CallosumOneShotError::Unavailable)?;
            let mut greeting = [0_u8; PIPE_HANDSHAKE_LEN];
            read_exact_deadline(&mut stream, &mut greeting, deadline)
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            let proof =
                client_proof(&secret, &greeting).map_err(|_| CallosumOneShotError::Unavailable)?;
            write_all_before(&mut stream, &proof, deadline)
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            write_all_before(&mut stream, line.as_bytes(), deadline)
                .and_then(|()| match expected {
                    Some(expected) => read_echo_before(&mut stream, &expected, deadline, true),
                    None => Ok(()),
                })
                .map_err(|_| CallosumOneShotError::Unavailable)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (line, expected, deadline);
            Err(CallosumOneShotError::Unavailable)
        }
    }
}

// Confirmation is used for small control envelopes. Oversized unrelated
// broadcasts are skipped through their newline without retaining their body.
const CONFIRM_FRAME_LIMIT: usize = 64 * 1024;

fn read_echo_before(
    reader: &mut impl std::io::Read,
    expected: &CallosumEnvelope,
    deadline: Instant,
    transient_zero: bool,
) -> std::io::Result<()> {
    use std::io::ErrorKind;

    let mut frame = Vec::new();
    let mut oversized = false;
    let mut buffer = [0_u8; 4096];
    loop {
        if Instant::now() >= deadline {
            return Err(ErrorKind::TimedOut.into());
        }
        let count = match reader.read(&mut buffer) {
            Ok(0) if !transient_zero => return Err(ErrorKind::UnexpectedEof.into()),
            Ok(0) => 0,
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::WouldBlock => 0,
            Err(error) => return Err(error),
        };
        if count == 0 {
            std::thread::sleep(
                Duration::from_millis(1).min(deadline.saturating_duration_since(Instant::now())),
            );
            continue;
        }
        for byte in &buffer[..count] {
            if *byte == b'\n' {
                if !oversized
                    && let Ok(echo) = serde_json::from_slice::<CallosumEnvelope>(&frame)
                    && echo.tract == expected.tract
                    && echo.event == expected.event
                    && echo.extra == expected.extra
                    && expected.ts.is_none_or(|ts| echo.ts == Some(ts))
                {
                    return Ok(());
                }
                frame.clear();
                oversized = false;
            } else if !oversized {
                if frame.len() == CONFIRM_FRAME_LIMIT {
                    frame.clear();
                    oversized = true;
                } else {
                    frame.push(*byte);
                }
            }
        }
    }
}

/// Write all of `bytes` before `deadline`.
///
/// The Windows client's pipe is nonblocking, and a nonblocking named pipe reports a full inbound
/// buffer as a zero-length write -- a line longer than the free buffer is refused whole until the
/// server's read is posted. So zero and `WouldBlock` both mean "not yet" and are retried; a closed
/// pipe fails the write with an error of its own.
#[cfg(any(unix, windows, test))]
fn write_all_before(
    writer: &mut impl std::io::Write,
    bytes: &[u8],
    deadline: std::time::Instant,
) -> std::io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        if std::time::Instant::now() >= deadline {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        match writer.write(&bytes[offset..]) {
            Ok(written) if written > 0 => {
                offset += written;
                continue;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Callosum pipe did not accept the line before the deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::{self, ErrorKind, Write};
    use std::time::{Duration, Instant};

    use super::{CONFIRM_FRAME_LIMIT, read_echo_before, write_all_before};

    fn expected() -> crate::CallosumEnvelope {
        serde_json::from_str(
            r#"{"tract":"supervisor","event":"request","ref":"browser:fixture","cmd":["journal","indexer","--rescan-file","20260930/device_browser_9e92ab54/174500_599/browser_pages.jsonl"]}"#,
        )
        .unwrap()
    }

    #[test]
    fn confirmation_matches_the_complete_packet_and_preserves_explicit_timestamp() {
        let mut expected = expected();
        expected.ts = Some(42);
        let mut wrong = serde_json::to_value(&expected).unwrap();
        wrong["cmd"] = serde_json::json!(["journal", "indexer", "--rescan"]);
        let mut wrong_timestamp = serde_json::to_value(&expected).unwrap();
        wrong_timestamp["ts"] = serde_json::json!(43);
        let stream = format!(
            "{wrong}\n{wrong_timestamp}\n{}\n",
            serde_json::to_string(&expected).unwrap()
        );
        let mut reader = io::Cursor::new(stream.into_bytes());
        read_echo_before(
            &mut reader,
            &expected,
            Instant::now() + Duration::from_secs(1),
            false,
        )
        .unwrap();
        assert_eq!(reader.position(), reader.get_ref().len() as u64);
    }

    struct Fragmented {
        bytes: io::Cursor<Vec<u8>>,
    }

    impl io::Read for Fragmented {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            io::Read::read(&mut self.bytes, &mut bytes[..1])
        }
    }

    #[test]
    fn confirmation_handles_fragments_server_timestamp_and_oversized_other_frames() {
        let expected = expected();
        let mut echo = serde_json::to_value(&expected).unwrap();
        echo["ts"] = serde_json::json!(1790816700000_i64);
        let stream = format!("{}\n{echo}\n", "x".repeat(CONFIRM_FRAME_LIMIT + 1));
        let mut reader = Fragmented {
            bytes: io::Cursor::new(stream.into_bytes()),
        };
        read_echo_before(
            &mut reader,
            &expected,
            Instant::now() + Duration::from_secs(1),
            false,
        )
        .unwrap();
    }

    #[test]
    fn confirmation_refuses_eof_without_the_matching_packet_and_an_expired_deadline() {
        let expected = expected();
        let mut eof = io::empty();
        assert_eq!(
            read_echo_before(
                &mut eof,
                &expected,
                Instant::now() + Duration::from_secs(1),
                false
            )
            .unwrap_err()
            .kind(),
            ErrorKind::UnexpectedEof
        );
        assert_eq!(
            read_echo_before(&mut eof, &expected, Instant::now(), true)
                .unwrap_err()
                .kind(),
            ErrorKind::TimedOut
        );
    }

    /// Plays back scripted write results; `Ok(n)` accepts `n` bytes.
    struct Scripted {
        results: VecDeque<io::Result<usize>>,
        accepted: Vec<u8>,
    }

    impl Write for Scripted {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let result = self.results.pop_front().unwrap_or(Ok(0));
            if let Ok(n) = result {
                self.accepted.extend_from_slice(&buf[..n.min(buf.len())]);
            }
            result
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn scripted(results: Vec<io::Result<usize>>) -> Scripted {
        Scripted {
            results: results.into(),
            accepted: Vec::new(),
        }
    }

    #[test]
    fn a_full_pipe_is_retried_until_it_accepts_the_line() {
        let mut pipe = scripted(vec![
            Ok(0),
            Err(ErrorKind::WouldBlock.into()),
            Ok(0),
            Ok(3),
            Ok(2),
        ]);
        let deadline = Instant::now() + Duration::from_secs(5);
        write_all_before(&mut pipe, b"hello", deadline).unwrap();
        assert_eq!(pipe.accepted, b"hello");
    }

    #[test]
    fn a_pipe_that_never_accepts_times_out_at_the_deadline() {
        let mut pipe = scripted(Vec::new());
        let error = write_all_before(&mut pipe, b"hello", Instant::now()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
    }

    #[test]
    fn a_closed_pipe_fails_at_once() {
        let mut pipe = scripted(vec![Err(ErrorKind::BrokenPipe.into())]);
        let deadline = Instant::now() + Duration::from_secs(5);
        let error = write_all_before(&mut pipe, b"hello", deadline).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    }
}
