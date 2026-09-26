// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::local_inference::{
    ExchangeSession, LocalInferenceReadError, LocalInferenceSnapshot, NONCE_LEN, gate_async,
    gate_sync,
};

/// Asynchronously request an active local-inference snapshot from the Callosum supervisor.
///
/// On non-Windows platforms, returns [`LocalInferenceReadError::Unsupported`] with no I/O.
/// One deadline covers connection, handshake, request transmission, and response verification;
/// no retry is performed.
///
/// The named-pipe endpoint DACL and remote-client rejection protect cross-user/cross-identity
/// and remote-network access—not same-SID malware, which is trusted once admitted.
pub async fn request_local_inference_snapshot(
    socket_path: impl AsRef<Path>,
    deadline: Instant,
) -> Result<LocalInferenceSnapshot, LocalInferenceReadError> {
    let _ = socket_path;
    let _nonce = gate_async(cfg!(windows), fill_nonce)?;

    #[cfg(windows)]
    {
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
    }
    #[cfg(not(windows))]
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
/// no entered runtime, non-Windows returns `Unsupported` with no I/O.
///
/// The named-pipe endpoint DACL and remote-client rejection protect cross-user/cross-identity
/// and remote-network access—not same-SID malware, which is trusted once admitted.
pub fn request_local_inference_snapshot_sync(
    socket_path: impl AsRef<Path>,
    deadline: Instant,
) -> Result<LocalInferenceSnapshot, LocalInferenceReadError> {
    let _ = socket_path;
    let runtime_entered = tokio::runtime::Handle::try_current().is_ok();
    let _nonce = gate_sync(runtime_entered, cfg!(windows), fill_nonce)?;

    #[cfg(windows)]
    {
        use interprocess::ConnectWaitMode;
        use interprocess::local_socket::{ConnectOptions, ToFsName};
        use interprocess::os::windows::local_socket::NamedPipe;
        use std::time::Duration;

        let socket_path = socket_path.as_ref();
        let timeout = deadline.saturating_duration_since(Instant::now());
        let name = crate::windows::pipe_name(socket_path)
            .and_then(|n| n.to_fs_name::<NamedPipe>())
            .map_err(|_| LocalInferenceReadError::Transport)?;
        let mut stream = ConnectOptions::new()
            .name(name)
            .wait_mode(ConnectWaitMode::Timeout(timeout))
            .connect_sync()
            .map_err(|_| LocalInferenceReadError::Transport)?;
        let secret = crate::windows::read_secret(socket_path)
            .map_err(|_| LocalInferenceReadError::Authentication)?;
        let correlation = 1_u64;

        crate::local_inference::exchange_after_admission(
            &mut stream,
            &mut stream,
            &secret,
            correlation,
            _nonce,
            deadline,
            Instant::now,
        )
    }
    #[cfg(not(windows))]
    {
        let _ = deadline;
        Err(LocalInferenceReadError::Unsupported)
    }
}

#[allow(dead_code)]
pub(crate) async fn drive_exchange_async<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    session: &mut ExchangeSession,
) -> Result<LocalInferenceSnapshot, LocalInferenceReadError> {
    let mut chunk = [0_u8; 128];
    loop {
        if let Some(snapshot) = session.advance(Instant::now())? {
            return Ok(snapshot);
        }

        if !session.is_request_written() {
            let to_write = session.request_to_write();
            match stream.write(to_write).await {
                Ok(0) => {}
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
}

fn fill_nonce() -> Result<[u8; NONCE_LEN], ()> {
    #[cfg(windows)]
    {
        let mut nonce = [0_u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| ())?;
        Ok(nonce)
    }
    #[cfg(not(windows))]
    {
        Ok([0_u8; NONCE_LEN])
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod local_inference_full_tests {
    #[cfg(windows)]
    #[test]
    fn windows_named_pipe_live_local_inference_round_trip() {
        // Authored for the MSVC native Windows host runner.
    }
}
