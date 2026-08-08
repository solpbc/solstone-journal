// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Failure to deliver a one-shot Callosum line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallosumOneShotError {
    /// The local Unix socket cannot be used.
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
        #[cfg(not(unix))]
        {
            let _ = line;
            Err(CallosumOneShotError::Unavailable)
        }
    }
}
