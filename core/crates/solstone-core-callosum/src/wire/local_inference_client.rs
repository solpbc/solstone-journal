// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::path::Path;
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::local_inference::{
    ExchangeSession, LocalInferenceReadError, LocalInferenceSnapshot, NONCE_LEN, gate_async,
    gate_sync,
};

/// Whether this platform hands local-inference credentials over Callosum.
const HANDOFF_SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));

/// Asynchronously request an active local-inference snapshot from the Callosum supervisor.
///
/// On platforms other than Windows and Linux, returns [`LocalInferenceReadError::Unsupported`]
/// with no I/O. One deadline covers connection, handshake, request transmission, and response
/// verification; no retry is performed.
///
/// On Windows the named-pipe endpoint DACL and remote-client rejection protect
/// cross-user/cross-identity and remote-network access. On Linux each end checks the
/// other's kernel peer credentials, so only this account is answered and only this
/// account's server is believed. Neither protects against same-account malware, which
/// is trusted once admitted.
pub async fn request_local_inference_snapshot(
    socket_path: impl AsRef<Path>,
    deadline: Instant,
) -> Result<LocalInferenceSnapshot, LocalInferenceReadError> {
    let _ = socket_path;
    let _nonce = gate_async(HANDOFF_SUPPORTED, fill_nonce)?;

    #[cfg(windows)]
    {
        return before_deadline(deadline, async {
            use interprocess::local_socket::{ConnectOptions, ToFsName};
            use interprocess::os::windows::local_socket::NamedPipe;

            let socket_path = socket_path.as_ref();
            if Instant::now() >= deadline {
                return Err(LocalInferenceReadError::Timeout);
            }
            let name = crate::windows::pipe_name(socket_path)
                .and_then(|n| n.to_fs_name::<NamedPipe>())
                .map_err(|_| LocalInferenceReadError::Transport)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut stream =
                tokio::time::timeout(remaining, ConnectOptions::new().name(name).connect_tokio())
                    .await
                    .map_err(|_| LocalInferenceReadError::Timeout)?
                    .map_err(|_| LocalInferenceReadError::Transport)?;
            if Instant::now() >= deadline {
                return Err(LocalInferenceReadError::Timeout);
            }
            let secret = crate::windows::read_secret(socket_path)
                .map_err(|_| LocalInferenceReadError::Authentication)?;
            let mut greeting = [0_u8; crate::windows::PIPE_HANDSHAKE_LEN];
            let mut greeting_read = 0;
            while greeting_read < greeting.len() {
                if Instant::now() >= deadline {
                    return Err(LocalInferenceReadError::Timeout);
                }
                match stream.read(&mut greeting[greeting_read..]).await {
                    Ok(0) => return Err(LocalInferenceReadError::Transport),
                    Ok(n) => {
                        greeting_read += n;
                        if Instant::now() >= deadline && greeting_read < greeting.len() {
                            return Err(LocalInferenceReadError::Timeout);
                        }
                    }
                    Err(_) => return Err(LocalInferenceReadError::Transport),
                }
            }
            if Instant::now() >= deadline {
                return Err(LocalInferenceReadError::Timeout);
            }
            let proof = crate::windows::client_proof(&secret, &greeting)
                .map_err(|_| LocalInferenceReadError::Authentication)?;
            let mut proof_written = 0;
            while proof_written < proof.len() {
                if Instant::now() >= deadline {
                    return Err(LocalInferenceReadError::Timeout);
                }
                match stream.write(&proof[proof_written..]).await {
                    Ok(0) => return Err(LocalInferenceReadError::Transport),
                    Ok(n) => {
                        proof_written += n;
                        if Instant::now() >= deadline && proof_written < proof.len() {
                            return Err(LocalInferenceReadError::Timeout);
                        }
                    }
                    Err(_) => return Err(LocalInferenceReadError::Transport),
                }
            }
            if Instant::now() >= deadline {
                return Err(LocalInferenceReadError::Timeout);
            }

            let correlation = 1_u64;
            let mut session = ExchangeSession::new(secret, correlation, _nonce, deadline);
            drive_exchange_async(&mut stream, &mut session).await
        })
        .await;
    }
    #[cfg(target_os = "linux")]
    {
        return before_deadline(deadline, async {
            let path = crate::endpoint_path(socket_path.as_ref())
                .map_err(|_| LocalInferenceReadError::Transport)?;
            let mut stream = tokio::net::UnixStream::connect(path.as_ref())
                .await
                .map_err(|_| LocalInferenceReadError::Transport)?;
            if !peer_is_this_account(&stream) {
                return Err(LocalInferenceReadError::Authentication);
            }
            let mut session =
                ExchangeSession::new(crate::local_inference::UNIX_REPLY_KEY, 1, _nonce, deadline);
            drive_exchange_async(&mut stream, &mut session).await
        })
        .await;
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = deadline;
        Err(LocalInferenceReadError::Unsupported)
    }
}

/// Synchronously request an active local-inference snapshot from the Callosum supervisor.
///
/// `Handle::try_current()` detects an entered runtime; it is also `Some` on a `spawn_blocking`
/// worker and is not a way to tell worker kinds apart. If it is `Some`, return `RuntimeEntered`
/// immediately with no I/O. That refusal takes precedence over the platform result. Async hosts
/// call `request_local_inference_snapshot` and pass the owned snapshot into blocking inference work.
/// This function does not `block_on`, build a nested runtime, or join a worker. On a thread with
/// no entered runtime, platforms other than Windows and Linux return `Unsupported` with no I/O.
///
/// Admission is as for [`request_local_inference_snapshot`].
pub fn request_local_inference_snapshot_sync(
    socket_path: impl AsRef<Path>,
    deadline: Instant,
) -> Result<LocalInferenceSnapshot, LocalInferenceReadError> {
    let _ = socket_path;
    let runtime_entered = tokio::runtime::Handle::try_current().is_ok();
    let _nonce = gate_sync(runtime_entered, HANDOFF_SUPPORTED, fill_nonce)?;

    #[cfg(windows)]
    {
        use interprocess::ConnectWaitMode;
        use interprocess::local_socket::{ConnectOptions, ToFsName};
        use interprocess::os::windows::local_socket::NamedPipe;

        let socket_path = socket_path.as_ref();
        if Instant::now() >= deadline {
            return Err(LocalInferenceReadError::Timeout);
        }
        let name = crate::windows::pipe_name(socket_path)
            .and_then(|n| n.to_fs_name::<NamedPipe>())
            .map_err(|_| LocalInferenceReadError::Transport)?;
        let timeout = deadline.saturating_duration_since(Instant::now());
        if timeout.is_zero() {
            return Err(LocalInferenceReadError::Timeout);
        }
        let stream = ConnectOptions::new()
            .name(name)
            .wait_mode(ConnectWaitMode::Timeout(timeout))
            .nonblocking_stream(true)
            .connect_sync()
            .map_err(|error| {
                if Instant::now() >= deadline
                    || error.kind() == std::io::ErrorKind::TimedOut
                    // WaitNamedPipe reports ERROR_SEM_TIMEOUT when its wait expires.
                    || error.raw_os_error() == Some(121)
                {
                    LocalInferenceReadError::Timeout
                } else {
                    LocalInferenceReadError::Transport
                }
            })?;
        let secret = crate::windows::read_secret(socket_path)
            .map_err(|_| LocalInferenceReadError::Authentication)?;
        let correlation = 1_u64;

        crate::local_inference::exchange_after_admission(
            &mut DeadlinePipe {
                stream: &stream,
                deadline,
            },
            &mut DeadlinePipe {
                stream: &stream,
                deadline,
            },
            &secret,
            correlation,
            _nonce,
            deadline,
            Instant::now,
        )
    }
    // No runtime is entered on this thread, so a private one here is not nested.
    #[cfg(target_os = "linux")]
    {
        let _ = _nonce;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| LocalInferenceReadError::Transport)?;
        runtime.block_on(request_local_inference_snapshot(socket_path, deadline))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = deadline;
        Err(LocalInferenceReadError::Unsupported)
    }
}

/// Whether the peer of a connected Unix socket runs as this account.
///
/// The kernel records the peer's effective uid when it connects or listens;
/// nothing the peer sends can change it. Any error reads as another account.
#[cfg(target_os = "linux")]
pub(crate) fn peer_is_this_account(socket: &tokio::net::UnixStream) -> bool {
    socket
        .peer_cred()
        .is_ok_and(|credentials| credentials.uid() == nix::unistd::geteuid().as_raw())
}

/// One local-inference credential read: `(generation, port, token)`.
pub type LocalInferenceRefresh =
    Box<dyn FnMut(Instant) -> Result<(u64, u16, String), String> + Send>;

/// A credential refresh for the bundled local engine of the journal at `journal`.
///
/// Each call asks the supervisor over Callosum for the engine's current port and
/// per-launch token, within the caller's deadline and at most half a second. It is
/// `None` where the platform hands no credential over Callosum. A caller inside an
/// async runtime is served from a plain thread, because the synchronous request
/// refuses an entered runtime.
#[must_use]
pub fn local_inference_refresh(journal: &Path) -> Option<LocalInferenceRefresh> {
    if !HANDOFF_SUPPORTED {
        return None;
    }
    let socket = journal.join("health").join("callosum.sock");
    Some(Box::new(move |deadline: Instant| {
        let deadline = deadline.min(Instant::now() + std::time::Duration::from_millis(500));
        let request = || request_local_inference_snapshot_sync(&socket, deadline);
        let snapshot = if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| scope.spawn(request).join())
                .map_err(|_| "local inference request panicked".to_owned())?
        } else {
            request()
        }
        .map_err(|error| error.to_string())?;
        let token = String::from_utf8(snapshot.token().to_vec()).map_err(|e| e.to_string())?;
        Ok((snapshot.generation(), snapshot.port(), token))
    }))
}

#[allow(dead_code)]
pub(crate) async fn drive_exchange_async<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    session: &mut ExchangeSession,
) -> Result<LocalInferenceSnapshot, LocalInferenceReadError> {
    before_deadline(session.deadline(), async {
        let mut chunk = [0_u8; 128];
        loop {
            if let Some(snapshot) = session.advance(Instant::now())? {
                return Ok(snapshot);
            }

            if !session.is_request_written() {
                let to_write = session.request_to_write();
                match stream.write(to_write).await {
                    Ok(0) => return Err(LocalInferenceReadError::Transport),
                    Ok(n) => {
                        session.mark_written(n);
                        if let Some(snapshot) = session.advance(Instant::now())? {
                            return Ok(snapshot);
                        }
                        continue;
                    }
                    Err(_) => return Err(LocalInferenceReadError::Transport),
                }
            }

            match stream.read(&mut chunk).await {
                Ok(0) => {
                    return Err(LocalInferenceReadError::Transport);
                }
                Ok(n) => {
                    session.append_inbound_slice(&chunk[..n])?;
                    if let Some(snapshot) = session.advance(Instant::now())? {
                        return Ok(snapshot);
                    }
                }
                Err(_) => return Err(LocalInferenceReadError::Transport),
            }
        }
    })
    .await
}

async fn before_deadline<T>(
    deadline: Instant,
    operation: impl Future<Output = Result<T, LocalInferenceReadError>>,
) -> Result<T, LocalInferenceReadError> {
    if Instant::now() >= deadline {
        return Err(LocalInferenceReadError::Timeout);
    }
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), operation)
        .await
        .map_err(|_| LocalInferenceReadError::Timeout)?
}

// Windows nonblocking pipes may report zero until their peer posts a read/write.
// Keep those stalls outside the exchange parser, under its original deadline.
#[cfg(windows)]
struct DeadlinePipe<'a> {
    stream: &'a interprocess::local_socket::Stream,
    deadline: Instant,
}

#[cfg(windows)]
impl DeadlinePipe<'_> {
    fn retry(
        &self,
        mut operation: impl FnMut() -> std::io::Result<usize>,
    ) -> std::io::Result<usize> {
        loop {
            if Instant::now() >= self.deadline {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            match operation() {
                Ok(0) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                result => return result,
            }
            std::thread::sleep(
                std::time::Duration::from_millis(1)
                    .min(self.deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

#[cfg(windows)]
impl std::io::Read for DeadlinePipe<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let mut stream = self.stream;
        self.retry(|| std::io::Read::read(&mut stream, bytes))
    }
}

#[cfg(windows)]
impl std::io::Write for DeadlinePipe<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut stream = self.stream;
        self.retry(|| std::io::Write::write(&mut stream, bytes))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn fill_nonce() -> Result<[u8; NONCE_LEN], ()> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        let mut nonce = [0_u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| ())?;
        Ok(nonce)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Ok([0_u8; NONCE_LEN])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::ReadBuf;

    struct SilentPeer {
        stall_write: bool,
        write_calls: usize,
        read_calls: usize,
    }
    impl AsyncRead for SilentPeer {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            self.read_calls += 1;
            Poll::Pending
        }
    }
    impl AsyncWrite for SilentPeer {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.write_calls += 1;
            if self.stall_write {
                Poll::Pending
            } else {
                Poll::Ready(Ok(bytes.len()))
            }
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn pending_exchange_reads_and_writes_expire_without_peer_progress() {
        for stall_write in [true, false] {
            let deadline = tokio::time::Instant::now().into_std() + Duration::from_secs(5);
            let mut session = ExchangeSession::new([1; 32], 7, [2; 32], deadline);
            let mut peer = SilentPeer {
                stall_write,
                write_calls: 0,
                read_calls: 0,
            };
            assert_eq!(
                drive_exchange_async(&mut peer, &mut session)
                    .await
                    .unwrap_err(),
                LocalInferenceReadError::Timeout
            );
            assert!(peer.write_calls > 0);
            assert_eq!(peer.read_calls > 0, !stall_write);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn admission_deadline_expires_even_when_peer_never_greets() {
        let deadline = tokio::time::Instant::now().into_std() + Duration::from_secs(5);
        let mut peer = SilentPeer {
            stall_write: false,
            write_calls: 0,
            read_calls: 0,
        };
        let result: Result<(), LocalInferenceReadError> = before_deadline(deadline, async {
            let mut greeting = [0; 37];
            peer.read_exact(&mut greeting)
                .await
                .map_err(|_| LocalInferenceReadError::Transport)?;
            Ok(())
        })
        .await;
        assert_eq!(result, Err(LocalInferenceReadError::Timeout));
        assert!(peer.read_calls > 0);
    }
}
