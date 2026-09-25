// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct loopback MCP door service.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_convey_http::listener::{LoopbackListeners, bind_loopback};
use solstone_core_journal_config::{
    LocalDoorConfig, MCP_LOCAL_DOOR_ORIGIN, MCP_LOCAL_DOOR_PORT, local_door_config,
    read_journal_config,
};
use solstone_core_journal_io::{JsonWriteOptions, write_json};
use solstone_core_system::lifecycle::{
    HostedServiceParentRuntime, HostedServiceShutdownEvidence, ParentLossReason,
};
use tokio::sync::{Semaphore, oneshot, watch};

use crate::McpServiceError;
use crate::oauth::OAuthRuntime;
use crate::permits::try_acquire_connection_permit;
use crate::server::{RequestGuard, serve_stream};
use crate::session::SessionTable;

pub(crate) const LOCAL_DOOR_STATE_PATH: &str = "mcp-endpoint/local-door-state.json";

/// Execution options for running the local door loop.
#[derive(Debug, Clone)]
pub struct LocalDoorRun {
    pub port: u16,
    pub origin: String,
    pub config_interval: Duration,
    pub rewrite_interval: Duration,
    pub bind_retry_interval: Duration,
    pub connection_permits: usize,
    #[cfg(test)]
    pub fail_accept_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl LocalDoorRun {
    pub fn production() -> Self {
        Self {
            port: MCP_LOCAL_DOOR_PORT,
            origin: MCP_LOCAL_DOOR_ORIGIN.to_string(),
            config_interval: Duration::from_secs(1),
            rewrite_interval: Duration::from_secs(10),
            bind_retry_interval: Duration::from_secs(2),
            connection_permits: 256,
            #[cfg(test)]
            fail_accept_flag: None,
        }
    }
}

/// Non-secret status for the local loopback door.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalDoorState {
    pub listening: bool,
    pub observed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Read the local door state file.
pub fn read_local_door_state(journal_root: &Path) -> Option<LocalDoorState> {
    let bytes = std::fs::read(journal_root.join(LOCAL_DOOR_STATE_PATH)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write the local door state file atomically with mode 0o600.
pub fn write_local_door_state(journal_root: &Path, listening: bool, reason: Option<&str>) {
    // a crash leaves the last record; until that record ages out of the reader window a turn-off can reach its wait bound, and the agents page can keep showing the door open until the owner reloads.
    let state = LocalDoorState {
        listening,
        observed_at: Utc::now(),
        reason: reason.map(str::to_owned),
    };
    let path = journal_root.join(LOCAL_DOOR_STATE_PATH);
    if let Err(_error) = write_json(
        path,
        &state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    ) {
        log::error!("failed to write local door state");
    }
}

/// Run the native local door service with an optional hosted parent runtime.
pub fn run_local_door_with_hosted_parent(
    journal_root: PathBuf,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), McpServiceError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("mcp-local-door")
        .build()
        .map_err(|_| McpServiceError::Runtime)?;
    runtime.block_on(run_local_door_async(
        journal_root,
        LocalDoorRun::production(),
        hosted_parent,
    ))
}

pub async fn run_local_door_async(
    journal_root: PathBuf,
    run: LocalDoorRun,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), McpServiceError> {
    crate::rlimit::apply_soft_nofile_limit();
    let (shutdown_send, shutdown_receive) = watch::channel(false);
    let signal_task = tokio::spawn(wait_for_shutdown_signal(shutdown_send.clone()));
    let (parent_loss_send, mut parent_loss_receive) = oneshot::channel();
    let parent_task = hosted_parent.as_ref().map(|parent| {
        tokio::spawn(wait_for_hosted_parent(
            Arc::clone(parent),
            shutdown_send.clone(),
            parent_loss_send,
        ))
    });

    let lan_oauth = Arc::new(OAuthRuntime::new_lan_door(&journal_root));
    let lan_root = journal_root.clone();
    let lan_shutdown = shutdown_receive.clone();
    let lan_task = tokio::spawn(async move {
        let _ = crate::lan_door::run_lan_door_async(lan_root, lan_oauth, lan_shutdown).await;
    });

    let result = run_local_door_loop(&journal_root, run, shutdown_receive).await;
    let _ = lan_task.await;
    let service_stopped = result.is_ok();
    signal_task.abort();
    let _ = signal_task.await;
    if let Some(parent_task) = parent_task {
        parent_task.abort();
        let _ = parent_task.await;
    }
    write_local_door_state(&journal_root, false, Some("not_running"));
    crate::lan_door::write_lan_door_state(&journal_root, false, Some("not_running"), None, None);
    if let Some(parent) = hosted_parent {
        let reason = parent_loss_receive.try_recv().ok().or_else(|| {
            parent
                .retire_expected_requested()
                .then_some(ParentLossReason::ExitedOrReused)
        });
        if reason.is_some() {
            parent
                .finish_parent_loss(HostedServiceShutdownEvidence {
                    listener_stopped: service_stopped,
                    service_runner_stopped: service_stopped,
                    operational_artifacts_cleaned: true,
                })
                .map_err(|_| McpServiceError::ParentLoss)?;
        }
    }
    result
}

#[cfg(test)]
pub static TEST_FORCE_LOCAL_SHUTDOWN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

async fn wait_for_shutdown_signal(shutdown_send: watch::Sender<bool>) {
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal");
    let mut sigint =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).expect("signal");
    #[cfg(test)]
    {
        loop {
            tokio::select! {
                _ = sigterm.recv() => break,
                _ = sigint.recv() => break,
                _ = tokio::time::sleep(Duration::from_millis(20)) => {
                    if TEST_FORCE_LOCAL_SHUTDOWN.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
    }
    #[cfg(not(test))]
    {
        tokio::select! {
            _ = sigterm.recv() => {}
            _ = sigint.recv() => {}
        }
    }
    let _ = shutdown_send.send(true);
}

async fn wait_for_hosted_parent(
    parent: Arc<HostedServiceParentRuntime>,
    shutdown_send: watch::Sender<bool>,
    parent_loss_send: oneshot::Sender<ParentLossReason>,
) {
    let reason = parent.await_parent_loss().await;
    let _ = parent_loss_send.send(reason);
    let _ = shutdown_send.send(true);
}

struct ActiveListener {
    _period_shutdown_send: watch::Sender<bool>,
    accept_task: tokio::task::JoinHandle<()>,
}

pub async fn run_local_door_loop(
    journal_root: &Path,
    run: LocalDoorRun,
    mut shutdown_receive: watch::Receiver<bool>,
) -> Result<(), McpServiceError> {
    let mut active_listener: Option<ActiveListener> = None;
    let sessions = Arc::new(SessionTable::new());
    let permits = Arc::new(Semaphore::new(run.connection_permits));
    let journal_root_arc = Arc::new(journal_root.to_path_buf());

    let mut last_readable_config: Option<LocalDoorConfig> = None;
    let mut current_state: Option<(bool, Option<String>)> = None;
    let mut last_write_at = Instant::now();
    let mut next_bind_attempt = Instant::now();

    loop {
        if *shutdown_receive.borrow() {
            break;
        }

        let config_result = read_journal_config(journal_root);
        let config = match config_result {
            Ok(read) => {
                let parsed = local_door_config(&read);
                last_readable_config = Some(parsed);
                parsed
            }
            Err(_) => {
                if last_readable_config.is_some() {
                    tokio::select! {
                        changed = shutdown_receive.changed() => {
                            if changed.is_err() || *shutdown_receive.borrow_and_update() {
                                break;
                            }
                        }
                        () = tokio::time::sleep(run.config_interval) => {}
                    }
                    continue;
                } else {
                    // No readable config seen yet
                    if let Some(active) = active_listener.take() {
                        drop(active._period_shutdown_send);
                        let _ = active.accept_task.await;
                    }
                    let new_snapshot = (false, Some("config_unreadable".to_owned()));
                    if current_state.as_ref() != Some(&new_snapshot)
                        || last_write_at.elapsed() >= run.rewrite_interval
                    {
                        write_local_door_state(journal_root, false, Some("config_unreadable"));
                        current_state = Some(new_snapshot);
                        last_write_at = Instant::now();
                    }
                    let sleep_dur = run
                        .config_interval
                        .min(run.rewrite_interval.saturating_sub(last_write_at.elapsed()));
                    tokio::select! {
                        changed = shutdown_receive.changed() => {
                            if changed.is_err() || *shutdown_receive.borrow_and_update() {
                                break;
                            }
                        }
                        () = tokio::time::sleep(sleep_dur) => {}
                    }
                    continue;
                }
            }
        };

        match config {
            LocalDoorConfig::Off => {
                if let Some(active) = active_listener.take() {
                    drop(active._period_shutdown_send);
                    let _ = active.accept_task.await;
                }
                let new_snapshot = (false, Some("disabled".to_owned()));
                if current_state.as_ref() != Some(&new_snapshot)
                    || last_write_at.elapsed() >= run.rewrite_interval
                {
                    write_local_door_state(journal_root, false, Some("disabled"));
                    current_state = Some(new_snapshot);
                    last_write_at = Instant::now();
                }
            }
            LocalDoorConfig::Invalid => {
                if let Some(active) = active_listener.take() {
                    drop(active._period_shutdown_send);
                    let _ = active.accept_task.await;
                }
                let new_snapshot = (false, Some("config_invalid".to_owned()));
                if current_state.as_ref() != Some(&new_snapshot)
                    || last_write_at.elapsed() >= run.rewrite_interval
                {
                    write_local_door_state(journal_root, false, Some("config_invalid"));
                    current_state = Some(new_snapshot);
                    last_write_at = Instant::now();
                }
            }
            LocalDoorConfig::On => {
                if let Some(active) = active_listener.as_ref() {
                    if active.accept_task.is_finished() {
                        // Accept loop died (accept error or fail flag)
                        let active = active_listener.take().unwrap();
                        drop(active._period_shutdown_send);
                        let _ = active.accept_task.await;
                        write_local_door_state(journal_root, false, Some("not_running"));
                        current_state = Some((false, Some("not_running".to_owned())));
                        last_write_at = Instant::now();
                        next_bind_attempt = Instant::now() + run.bind_retry_interval;
                    } else {
                        // Both listeners held and accept loop running
                        let new_snapshot = (true, None);
                        if current_state.as_ref() != Some(&new_snapshot)
                            || last_write_at.elapsed() >= run.rewrite_interval
                        {
                            write_local_door_state(journal_root, true, None);
                            current_state = Some(new_snapshot);
                            last_write_at = Instant::now();
                        }
                    }
                }

                if active_listener.is_none() {
                    if Instant::now() >= next_bind_attempt {
                        // another process can bind 7659 while this journal is down and show a lookalike consent page. The owner's check is the journal mark on the consent page and beside the code in the agents app. A journal with no identity yet shows a generic mark a lookalike can copy.
                        match bind_loopback(run.port).await {
                            Ok(listeners) => {
                                write_local_door_state(journal_root, true, None);
                                current_state = Some((true, None));
                                last_write_at = Instant::now();
                                let (period_send, period_receive) = watch::channel(false);
                                let sessions_clone = Arc::clone(&sessions);
                                let permits_clone = Arc::clone(&permits);
                                let root_clone = Arc::clone(&journal_root_arc);
                                let oauth = Arc::new(OAuthRuntime::new_bound(
                                    journal_root,
                                    run.origin.clone(),
                                ));
                                #[cfg(test)]
                                let fail_flag = run.fail_accept_flag.clone();
                                let task = tokio::spawn(async move {
                                    run_accept_loop(
                                        listeners,
                                        root_clone,
                                        oauth,
                                        sessions_clone,
                                        permits_clone,
                                        period_receive,
                                        #[cfg(test)]
                                        fail_flag,
                                    )
                                    .await;
                                });
                                active_listener = Some(ActiveListener {
                                    _period_shutdown_send: period_send,
                                    accept_task: task,
                                });
                            }
                            Err(err) => {
                                let reason = if err.kind() == std::io::ErrorKind::AddrInUse {
                                    // a second login's door records port_in_use while the first login holds the port. Agents of the second login reach the first login's journal. The pairing code still gates consent.
                                    "port_in_use"
                                } else {
                                    "bind_failed"
                                };
                                write_local_door_state(journal_root, false, Some(reason));
                                current_state = Some((false, Some(reason.to_owned())));
                                last_write_at = Instant::now();
                                next_bind_attempt = Instant::now() + run.bind_retry_interval;
                            }
                        }
                    } else if last_write_at.elapsed() >= run.rewrite_interval {
                        // Rewrite state during retry wait
                        if let Some((listening, reason)) = &current_state {
                            write_local_door_state(journal_root, *listening, reason.as_deref());
                            last_write_at = Instant::now();
                        }
                    }
                }
            }
        }

        let now = Instant::now();
        let time_to_rewrite = run
            .rewrite_interval
            .saturating_sub(now.duration_since(last_write_at));
        let time_to_retry = next_bind_attempt.saturating_duration_since(now);
        let mut sleep_dur = run.config_interval.min(time_to_rewrite);
        if active_listener.is_none() && time_to_retry > Duration::ZERO {
            sleep_dur = sleep_dur.min(time_to_retry);
        }

        tokio::select! {
            changed = shutdown_receive.changed() => {
                if changed.is_err() || *shutdown_receive.borrow_and_update() {
                    break;
                }
            }
            () = tokio::time::sleep(sleep_dur) => {}
        }
    }

    if let Some(active) = active_listener.take() {
        drop(active._period_shutdown_send);
        let _ = active.accept_task.await;
    }
    write_local_door_state(journal_root, false, Some("not_running"));
    Ok(())
}

async fn run_accept_loop(
    listeners: LoopbackListeners,
    journal_root: Arc<PathBuf>,
    oauth: Arc<OAuthRuntime>,
    sessions: Arc<SessionTable>,
    permits: Arc<Semaphore>,
    mut period_shutdown: watch::Receiver<bool>,
    #[cfg(test)] fail_accept_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
) {
    loop {
        #[cfg(test)]
        if let Some(flag) = &fail_accept_flag
            && flag.load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        if *period_shutdown.borrow() {
            return;
        }
        tokio::select! {
            changed = period_shutdown.changed() => {
                let _ = changed;
                return;
            }
            accepted = listeners.accept_peer() => {
                #[cfg(test)]
                if let Some(flag) = &fail_accept_flag
                    && flag.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                let Ok((socket, peer_addr)) = accepted else {
                    // Accept error: leave loop, drop listeners, record not_running
                    return;
                };
                let Some(permit) = try_acquire_connection_permit(&permits) else {
                    // Permit exhaustion still drops that socket and keeps accepting
                    drop(socket);
                    continue;
                };
                let connection_root = Arc::clone(&journal_root);
                let connection_oauth = Arc::clone(&oauth);
                let connection_sessions = Arc::clone(&sessions);
                let connection_shutdown = period_shutdown.clone();
                let source = peer_addr.ip();
                tokio::spawn(async move {
                    let _permit = permit;
                    // 127.0.0.1 and ::1 are two registration sources. Each source is capped at 16 clients, the store at 1024, and registering still evicts the oldest idle client of that source before the oldest idle client overall.
                    let _ = serve_stream(
                        socket,
                        connection_root,
                        connection_oauth,
                        connection_sessions,
                        source,
                        connection_shutdown,
                        RequestGuard::Loopback,
                    )
                    .await;
                });
            }
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod full_tests {
    use super::*;
    use base64::Engine as _;
    use rusqlite::params;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::watch;

    use crate::oauth::store::OAuthStore;
    use crate::oauth::urlparse::query_value_encode;

    const OAUTH_VERIFIER: &str =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

    fn pkce_challenge() -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(OAUTH_VERIFIER.as_bytes()))
    }

    fn seed_indexed_note(journal: &Path) {
        let path = "20260831/default/123456_1/talents/brief.md";
        let source = journal.join("chronicle/20260831/default/123456_1");
        fs::create_dir_all(source.join("talents")).expect("fixture segment directory");
        fs::write(source.join("talents/brief.md"), "disk fixture source")
            .expect("fixture source content");
        fs::write(source.join("talents/facets.json"), "[]").expect("fixture assignments");
        let connection =
            solstone_core_indexer_store::db::open_index(journal).expect("fixture index opens");
        connection
            .execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?1, ?2, '20260831', '', 'fixture', 'default', 0, '')",
                params!["MCP fixture search needle", path],
            )
            .expect("fixture chunk inserts");
        connection
            .execute(
                "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'complete', 1, 1)",
                [],
            )
            .expect("fixture index completes");
        connection
            .execute(
                "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
                params![path],
            )
            .expect("fixture classification inserts");
        connection
            .execute(
                "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) VALUES (1, '', 1, 0, NULL, 0)",
                [],
            )
            .expect("fixture classification completes");
    }

    async fn get_free_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        port
    }

    async fn exchange_plain_http(addr: &str, raw_request: &str) -> (u16, Vec<u8>, String) {
        let mut socket = TcpStream::connect(addr).await.unwrap();
        socket.write_all(raw_request.as_bytes()).await.unwrap();
        let mut head = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            socket.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            if head.len() > 65536 {
                panic!("response headers too large");
            }
        }
        let head_str = String::from_utf8_lossy(&head).to_string();
        let status = head_str
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let content_length = head_str
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("content-length:") {
                    Some(
                        line.split_once(':')
                            .unwrap()
                            .1
                            .trim()
                            .parse::<usize>()
                            .unwrap(),
                    )
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let mut body = vec![0_u8; content_length];
        if content_length > 0 {
            socket.read_exact(&mut body).await.unwrap();
        }
        (status, body, head_str)
    }

    fn hidden_transaction_id(body: &str) -> String {
        let marker = "name=\"transaction_id\" value=\"";
        let start = body.find(marker).expect("hidden transaction_id") + marker.len();
        let end = start + body[start..].find('"').expect("value terminator");
        body[start..end].to_owned()
    }

    fn location_param(location: &str, name: &str) -> String {
        let query = location.split_once('?').expect("redirect has query").1;
        let prefix = format!("{name}=");
        query
            .split('&')
            .find_map(|piece| piece.strip_prefix(&prefix))
            .expect("parameter present")
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn local_door_end_to_end_plain_flow() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path().to_path_buf();
        fs::create_dir_all(journal_root.join("config")).unwrap();
        fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();
        seed_indexed_note(&journal_root);

        let port = get_free_port().await;
        let origin = format!("http://127.0.0.1:{port}");
        let run = LocalDoorRun {
            port,
            origin: origin.clone(),
            config_interval: Duration::from_millis(50),
            rewrite_interval: Duration::from_secs(10),
            bind_retry_interval: Duration::from_millis(100),
            connection_permits: 256,
            fail_accept_flag: None,
        };

        let (shutdown_send, shutdown_receive) = watch::channel(false);
        let root_clone = journal_root.clone();
        let server_task = tokio::spawn(async move {
            let _ = run_local_door_loop(&root_clone, run, shutdown_receive).await;
        });

        // Wait for door to start listening
        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && state.listening
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let addr = format!("127.0.0.1:{port}");

        // 1. Unauthenticated POST /mcp -> 401 with resource_metadata header
        let (status, _, head) = exchange_plain_http(
            &addr,
            &format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"),
        )
        .await;
        assert_eq!(status, 401);
        let auth_hdr = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("www-authenticate:"))
            .unwrap();
        assert!(auth_hdr.contains(&format!(
            "resource_metadata=\"{origin}/.well-known/oauth-protected-resource\""
        )));

        // 2. GET /.well-known/oauth-protected-resource
        let (status, body, _) = exchange_plain_http(
            &addr,
            &format!("GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let pr: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(pr["resource"], format!("{origin}/mcp"));
        assert_eq!(pr["authorization_servers"], json!([origin]));

        // 3. GET /.well-known/oauth-authorization-server
        let (status, body, _) = exchange_plain_http(
            &addr,
            &format!("GET /.well-known/oauth-authorization-server HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let as_meta: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(as_meta["issuer"], origin);

        // 4. DCR: POST /register
        let reg_payload = serde_json::to_vec(&json!({
            "redirect_uris": ["http://localhost:12345/callback"],
            "token_endpoint_auth_method": "none",
            "client_name": "Local Door Test",
        }))
        .unwrap();
        let (status, body, _) = exchange_plain_http(
            &addr,
            &format!(
                "POST /register HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                reg_payload.len(),
                std::str::from_utf8(&reg_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 201);
        let reg_res: Value = serde_json::from_slice(&body).unwrap();
        let client_id = reg_res["client_id"].as_str().unwrap().to_owned();
        assert!(client_id.starts_with("oauth:dcr:"));

        // 5. GET /authorize consent page
        let challenge = pkce_challenge();
        let query = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&resource={}/mcp&state=st_local",
            query_value_encode(&client_id),
            query_value_encode("http://localhost:12345/callback"),
            query_value_encode(&challenge),
            query_value_encode(&origin),
        );
        let (status, body, _) = exchange_plain_http(
            &addr,
            &format!("GET /authorize?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let html = String::from_utf8(body).unwrap();
        let tx_id = hidden_transaction_id(&html);

        // 6. POST /authorize with pairing code
        let store = OAuthStore::open(&journal_root);
        let pairing = store.generate_pairing_code().unwrap();
        let auth_form = format!(
            "transaction_id={}&pairing_code={}&scope=whole_journal&category=transcripts",
            query_value_encode(&tx_id),
            query_value_encode(&pairing.code),
        );
        let (status, _, head) = exchange_plain_http(
            &addr,
            &format!(
                "POST /authorize HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{auth_form}",
                auth_form.len()
            ),
        )
        .await;
        assert_eq!(status, 302);
        let location = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("location:"))
            .unwrap();
        assert!(location.contains(&format!("iss={}", query_value_encode(&origin))));
        let code = location_param(location, "code");

        // 7. POST /token to exchange authorization_code
        let token_form = format!(
            "grant_type=authorization_code&code={}&client_id={}&code_verifier={}&redirect_uri={}&resource={}/mcp",
            query_value_encode(&code),
            query_value_encode(&client_id),
            query_value_encode(OAUTH_VERIFIER),
            query_value_encode("http://localhost:12345/callback"),
            query_value_encode(&origin),
        );
        let (status, body, _) = exchange_plain_http(
            &addr,
            &format!(
                "POST /token HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{token_form}",
                token_form.len()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let token_res: Value = serde_json::from_slice(&body).unwrap();
        let _access_token = token_res["access_token"].as_str().unwrap().to_owned();
        let refresh_token = token_res["refresh_token"].as_str().unwrap().to_owned();

        // 8. POST /token refresh
        let refresh_form = format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}&resource={}/mcp",
            query_value_encode(&refresh_token),
            query_value_encode(&client_id),
            query_value_encode(&origin),
        );
        let (status, body, _) = exchange_plain_http(
            &addr,
            &format!(
                "POST /token HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{refresh_form}",
                refresh_form.len()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let refreshed: Value = serde_json::from_slice(&body).unwrap();
        let active_access_token = refreshed["access_token"].as_str().unwrap().to_owned();

        // 9. POST /mcp initialize
        let init_payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }))
        .unwrap();
        let (status, _, head) = exchange_plain_http(
            &addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {active_access_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                init_payload.len(),
                std::str::from_utf8(&init_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let session_id = head
            .lines()
            .find_map(|l| l.strip_prefix("Mcp-Session-Id: "))
            .unwrap()
            .trim()
            .to_owned();

        // 10. POST /mcp tools/list -> verify readOnlyHint
        let list_payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .unwrap();
        let (status, body, _) = exchange_plain_http(
            &addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {active_access_token}\r\nMcp-Session-Id: {session_id}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                list_payload.len(),
                std::str::from_utf8(&list_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let list_res: Value = serde_json::from_slice(&body).unwrap();
        let tools = list_res["result"]["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
        for tool in tools {
            let read_only = tool.get("readOnlyHint") == Some(&Value::Bool(true))
                || tool.get("annotations").and_then(|a| a.get("readOnlyHint"))
                    == Some(&Value::Bool(true));
            assert!(read_only, "tool {} must declare readOnlyHint", tool["name"]);
        }

        // 11. POST /mcp tools/call search -> verify outcome & admission audit
        let call_payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {"query": "needle", "limit": 1}
            }
        }))
        .unwrap();
        let (status, body, _) = exchange_plain_http(
            &addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {active_access_token}\r\nMcp-Session-Id: {session_id}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                call_payload.len(),
                std::str::from_utf8(&call_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let call_res: Value = serde_json::from_slice(&body).unwrap();
        assert!(call_res["result"].is_object());

        // Verify audit interaction was recorded
        let audit_count = fs::read_dir(journal_root.join("chronicle"))
            .unwrap()
            .flat_map(|day| {
                fs::read_dir(day.unwrap().path().join("mcp.agent"))
                    .ok()
                    .into_iter()
                    .flatten()
            })
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .join("interaction.json")
                    .is_file()
            })
            .count();
        assert!(audit_count >= 1);

        // 12. Host: localhost:<port> still names http://127.0.0.1:<port>
        let (status, body, _) = exchange_plain_http(
            &addr,
            &format!("GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let pr_localhost: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(pr_localhost["resource"], format!("{origin}/mcp"));

        // Cleanup
        let _ = shutdown_send.send(true);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn local_door_ipv6_and_port_in_use() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path().to_path_buf();
        fs::create_dir_all(journal_root.join("config")).unwrap();
        fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

        let port = get_free_port().await;
        let v6_holder = std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, port)).unwrap();

        let fail_flag = Arc::new(AtomicBool::new(false));
        let run = LocalDoorRun {
            port,
            origin: format!("http://127.0.0.1:{port}"),
            config_interval: Duration::from_millis(50),
            rewrite_interval: Duration::from_millis(50),
            bind_retry_interval: Duration::from_millis(100),
            connection_permits: 256,
            fail_accept_flag: Some(Arc::clone(&fail_flag)),
        };

        let (shutdown_send, shutdown_receive) = watch::channel(false);
        let root_clone = journal_root.clone();
        let server_task = tokio::spawn(async move {
            let _ = run_local_door_loop(&root_clone, run, shutdown_receive).await;
        });

        // 1. Check three rewrite intervals with strictly later observed_at and reason port_in_use
        let mut observed_timestamps = Vec::new();
        for _ in 0..15 {
            if let Some(state) = read_local_door_state(&journal_root)
                && !state.listening
                && state.reason.as_deref() == Some("port_in_use")
                && !observed_timestamps.contains(&state.observed_at)
            {
                observed_timestamps.push(state.observed_at);
            }
            if observed_timestamps.len() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert!(
            observed_timestamps.len() >= 3,
            "expected at least 3 rewrites of port_in_use"
        );
        for window in observed_timestamps.windows(2) {
            assert!(
                window[1] > window[0],
                "timestamps must be strictly increasing"
            );
        }

        // 2. Check reader window reports port_in_use
        let val = crate::owner_web::state_value(&journal_root).unwrap();
        assert_eq!(val["local_door"]["listening"], false);
        assert_eq!(val["local_door"]["reason"], "port_in_use");

        // 3. Release [::1]:<port> holder
        drop(v6_holder);

        // Within retry interval, door binds dual-stack and writes listening: true
        let mut listening_bound = false;
        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && state.listening
                && state.reason.is_none()
            {
                listening_bound = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(listening_bound, "door must bind after holder is released");

        // Both 127.0.0.1 and [::1] accept connections
        let v4_client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await;
        assert!(v4_client.is_ok(), "IPv4 connects to door");
        let v6_client = TcpStream::connect((Ipv6Addr::LOCALHOST, port)).await;
        assert!(v6_client.is_ok(), "IPv6 connects to door");

        // 4. Set fail_accept_flag -> accept loop dies, next bind is no sooner than retry interval
        fail_flag.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await; // trigger accept

        let start_time = tokio::time::Instant::now();
        let mut noticed_drop = false;
        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && !state.listening
            {
                noticed_drop = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(noticed_drop);

        // Turn fail_flag off
        fail_flag.store(false, Ordering::SeqCst);

        // Wait for rebind
        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && state.listening
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            start_time.elapsed() >= Duration::from_millis(80),
            "retry took at least retry interval"
        );

        let _ = shutdown_send.send(true);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn local_door_loopback_guard_rejections() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path().to_path_buf();
        fs::create_dir_all(journal_root.join("config")).unwrap();
        fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

        let port = get_free_port().await;
        let origin = format!("http://127.0.0.1:{port}");
        let run = LocalDoorRun {
            port,
            origin: origin.clone(),
            config_interval: Duration::from_millis(50),
            rewrite_interval: Duration::from_secs(10),
            bind_retry_interval: Duration::from_millis(100),
            connection_permits: 256,
            fail_accept_flag: None,
        };

        let (shutdown_send, shutdown_receive) = watch::channel(false);
        let root_clone = journal_root.clone();
        let server_task = tokio::spawn(async move {
            let _ = run_local_door_loop(&root_clone, run, shutdown_receive).await;
        });

        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && state.listening
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let addr = format!("127.0.0.1:{port}");

        let two_hosts = format!(
            "GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nHost: evil.example\r\n\r\n"
        );

        // Test guard rejections over plain HTTP
        let cases = [
            (
                "GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: evil.example\r\n\r\n",
                403,
                "host_not_allowed",
            ),
            (
                "GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: 127.0.0.1.evil.example\r\n\r\n",
                403,
                "host_not_allowed",
            ),
            (
                "GET /.well-known/oauth-protected-resource HTTP/1.1\r\n\r\n",
                403,
                "host_not_allowed",
            ),
            (two_hosts.as_str(), 403, "host_not_allowed"),
            (
                "POST http://evil.example/token HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n",
                403,
                "host_not_allowed",
            ),
            (
                "GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: \x01\r\n\r\n",
                403,
                "host_not_allowed",
            ),
            (
                "GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: foo\rbar\r\n\r\n",
                403,
                "host_not_allowed",
            ),
            (
                "POST /token HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\nContent-Length: 0\r\n\r\n",
                403,
                "cross_origin_blocked",
            ),
            (
                "POST /authorize HTTP/1.1\r\nHost: 127.0.0.1\r\nSec-Fetch-Site: cross-site\r\nContent-Length: 0\r\n\r\n",
                403,
                "cross_origin_blocked",
            ),
        ];

        for (req, expected_status, expected_body) in cases {
            let (status, body, _) = exchange_plain_http(&addr, req).await;
            assert_eq!(status, expected_status, "request: {req}");
            assert_eq!(
                String::from_utf8_lossy(&body).trim(),
                expected_body,
                "request: {req}"
            );
        }

        // POST /token with neither header is admitted (not loopback refused)
        let (status, _, _) = exchange_plain_http(
            &addr,
            &format!("POST /token HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\n\r\n"),
        )
        .await;
        assert_ne!(
            status, 403,
            "POST /token without origin/sec-fetch-site is admitted"
        );

        let _ = shutdown_send.send(true);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn local_door_unserved_routes_404() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path().to_path_buf();
        fs::create_dir_all(journal_root.join("config")).unwrap();
        fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

        let port = get_free_port().await;
        let origin = format!("http://127.0.0.1:{port}");
        let run = LocalDoorRun {
            port,
            origin: origin.clone(),
            config_interval: Duration::from_millis(50),
            rewrite_interval: Duration::from_secs(10),
            bind_retry_interval: Duration::from_millis(100),
            connection_permits: 256,
            fail_accept_flag: None,
        };

        let (shutdown_send, shutdown_receive) = watch::channel(false);
        let root_clone = journal_root.clone();
        let server_task = tokio::spawn(async move {
            let _ = run_local_door_loop(&root_clone, run, shutdown_receive).await;
        });

        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && state.listening
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let addr = format!("127.0.0.1:{port}");
        for path in ["/", "/app/agents", "/app/agents/api/state"] {
            let (status, _, _) = exchange_plain_http(
                &addr,
                &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            )
            .await;
            assert_eq!(status, 404, "path {path} must return 404 on local door");
        }

        let _ = shutdown_send.send(true);
        let _ = server_task.await;
    }

    fn create_test_token(
        runtime: &OAuthRuntime,
        redirect_uri: &str,
        resource: &str,
    ) -> (String, String) {
        let client_id = "oauth:dcr:test-token-client";
        let client = runtime
            .store
            .register_client_with_random(
                client_id,
                vec![redirect_uri.to_string()],
                None,
                "127.0.0.1",
                &crate::tokens::SystemRandomSource,
            )
            .unwrap();
        let challenge = pkce_challenge();
        let pairing = runtime.store.generate_pairing_code().unwrap();
        let tx = runtime
            .store
            .create_transaction(
                &client.id,
                redirect_uri,
                resource,
                runtime.fixed_resource_origin(),
                &challenge,
                "S256",
                None,
                "127.0.0.1",
            )
            .unwrap();
        let auth = runtime
            .store
            .complete_pairing_with_permission(&tx, &pairing.code, None, &runtime.binding())
            .unwrap();
        let tokens = runtime
            .store
            .redeem_authorization_code(
                &auth.code,
                &client.client_id,
                redirect_uri,
                resource,
                OAUTH_VERIFIER,
                &runtime.binding(),
            )
            .unwrap();
        (client.client_id, tokens.access_token)
    }

    #[tokio::test]
    async fn local_door_and_relay_token_isolation() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path().to_path_buf();
        fs::create_dir_all(journal_root.join("config")).unwrap();
        fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();
        seed_indexed_note(&journal_root);

        let port = get_free_port().await;
        let origin = format!("http://127.0.0.1:{port}");

        // Create bound local door token
        let local_runtime = OAuthRuntime::new_bound(&journal_root, origin.clone());
        let (_, local_token) = create_test_token(
            &local_runtime,
            "http://localhost:12345/callback",
            &format!("{origin}/mcp"),
        );

        // Create unbound relay token
        let relay_runtime = OAuthRuntime::new(&journal_root, "https://mcp.test".to_string());
        let (_, relay_token) = create_test_token(
            &relay_runtime,
            "http://localhost:12345/callback",
            "https://mcp.test/mcp",
        );

        // Start local door
        let run = LocalDoorRun {
            port,
            origin: origin.clone(),
            config_interval: Duration::from_millis(50),
            rewrite_interval: Duration::from_secs(10),
            bind_retry_interval: Duration::from_millis(100),
            connection_permits: 256,
            fail_accept_flag: None,
        };
        let (shutdown_send, shutdown_receive) = watch::channel(false);
        let root_clone = journal_root.clone();
        let server_task = tokio::spawn(async move {
            let _ = run_local_door_loop(&root_clone, run, shutdown_receive).await;
        });

        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && state.listening
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let addr = format!("127.0.0.1:{port}");

        // Test 1: Relay token on local door -> 401
        let init_body =
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))
                .unwrap();
        let (status, _, _) = exchange_plain_http(
            &addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {relay_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                init_body.len(),
                std::str::from_utf8(&init_body).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 401, "relay token must be 401 on local door");

        // Test 2: Local token on local door -> 200
        let (status, _, _) = exchange_plain_http(
            &addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {local_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                init_body.len(),
                std::str::from_utf8(&init_body).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 200, "local door token must succeed on local door");

        let _ = shutdown_send.send(true);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn local_door_config_dynamic_reconfiguration() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path().to_path_buf();
        fs::create_dir_all(journal_root.join("config")).unwrap();
        fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

        let port = get_free_port().await;
        let origin = format!("http://127.0.0.1:{port}");

        // Start with local_door: false
        fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"local_door":false}}"#,
        )
        .unwrap();

        let run = LocalDoorRun {
            port,
            origin: origin.clone(),
            config_interval: Duration::from_millis(50),
            rewrite_interval: Duration::from_secs(10),
            bind_retry_interval: Duration::from_millis(100),
            connection_permits: 256,
            fail_accept_flag: None,
        };

        let (shutdown_send, shutdown_receive) = watch::channel(false);
        let root_clone = journal_root.clone();
        let server_task = tokio::spawn(async move {
            let _ = run_local_door_loop(&root_clone, run, shutdown_receive).await;
        });

        // Verify state is disabled
        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && !state.listening
                && state.reason.as_deref() == Some("disabled")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            read_local_door_state(&journal_root)
                .unwrap()
                .reason
                .as_deref(),
            Some("disabled")
        );

        // Unparseable JSON -> stays disabled
        fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint": {"local_door": "#,
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(
            read_local_door_state(&journal_root)
                .unwrap()
                .reason
                .as_deref(),
            Some("disabled")
        );

        // Write local_door: true -> becomes listening
        fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"local_door":true}}"#,
        )
        .unwrap();
        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && state.listening
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(read_local_door_state(&journal_root).unwrap().listening);

        // Unparseable JSON -> stays listening (keeps earlier readable config)
        fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint": {"local_door": "#,
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(read_local_door_state(&journal_root).unwrap().listening);

        // Turn off: local_door: false -> refused within one config interval
        fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"local_door":false}}"#,
        )
        .unwrap();
        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && !state.listening
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            read_local_door_state(&journal_root)
                .unwrap()
                .reason
                .as_deref(),
            Some("disabled")
        );

        // Refuses connects to 127.0.0.1 and [::1]
        assert!(
            TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
        assert!(
            TcpStream::connect((Ipv6Addr::LOCALHOST, port))
                .await
                .is_err()
        );

        let _ = shutdown_send.send(true);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn local_door_oauth_revocation() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path().to_path_buf();
        fs::create_dir_all(journal_root.join("config")).unwrap();
        fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();
        seed_indexed_note(&journal_root);

        let port = get_free_port().await;
        let origin = format!("http://127.0.0.1:{port}");

        let runtime = OAuthRuntime::new_bound(&journal_root, origin.clone());
        let (client_id, token) = create_test_token(
            &runtime,
            "http://localhost:12345/callback",
            &format!("{origin}/mcp"),
        );

        let run = LocalDoorRun {
            port,
            origin: origin.clone(),
            config_interval: Duration::from_millis(50),
            rewrite_interval: Duration::from_secs(10),
            bind_retry_interval: Duration::from_millis(100),
            connection_permits: 256,
            fail_accept_flag: None,
        };
        let (shutdown_send, shutdown_receive) = watch::channel(false);
        let root_clone = journal_root.clone();
        let server_task = tokio::spawn(async move {
            let _ = run_local_door_loop(&root_clone, run, shutdown_receive).await;
        });

        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && state.listening
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let addr = format!("127.0.0.1:{port}");

        // Initialize session
        let init_body =
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))
                .unwrap();
        let (status, _, head) = exchange_plain_http(
            &addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                init_body.len(),
                std::str::from_utf8(&init_body).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let session_id = head
            .lines()
            .find_map(|l| l.strip_prefix("Mcp-Session-Id: "))
            .unwrap()
            .trim()
            .to_owned();

        // Revoke client grant
        runtime
            .store
            .revoke_client_by_client_id(&client_id)
            .unwrap();

        // Next tools/call returns 401
        let call_body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "search", "arguments": {"query": "test"}}
        }))
        .unwrap();
        let (status, _, _) = exchange_plain_http(
            &addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nMcp-Session-Id: {session_id}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                call_body.len(),
                std::str::from_utf8(&call_body).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 401, "revoked token returns 401");

        let _ = shutdown_send.send(true);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn local_door_parent_loss_and_signal() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path().to_path_buf();
        fs::create_dir_all(journal_root.join("config")).unwrap();
        fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

        let port = get_free_port().await;
        let origin = format!("http://127.0.0.1:{port}");

        let run = LocalDoorRun {
            port,
            origin: origin.clone(),
            config_interval: Duration::from_millis(50),
            rewrite_interval: Duration::from_secs(10),
            bind_retry_interval: Duration::from_millis(100),
            connection_permits: 256,
            fail_accept_flag: None,
        };

        let (shutdown_send, shutdown_receive) = watch::channel(false);
        let root_clone = journal_root.clone();
        let server_task = tokio::spawn(async move {
            let _ = run_local_door_loop(&root_clone, run, shutdown_receive).await;
        });

        for _ in 0..50 {
            if let Some(state) = read_local_door_state(&journal_root)
                && state.listening
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Trigger shutdown (simulates SIGTERM or parent loss)
        let start = std::time::Instant::now();
        let _ = shutdown_send.send(true);
        let _ = server_task.await;
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "shutdown completed within 2 seconds"
        );

        // Record is not_running
        assert_eq!(
            read_local_door_state(&journal_root)
                .unwrap()
                .reason
                .as_deref(),
            Some("not_running")
        );

        // Port can be bound again immediately
        let bind_again = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port));
        assert!(
            bind_again.is_ok(),
            "port must be released and bindable immediately"
        );
    }
}
