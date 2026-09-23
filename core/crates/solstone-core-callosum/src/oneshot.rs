// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::net::UnixStream;

            let mut stream = UnixStream::connect(&self.socket_path)
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            stream
                .set_write_timeout(Some(self.timeout))
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            stream
                .set_read_timeout(Some(self.timeout))
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            stream
                .write_all(line.as_bytes())
                .map_err(|_| CallosumOneShotError::Unavailable)
        }
        #[cfg(windows)]
        {
            use std::io::{ErrorKind, Read};
            use std::thread;
            use std::time::Instant;

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

            let deadline = Instant::now() + self.timeout;
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
                .map_err(|_| CallosumOneShotError::Unavailable)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = line;
            Err(CallosumOneShotError::Unavailable)
        }
    }
}

/// Write all of `bytes` before `deadline`.
///
/// The Windows client's pipe is nonblocking, and a nonblocking named pipe reports a full inbound
/// buffer as a zero-length write -- a line longer than the free buffer is refused whole until the
/// server's read is posted. So zero and `WouldBlock` both mean "not yet" and are retried; a closed
/// pipe fails the write with an error of its own.
#[cfg(any(windows, test))]
fn write_all_before(
    writer: &mut impl std::io::Write,
    bytes: &[u8],
    deadline: std::time::Instant,
) -> std::io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
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

    use super::write_all_before;

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
