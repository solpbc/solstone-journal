// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use ureq::Error;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, LazyBuffers, NextTimeout, Transport,
};

#[derive(Debug)]
pub struct UnixConnector(pub PathBuf);

impl<In: Transport> Connector<In> for UnixConnector {
    type Out = UnixTransport;

    fn connect(
        &self,
        details: &ConnectionDetails,
        _: Option<In>,
    ) -> Result<Option<Self::Out>, Error> {
        if details.uri.scheme_str() != Some("http") || details.uri.host() != Some("localhost") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unix transport requires http://localhost",
            )
            .into());
        }
        let stream = crate::connect(
            &self.0,
            details
                .timeout
                .not_zero()
                .map(|d| *d)
                .unwrap_or(std::time::Duration::from_secs(2)),
        )?;
        Ok(Some(UnixTransport {
            stream,
            buffers: LazyBuffers::new(
                details.config.input_buffer_size(),
                details.config.output_buffer_size(),
            ),
        }))
    }
}

#[derive(Debug)]
pub struct UnixTransport {
    stream: UnixStream,
    buffers: LazyBuffers,
}

fn map_io(error: io::Error, timeout: NextTimeout) -> Error {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Error::Timeout(timeout.reason),
        _ => error.into(),
    }
}

impl UnixTransport {
    fn read_input(&mut self, timeout: NextTimeout) -> io::Result<usize> {
        #[cfg(target_os = "macos")]
        if let Err(error) = self.stream.set_read_timeout(timeout.not_zero().map(|d| *d)) {
            if error.raw_os_error() == Some(libc::EINVAL) {
                // Darwin can reject SO_RCVTIMEO after the peer has closed even
                // while response bytes remain queued. Drain only immediately
                // available bytes; this fallback must never block.
                self.stream.set_nonblocking(true)?;
                let result = self.stream.read(self.buffers.input_append_buf());
                self.stream.set_nonblocking(false)?;
                return result;
            }
            return Err(error);
        }
        #[cfg(target_os = "linux")]
        self.stream
            .set_read_timeout(timeout.not_zero().map(|d| *d))?;
        self.stream.read(self.buffers.input_append_buf())
    }
}

impl Transport for UnixTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), Error> {
        self.stream
            .set_write_timeout(timeout.not_zero().map(|d| *d))?;
        self.stream
            .write_all(&self.buffers.output()[..amount])
            .map_err(|e| map_io(e, timeout))
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, Error> {
        let n = self.read_input(timeout).map_err(|e| map_io(e, timeout))?;
        self.buffers.input_appended(n);
        Ok(n > 0)
    }

    fn is_open(&mut self) -> bool {
        // Each request opens a fresh connection to the selected pathname.
        false
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn queued_response_is_read_after_the_peer_closes() {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        peer.write_all(b"reply").unwrap();
        drop(peer);
        let mut transport = UnixTransport {
            stream,
            buffers: LazyBuffers::new(4096, 4096),
        };
        let timeout = NextTimeout {
            after: std::time::Duration::from_millis(250).into(),
            reason: ureq::Timeout::Global,
        };
        assert_eq!(transport.read_input(timeout).unwrap(), 5);
        assert_eq!(transport.read_input(timeout).unwrap(), 0);
    }
}
