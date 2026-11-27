// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A private request channel over an authenticated named pipe.
//!
//! The pipe takes its name from a journal socket path, as Callosum's does, and
//! admits a caller by the same two checks: a DACL that grants the owner's
//! account alone, with remote clients refused, and a challenge answered with
//! the journal's pipe secret. Each connection then carries whatever its two
//! ends agree on; a call that does not finish the challenge is dropped.

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use interprocess::local_socket::traits::{Listener as _, Stream as _};
use interprocess::local_socket::{
    ConnectOptions, Listener, ListenerNonblockingMode, ListenerOptions, Stream, ToFsName,
};
use interprocess::os::windows::local_socket::{ListenerOptionsExt as _, NamedPipe};

use crate::windows::{
    PIPE_CHALLENGE_LEN, PIPE_HANDSHAKE_LEN, client_proof, create_or_read_secret, pipe_name,
    read_secret, server_greeting, verify_client_proof,
};

/// A connected private pipe, both ways.
pub type PrivatePipeStream = Stream;

/// The server answers the caller's own challenge too, so a caller never sends
/// a call to whatever took the pipe's name first.
const SERVER_PROOF_DOMAIN: &[u8] = b"solstone-private-pipe-server-proof-v1\0";

fn server_proof(
    secret: &[u8; PIPE_CHALLENGE_LEN],
    challenge: &[u8; PIPE_CHALLENGE_LEN],
) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(SERVER_PROOF_DOMAIN);
    mac.update(challenge);
    mac.finalize().into_bytes().into()
}

/// Accepts callers of one private pipe. Accepting never blocks.
pub struct PrivatePipeListener {
    listener: Listener,
    secret: [u8; PIPE_CHALLENGE_LEN],
}

/// A caller that has connected and not yet answered the challenge.
pub struct PendingCaller {
    stream: Stream,
    secret: [u8; PIPE_CHALLENGE_LEN],
}

impl PrivatePipeListener {
    /// Serve the pipe named for `socket_path`.
    ///
    /// # Errors
    /// Returns the naming, secret or pipe-creation error.
    pub fn bind(socket_path: &Path) -> io::Result<Self> {
        let name = pipe_name(socket_path)?.to_fs_name::<NamedPipe>()?;
        let secret = create_or_read_secret(socket_path)?;
        let listener = ListenerOptions::new()
            .name(name)
            .security_descriptor(crate::windows::current_user_pipe_security_descriptor()?)
            .nonblocking(ListenerNonblockingMode::Accept)
            .create_sync()?;
        Ok(Self { listener, secret })
    }

    /// The next caller, or `None` when none is waiting.
    ///
    /// # Errors
    /// Returns an accept error other than an empty queue.
    pub fn accept(&self) -> io::Result<Option<PendingCaller>> {
        match self.listener.accept() {
            Ok(stream) => Ok(Some(PendingCaller {
                stream,
                secret: self.secret,
            })),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl PendingCaller {
    /// Challenge the caller; a caller that answers with the journal's secret
    /// is admitted. This blocks on the caller, so run it off the accept loop.
    ///
    /// # Errors
    /// Returns an error for a caller that fails or abandons the challenge.
    pub fn admit(mut self) -> io::Result<PrivatePipeStream> {
        let mut challenge = [0_u8; PIPE_CHALLENGE_LEN];
        getrandom::fill(&mut challenge).map_err(|error| io::Error::other(error.to_string()))?;
        let greeting = server_greeting(challenge);
        self.stream.write_all(&greeting)?;
        self.stream.flush()?;
        let mut proof = [0_u8; PIPE_HANDSHAKE_LEN];
        self.stream.read_exact(&mut proof)?;
        if !verify_client_proof(&self.secret, &greeting, &proof) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private pipe caller failed the challenge",
            ));
        }
        let mut theirs = [0_u8; PIPE_CHALLENGE_LEN];
        self.stream.read_exact(&mut theirs)?;
        self.stream
            .write_all(&server_proof(&self.secret, &theirs))?;
        self.stream.flush()?;
        Ok(self.stream)
    }
}

/// Connect to the private pipe named for `socket_path`, answer its
/// challenge and check the server's answer to ours. `None` means no one
/// serves that pipe, or no one answered within `wait`; the caller then works
/// without it.
///
/// # Errors
/// Returns an error when the server failed to prove it holds the secret.
pub fn connect_private_pipe(
    socket_path: &Path,
    wait: Duration,
) -> io::Result<Option<PrivatePipeStream>> {
    let Ok(name) = pipe_name(socket_path).and_then(|name| name.to_fs_name::<NamedPipe>()) else {
        return Ok(None);
    };
    let Ok(secret) = read_secret(socket_path) else {
        return Ok(None);
    };
    // A blocking connect waits without limit for a busy pipe, and a greeting
    // read has no timeout, so both run on a thread the caller stops waiting for.
    let (sent, answered) = std::sync::mpsc::channel();
    let name = name.into_owned();
    std::thread::Builder::new()
        .name("private-pipe-connect".into())
        .spawn(move || {
            let outcome = (|| {
                let Ok(mut stream) = ConnectOptions::new().name(name).connect_sync() else {
                    return Ok(None);
                };
                let mut greeting = [0_u8; PIPE_HANDSHAKE_LEN];
                stream.read_exact(&mut greeting)?;
                stream.write_all(&client_proof(&secret, &greeting)?)?;
                let mut ours = [0_u8; PIPE_CHALLENGE_LEN];
                getrandom::fill(&mut ours).map_err(|error| io::Error::other(error.to_string()))?;
                stream.write_all(&ours)?;
                stream.flush()?;
                let mut answer = [0_u8; 32];
                stream.read_exact(&mut answer)?;
                if answer != server_proof(&secret, &ours) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "private pipe server failed the challenge",
                    ));
                }
                Ok(Some(stream))
            })();
            let _ = sent.send(outcome);
        })?;
    answered.recv_timeout(wait).unwrap_or(Ok(None))
}

/// Split a connected pipe into its receiving and sending halves.
pub fn split_private_pipe(
    stream: PrivatePipeStream,
) -> (impl Read + Send + 'static, impl Write + Send + 'static) {
    stream.split()
}

/// Name a test's private pipe without an installation binding, as
/// Callosum's own native tests do. Absent from builds without test-fixtures.
#[cfg(feature = "test-fixtures")]
#[must_use]
pub fn test_pipe_namespace(socket_path: &Path) -> impl Drop + use<> {
    crate::windows::namespace_fixture::Guard::register(socket_path)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn a_caller_with_the_secret_is_admitted_and_both_ends_talk() {
        let journal = tempfile::tempdir().unwrap();
        let health = journal.path().join("health");
        std::fs::create_dir_all(&health).unwrap();
        let socket = health.join("private-test.sock");
        let _namespace = crate::windows::namespace_fixture::Guard::register(&socket);
        let listener = PrivatePipeListener::bind(&socket).unwrap();
        let client = {
            let socket = socket.clone();
            std::thread::spawn(move || {
                let mut stream = connect_private_pipe(&socket, Duration::from_secs(5))
                    .unwrap()
                    .expect("served");
                stream.write_all(b"ping").unwrap();
                let mut reply = [0_u8; 4];
                stream.read_exact(&mut reply).unwrap();
                reply
            })
        };
        let pending = loop {
            if let Some(pending) = listener.accept().unwrap() {
                break pending;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let (mut receive, mut send) = split_private_pipe(pending.admit().unwrap());
        let mut asked = [0_u8; 4];
        receive.read_exact(&mut asked).unwrap();
        assert_eq!(&asked, b"ping");
        send.write_all(b"pong").unwrap();
        assert_eq!(&client.join().unwrap(), b"pong");
    }

    #[test]
    fn a_caller_without_the_secret_is_refused() {
        let journal = tempfile::tempdir().unwrap();
        let health = journal.path().join("health");
        std::fs::create_dir_all(&health).unwrap();
        let socket = health.join("refused-test.sock");
        let _namespace = crate::windows::namespace_fixture::Guard::register(&socket);
        let listener = PrivatePipeListener::bind(&socket).unwrap();
        // The caller holds a different secret than the server's.
        std::fs::write(crate::windows::secret_path(&socket).unwrap(), [7_u8; 32]).unwrap();
        let client = {
            let socket = socket.clone();
            std::thread::spawn(move || connect_private_pipe(&socket, Duration::from_secs(5)))
        };
        let pending = loop {
            if let Some(pending) = listener.accept().unwrap() {
                break pending;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(pending.admit().is_err());
        assert!(!matches!(client.join().unwrap(), Ok(Some(_))));
    }

    #[test]
    fn a_pipe_no_one_serves_reads_as_absent() {
        let journal = tempfile::tempdir().unwrap();
        let health = journal.path().join("health");
        std::fs::create_dir_all(&health).unwrap();
        let socket = health.join("absent-test.sock");
        let _namespace = crate::windows::namespace_fixture::Guard::register(&socket);
        // As in a journal, the pipe secret exists; only the server is missing.
        create_or_read_secret(&socket).unwrap();
        assert!(
            connect_private_pipe(&socket, Duration::from_millis(100))
                .unwrap()
                .is_none()
        );
    }
}
