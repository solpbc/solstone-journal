// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixed journal-local forwarding for bridge-opened opaque byte streams.
//!
//! This module neither binds a port nor interprets the bytes it copies. Lane B
//! owns the one loopback listener; this side owns the bounded bridge session
//! and reconnect lifecycle only.

use std::path::Path;
use std::sync::Arc;

use ring::rand::{SecureRandom as _, SystemRandom};
use solstone_core_journal_config::MCP_ENDPOINT_LOOPBACK_PORT;
use tokio::net::TcpStream;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;
use tokio::time::{Duration, Instant, sleep, timeout, timeout_at};

use crate::{
    McpBridgeCarrierError, McpBridgeSession, McpEndpointOwnerContext, McpEndpointTlsService,
    McpPublicStream,
};

const PUBLIC_STREAM_TASK_LIMIT: usize = 255;
const LOOPBACK_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const COPY_BUFFER_BYTES: usize = 16 * 1024;
const CONNECTED_BACKOFF_RESET_AFTER: Duration = Duration::from_secs(60);
const FORWARDER_SHUTDOWN_BOUND: Duration = Duration::from_secs(2);

fn loopback_target() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], MCP_ENDPOINT_LOOPBACK_PORT))
}

/// Wait out one failed reconnect. A missing subscription holds for its fixed
/// delay and leaves the backoff alone; any other failure takes the short
/// jittered backoff and doubles it.
async fn wait_after_failed_connect(
    error: &McpBridgeCarrierError,
    journal_root: &Path,
    address: Option<&str>,
    legs: (&str, &str, &str),
    shutdown: &mut watch::Receiver<bool>,
    backoff_cap_seconds: &mut u64,
) {
    if let Some(delay) = crate::bridge_carrier::needs_subscription_retry(error) {
        let next_attempt = chrono::Utc::now()
            + chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::seconds(300));
        crate::owner_state::write_mcp_needs_subscription_state(
            journal_root,
            address,
            legs,
            next_attempt,
        );
        wait_for_fixed_delay(shutdown, delay).await;
    } else {
        wait_for_retry(shutdown, *backoff_cap_seconds).await;
        *backoff_cap_seconds = backoff_cap_seconds.saturating_mul(2).min(60);
    }
}

/// Run one bridge generation at a time until the supervisor requests shutdown.
pub(crate) async fn run(
    owner: &McpEndpointOwnerContext,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), McpBridgeCarrierError> {
    let mut backoff_cap_seconds = 1_u64;
    loop {
        if shutdown_requested(shutdown) {
            return Ok(());
        }
        let session = match owner.connect_mcp_bridge(shutdown).await {
            Ok(session) => session,
            Err(McpBridgeCarrierError::Cancelled) if shutdown_requested(shutdown) => return Ok(()),
            Err(ref error) => {
                wait_after_failed_connect(
                    error,
                    owner.journal_path(),
                    None,
                    ("waiting", "waiting", "waiting"),
                    shutdown,
                    &mut backoff_cap_seconds,
                )
                .await;
                continue;
            }
        };
        let connected_at = Instant::now();
        let _ = forward_generation(session, shutdown).await;
        if shutdown_requested(shutdown) {
            return Ok(());
        }
        if connected_at.elapsed() >= CONNECTED_BACKOFF_RESET_AFTER {
            backoff_cap_seconds = 1;
        }
        wait_for_retry(shutdown, backoff_cap_seconds).await;
        backoff_cap_seconds = (backoff_cap_seconds.saturating_mul(2)).min(60);
    }
}

/// Forward the initial paired session and recover later carriers only when
/// their account authority still names that exact TLS service.
///
/// This retains the listener and its certificate state through an ordinary
/// lease or transport loss. A reconnect cannot silently switch to a different
/// public hostname: the carrier is checked against `tls` before it can expose
/// a stream.
pub(crate) async fn run_bound_session(
    owner: McpEndpointOwnerContext,
    tls: Arc<McpEndpointTlsService>,
    initial_session: McpBridgeSession,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), McpBridgeCarrierError> {
    let mut backoff_cap_seconds = 1_u64;
    let mut next_session = Some(initial_session);
    loop {
        if shutdown_requested(shutdown) {
            return Ok(());
        }
        let session = match next_session.take() {
            Some(session) => session,
            None => {
                match owner
                    .connect_mcp_bridge_for_tls(tls.as_ref(), shutdown)
                    .await
                {
                    Ok(session) => session,
                    Err(McpBridgeCarrierError::Cancelled) if shutdown_requested(shutdown) => {
                        return Ok(());
                    }
                    Err(ref error) => {
                        wait_after_failed_connect(
                            error,
                            owner.journal_path(),
                            Some(tls.authorized_hostname()),
                            ("done", "done", "waiting"),
                            shutdown,
                            &mut backoff_cap_seconds,
                        )
                        .await;
                        continue;
                    }
                }
            }
        };
        let connected_at = Instant::now();
        let _ = forward_generation(session, shutdown).await;
        if shutdown_requested(shutdown) {
            return Ok(());
        }
        if connected_at.elapsed() >= CONNECTED_BACKOFF_RESET_AFTER {
            backoff_cap_seconds = 1;
        }
        wait_for_retry(shutdown, backoff_cap_seconds).await;
        backoff_cap_seconds = (backoff_cap_seconds.saturating_mul(2)).min(60);
    }
}

async fn forward_generation(
    session: McpBridgeSession,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), McpBridgeCarrierError> {
    let permits = Arc::new(Semaphore::new(PUBLIC_STREAM_TASK_LIMIT));
    let (generation_cancel, generation_shutdown) = watch::channel(false);
    let mut tasks = JoinSet::new();
    let generation_result = loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    break Ok(());
                }
            }
            accepted = session.accept_public() => {
                let stream = match accepted {
                    Ok(stream) => stream,
                    Err(error) => break Err(error),
                };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    // Dropping the opaque stream sends one SPL reset; the carrier
                    // and every sibling remain live.
                    drop(stream);
                    continue;
                };
                let task_shutdown = generation_shutdown.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    forward_stream(stream, task_shutdown).await;
                });
            }
            joined = tasks.join_next(), if !tasks.is_empty() => {
                let _ = joined;
            }
        }
    };
    generation_cancel.send_replace(true);
    let session_result = session.shutdown().await;
    join_generation_tasks(&mut tasks).await;
    generation_result.and(session_result)
}

async fn forward_stream(mut public: McpPublicStream, mut shutdown: watch::Receiver<bool>) {
    let loopback = tokio::select! {
        changed = shutdown.changed() => {
            let _ = changed;
            return;
        }
        connected = timeout(LOOPBACK_CONNECT_TIMEOUT, TcpStream::connect(loopback_target())) => {
            match connected {
                Ok(Ok(stream)) => stream,
                Ok(Err(_)) | Err(_) => return,
            }
        }
    };
    let mut loopback = loopback;
    let copy = tokio::io::copy_bidirectional_with_sizes(
        &mut public,
        &mut loopback,
        COPY_BUFFER_BYTES,
        COPY_BUFFER_BYTES,
    );
    tokio::pin!(copy);
    tokio::select! {
        changed = shutdown.changed() => {
            let _ = changed;
        }
        _ = &mut copy => {}
    }
}

async fn join_generation_tasks(tasks: &mut JoinSet<()>) {
    let deadline = Instant::now() + FORWARDER_SHUTDOWN_BOUND;
    while !tasks.is_empty() {
        if timeout_at(deadline, tasks.join_next()).await.is_err() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            return;
        }
    }
}

async fn wait_for_retry(shutdown: &mut watch::Receiver<bool>, cap_seconds: u64) {
    let delay = full_jitter_delay(cap_seconds);
    tokio::select! {
        changed = shutdown.changed() => {
            let _ = changed;
        }
        _ = sleep(delay) => {}
    }
}

async fn wait_for_fixed_delay(shutdown: &mut watch::Receiver<bool>, delay: Duration) {
    tokio::select! {
        changed = shutdown.changed() => {
            let _ = changed;
        }
        _ = sleep(delay) => {}
    }
}

fn full_jitter_delay(cap_seconds: u64) -> Duration {
    let cap_seconds = cap_seconds.clamp(1, 60);
    let limit = u64::MAX - (u64::MAX % cap_seconds);
    let rng = SystemRandom::new();
    for _ in 0..8 {
        let mut bytes = [0_u8; 8];
        if rng.fill(&mut bytes).is_err() {
            break;
        }
        let sample = u64::from_be_bytes(bytes);
        if sample < limit {
            return Duration::from_secs((sample % cap_seconds) + 1);
        }
    }
    Duration::from_secs(cap_seconds)
}

fn shutdown_requested(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    #[test]
    fn loopback_target_is_the_one_lane_b_listener() {
        assert_eq!(loopback_target(), "127.0.0.1:7658".parse().unwrap());
    }

    #[test]
    fn jitter_caps_are_closed_and_never_zero() {
        for cap in [0, 1, 2, 4, 8, 16, 32, 60, 61, u64::MAX] {
            let delay = full_jitter_delay(cap);
            assert!(delay >= Duration::from_secs(1));
            assert!(delay <= Duration::from_secs(60));
        }
    }

    fn journal_with_state_directory() -> tempfile::TempDir {
        let journal = tempfile::tempdir().expect("fixture journal");
        std::fs::create_dir_all(journal.path().join("mcp-endpoint")).expect("state directory");
        journal
    }

    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_missing_subscription_holds_a_reconnect_five_minutes_and_keeps_the_backoff() {
        let journal = journal_with_state_directory();
        let root = journal.path().to_path_buf();
        let (_keep, mut shutdown) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut backoff_cap_seconds = 4_u64;
            wait_after_failed_connect(
                &McpBridgeCarrierError::NeedsSubscription,
                &root,
                Some("aaaqeaye.solstone.me"),
                ("done", "done", "waiting"),
                &mut shutdown,
                &mut backoff_cap_seconds,
            )
            .await;
            backoff_cap_seconds
        });
        settle().await;
        let state = crate::owner_state::read_mcp_owner_state(journal.path()).expect("owner state");
        assert_eq!(state.status, "needs_subscription");
        assert_eq!(state.address.as_deref(), Some("aaaqeaye.solstone.me"));

        tokio::time::advance(Duration::from_secs(299)).await;
        settle().await;
        assert!(!task.is_finished(), "the hold ended inside five minutes");

        tokio::time::advance(Duration::from_secs(2)).await;
        settle().await;
        assert_eq!(task.await.expect("task"), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn any_other_reconnect_failure_keeps_the_short_backoff_and_doubles_it() {
        for (error, before, after) in [
            (McpBridgeCarrierError::Account, 1_u64, 2_u64),
            (McpBridgeCarrierError::Connect, 8, 16),
            (McpBridgeCarrierError::Tls, 60, 60),
        ] {
            let journal = journal_with_state_directory();
            let root = journal.path().to_path_buf();
            let (_keep, mut shutdown) = watch::channel(false);
            let mut backoff_cap_seconds = before;
            tokio::time::timeout(
                Duration::from_secs(61),
                wait_after_failed_connect(
                    &error,
                    &root,
                    None,
                    ("waiting", "waiting", "waiting"),
                    &mut shutdown,
                    &mut backoff_cap_seconds,
                ),
            )
            .await
            .expect("a short backoff never exceeds sixty seconds");
            assert_eq!(backoff_cap_seconds, after, "{error}");
            assert!(
                crate::owner_state::read_mcp_owner_state(journal.path()).is_none(),
                "{error} wrote a subscription state"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_shutdown_request_ends_the_subscription_hold() {
        let journal = journal_with_state_directory();
        let root = journal.path().to_path_buf();
        let (send, mut shutdown) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut backoff_cap_seconds = 1_u64;
            wait_after_failed_connect(
                &McpBridgeCarrierError::NeedsSubscription,
                &root,
                None,
                ("waiting", "waiting", "waiting"),
                &mut shutdown,
                &mut backoff_cap_seconds,
            )
            .await;
        });
        settle().await;
        assert!(!task.is_finished());
        send.send_replace(true);
        settle().await;
        assert!(task.is_finished());
    }
}
