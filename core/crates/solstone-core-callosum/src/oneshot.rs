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
        formatter.write_str("Callosum socket unavailable")
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
            use std::io::{ErrorKind, Read, Write};
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

            fn write_all_deadline(
                stream: &mut interprocess::local_socket::Stream,
                bytes: &[u8],
                deadline: Instant,
            ) -> std::io::Result<()> {
                let mut offset = 0;
                while offset < bytes.len() {
                    let written = retry_io(deadline, || stream.write(&bytes[offset..]))?;
                    if written == 0 {
                        return Err(std::io::Error::new(
                            ErrorKind::WriteZero,
                            "Callosum pipe closed",
                        ));
                    }
                    offset += written;
                }
                Ok(())
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
            write_all_deadline(&mut stream, &proof, deadline)
                .map_err(|_| CallosumOneShotError::Unavailable)?;
            write_all_deadline(&mut stream, line.as_bytes(), deadline)
                .map_err(|_| CallosumOneShotError::Unavailable)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = line;
            Err(CallosumOneShotError::Unavailable)
        }
    }
}
