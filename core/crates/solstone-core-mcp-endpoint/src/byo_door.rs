// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct BYO owner-hostname MCP ingress service.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_journal_config::{
    ByoHostnameConfigStatus, byo_hostname_config, read_journal_config,
};
use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_journal_io::{JsonWriteOptions, write_json};
use tokio::net::UnixListener;
use tokio::sync::{Semaphore, watch};
use tokio_rustls::TlsAcceptor;

use crate::byo_dns::resolve_byo_dns;
use crate::oauth::OAuthRuntime;
use crate::permits::try_acquire_connection_permit;
use crate::server::{RequestGuard, serve_stream};
use crate::session::SessionTable;
use crate::tls::{McpEndpointTlsService, mcp_endpoint_server_config};
use crate::unix::{self, ByoSocketBlocker};

pub(crate) const BYO_DOOR_STATE_PATH: &str = "mcp-endpoint/byo-door-state.json";
pub const DEFAULT_BYO_CONNECTION_PERMITS: usize = 64;

/// Non-secret status for the BYO owner-hostname door.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByoDoorState {
    pub hostname: Option<String>,
    pub enabled: bool,
    pub generation: u64,
    pub account_uri: Option<String>,
    pub caa: Option<String>,
    pub dns_verdict: Option<String>,
    pub dns_observed_at: Option<DateTime<Utc>>,
    pub socket_listening: bool,
    pub certificate_active: bool,
    pub socket_path: Option<String>,
    pub socket_blocker: Option<ByoSocketBlocker>,
    pub next_action: Option<String>,
    pub observed_at: DateTime<Utc>,
}

/// Read the BYO door state file.
pub fn read_byo_door_state(journal_root: &Path) -> Option<ByoDoorState> {
    let bytes = fs::read(journal_root.join(BYO_DOOR_STATE_PATH)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write the BYO door state file atomically with mode 0o600.
pub fn write_byo_door_state(journal_root: &Path, state: &ByoDoorState) {
    let path = journal_root.join(BYO_DOOR_STATE_PATH);
    let _ = write_json(
        path,
        state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    );
}

struct ByoBoundResources {
    ingress_task: tokio::task::JoinHandle<()>,
    acme_task: Option<tokio::task::JoinHandle<()>>,
    ingress_inode: u64,
}

struct ByoServiceRuntime {
    hostname: String,
    generation: u64,
    oauth: Arc<OAuthRuntime>,
    sessions: Arc<SessionTable>,
    permits: Arc<Semaphore>,
    tls_service: Arc<McpEndpointTlsService>,
    tls_config: Arc<rustls::ServerConfig>,
    account_dir: unix::TlsStateDirectory,
    service_shutdown_tx: watch::Sender<bool>,
    service_shutdown_rx: watch::Receiver<bool>,
    bound_resources: Option<ByoBoundResources>,
    last_dns_verdict: Option<crate::byo_dns::DnsVerdict>,
    last_dns_account_uri: Option<String>,
    last_dns_check: tokio::time::Instant,
}

impl ByoServiceRuntime {
    fn new(
        journal_root: &Path,
        byo_dir: &unix::ByoDirectory,
        hostname: String,
        generation: u64,
    ) -> Option<Self> {
        let account_dir = unix::open_byo_account_directory(byo_dir, &hostname).ok()?;
        let cert_dir = unix::open_byo_cert_directory(byo_dir, &hostname, generation).ok()?;
        let tls_service =
            McpEndpointTlsService::for_byo_cert_directory(cert_dir, hostname.clone()).ok()?;
        let tls_service = Arc::new(tls_service);
        let oauth = Arc::new(OAuthRuntime::new_byo(journal_root, &hostname, generation));
        let sessions = Arc::new(SessionTable::new());
        let permits = Arc::new(Semaphore::new(DEFAULT_BYO_CONNECTION_PERMITS));
        let tls_config = mcp_endpoint_server_config(&tls_service);
        let (service_shutdown_tx, service_shutdown_rx) = watch::channel(false);

        Some(Self {
            hostname,
            generation,
            oauth,
            sessions,
            permits,
            tls_service,
            tls_config,
            account_dir,
            service_shutdown_tx,
            service_shutdown_rx,
            bound_resources: None,
            last_dns_verdict: None,
            last_dns_account_uri: None,
            last_dns_check: tokio::time::Instant::now() - Duration::from_secs(120),
        })
    }

    fn unbind_ingress(&mut self, byo_dir: &unix::ByoDirectory) {
        if let Some(res) = self.bound_resources.take() {
            // Close accepted keep-alive streams too. Otherwise a client could
            // start another MCP request after CAA drift withdrew the socket.
            let _ = self.service_shutdown_tx.send(true);
            res.ingress_task.abort();
            if let Some(a) = res.acme_task {
                a.abort();
            }
            unix::unlink_byo_socket_if_inode_matches(
                byo_dir,
                unix::BYO_INGRESS_SOCKET,
                res.ingress_inode,
            );
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            self.service_shutdown_tx = shutdown_tx;
            self.service_shutdown_rx = shutdown_rx;
            self.sessions = Arc::new(SessionTable::new());
        }
    }

    fn stop(&mut self, byo_dir: &unix::ByoDirectory) {
        let _ = self.service_shutdown_tx.send(true);
        self.unbind_ingress(byo_dir);
    }

    async fn tick_service_admission(
        &mut self,
        journal_root: &Path,
        byo_dir: &unix::ByoDirectory,
        ingress_path: &Path,
        ingress_path_str: &str,
    ) {
        // 1. Account Check
        let account_uri = unix::read_byo_account_uri(&self.account_dir).ok().flatten();
        let account_key = unix::read_byo_account_key(&self.account_dir).ok().flatten();
        let key_valid = account_key
            .as_deref()
            .is_some_and(|bytes| crate::tls::validate_acme_account_key(bytes).is_ok());

        let (account_valid, next_action_account) = match (&account_uri, key_valid) {
            (Some(_), true) => (true, None),
            (Some(_), false) => (false, Some("account_key_lost".to_string())),
            _ => (false, Some("register_account".to_string())),
        };

        if !account_valid {
            self.unbind_ingress(byo_dir);
            write_byo_door_state(
                journal_root,
                &ByoDoorState {
                    hostname: Some(self.hostname.clone()),
                    enabled: true,
                    generation: self.generation,
                    account_uri,
                    caa: None,
                    dns_verdict: None,
                    dns_observed_at: None,
                    socket_listening: false,
                    certificate_active: false,
                    socket_path: None,
                    socket_blocker: None,
                    next_action: next_action_account,
                    observed_at: Utc::now(),
                },
            );
            return;
        }

        let uri_str = account_uri.clone().unwrap();

        // 2. DNS Check (recheck every 60s)
        if self.last_dns_account_uri.as_deref() != Some(uri_str.as_str()) {
            // A replacement ACME account has no inherited CAA verdict.
            self.unbind_ingress(byo_dir);
            self.last_dns_verdict = None;
            self.last_dns_account_uri = Some(uri_str.clone());
            write_byo_door_state(
                journal_root,
                &ByoDoorState {
                    hostname: Some(self.hostname.clone()),
                    enabled: true,
                    generation: self.generation,
                    account_uri: Some(uri_str.clone()),
                    caa: None,
                    dns_verdict: None,
                    dns_observed_at: None,
                    socket_listening: false,
                    certificate_active: false,
                    socket_path: None,
                    socket_blocker: None,
                    next_action: Some("publish_caa".to_string()),
                    observed_at: Utc::now(),
                },
            );
        }
        if self.last_dns_check.elapsed() >= Duration::from_secs(60)
            || self.last_dns_verdict.is_none()
        {
            let verdict = resolve_byo_dns(&self.hostname, &uri_str, Utc::now()).await;
            self.last_dns_verdict = Some(verdict);
            self.last_dns_account_uri = Some(uri_str.clone());
            self.last_dns_check = tokio::time::Instant::now();
        }

        let verdict = self.last_dns_verdict.clone().unwrap();
        let dns_fresh = (Utc::now() - verdict.observed_at) <= chrono::Duration::seconds(60);
        let dns_admitted = verdict.is_admitted() && dns_fresh;

        if !dns_admitted {
            self.unbind_ingress(byo_dir);
            write_byo_door_state(
                journal_root,
                &ByoDoorState {
                    hostname: Some(self.hostname.clone()),
                    enabled: true,
                    generation: self.generation,
                    account_uri: Some(uri_str),
                    caa: Some(verdict.code.as_str().to_string()),
                    dns_verdict: Some(verdict.code.as_str().to_string()),
                    dns_observed_at: Some(verdict.observed_at),
                    socket_listening: false,
                    certificate_active: false,
                    socket_path: None,
                    socket_blocker: None,
                    next_action: Some("publish_caa".to_string()),
                    observed_at: Utc::now(),
                },
            );
            return;
        }

        // 3. Both account and DNS are valid! Bind if not already bound.
        if self.bound_resources.is_none() {
            let acc_dir_clone = match unix::open_byo_account_directory(byo_dir, &self.hostname) {
                Ok(d) => d,
                Err(_) => {
                    self.last_dns_verdict = None;
                    return;
                }
            };
            let (std_ingress, ingress_inode) =
                match unix::bind_byo_socket(byo_dir, unix::BYO_INGRESS_SOCKET, ingress_path) {
                    Ok(p) => p,
                    Err((_, blocker)) => {
                        write_byo_door_state(
                            journal_root,
                            &ByoDoorState {
                                hostname: Some(self.hostname.clone()),
                                enabled: true,
                                generation: self.generation,
                                account_uri: Some(uri_str),
                                caa: None,
                                dns_verdict: Some(verdict.code.as_str().to_string()),
                                dns_observed_at: Some(verdict.observed_at),
                                socket_listening: false,
                                certificate_active: false,
                                socket_path: None,
                                socket_blocker: blocker,
                                next_action: Some("socket_blocked".to_string()),
                                observed_at: Utc::now(),
                            },
                        );
                        return;
                    }
                };

            let _ = std_ingress.set_nonblocking(true);

            let ingress_listener = match UnixListener::from_std(std_ingress) {
                Ok(l) => l,
                Err(_) => {
                    unix::unlink_byo_socket_if_inode_matches(
                        byo_dir,
                        unix::BYO_INGRESS_SOCKET,
                        ingress_inode,
                    );
                    return;
                }
            };

            // ACME renewal task
            let tls_clone = Arc::clone(&self.tls_service);
            let mut acme_shutdown = self.service_shutdown_rx.clone();
            let acme_task = tokio::spawn(async move {
                let _ = tls_clone
                    .run_byo_acme_renewal(acc_dir_clone, &mut acme_shutdown)
                    .await;
            });

            // Ingress accept task
            let ingress_root = Arc::new(journal_root.to_path_buf());
            let ingress_oauth = Arc::clone(&self.oauth);
            let ingress_sessions = Arc::clone(&self.sessions);
            let ingress_permits = Arc::clone(&self.permits);
            let ingress_tls_config = Arc::clone(&self.tls_config);
            let ingress_host_arc: Arc<str> = Arc::from(self.hostname.as_str());
            let mut ingress_shut = self.service_shutdown_rx.clone();

            let ingress_task = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = ingress_shut.changed() => break,
                        accepted = ingress_listener.accept() => {
                            let (stream, _) = match accepted {
                                Ok(conn) => conn,
                                Err(_) => {
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                    continue;
                                }
                            };
                            let permit = match try_acquire_connection_permit(&ingress_permits) {
                                Some(p) => p,
                                None => continue,
                            };
                            let conn_root = Arc::clone(&ingress_root);
                            let conn_oauth = Arc::clone(&ingress_oauth);
                            let conn_sessions = Arc::clone(&ingress_sessions);
                            let conn_tls_config = Arc::clone(&ingress_tls_config);
                            let conn_hostname = Arc::clone(&ingress_host_arc);
                            let conn_shutdown = ingress_shut.clone();

                            tokio::spawn(async move {
                                let _permit = permit;
                                let acceptor = TlsAcceptor::from(conn_tls_config);
                                let tls_stream = match tokio::time::timeout(Duration::from_secs(5), acceptor.accept(stream)).await {
                                    Ok(Ok(s)) => s,
                                    _ => return,
                                };
                                let dummy_source = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
                                let _ = serve_stream(
                                    tls_stream,
                                    conn_root,
                                    conn_oauth,
                                    conn_sessions,
                                    dummy_source,
                                    conn_shutdown,
                                    RequestGuard::ByoHostname { canonical_hostname: conn_hostname },
                                ).await;
                            });
                        }
                    }
                }
            });

            self.bound_resources = Some(ByoBoundResources {
                ingress_task,
                acme_task: Some(acme_task),
                ingress_inode,
            });
        }

        let cert_active = self.tls_service.ordinary_certificate_is_active();
        let next_action = if cert_active {
            Some("none".to_string())
        } else {
            Some("issue_certificate".to_string())
        };

        write_byo_door_state(
            journal_root,
            &ByoDoorState {
                hostname: Some(self.hostname.clone()),
                enabled: true,
                generation: self.generation,
                account_uri: Some(uri_str),
                caa: None,
                dns_verdict: Some(verdict.code.as_str().to_string()),
                dns_observed_at: Some(verdict.observed_at),
                socket_listening: true,
                certificate_active: cert_active,
                socket_path: Some(ingress_path_str.to_string()),
                socket_blocker: None,
                next_action,
                observed_at: Utc::now(),
            },
        );
    }
}

async fn tick_byo_admission(
    journal_root: &Path,
    byo_dir: &unix::ByoDirectory,
    runtime: &mut Option<ByoServiceRuntime>,
    ingress_path: &Path,
    ingress_path_str: &str,
) {
    let config = match read_journal_config(journal_root) {
        Ok(cfg) => byo_hostname_config(&cfg),
        Err(_) => ByoHostnameConfigStatus::Invalid,
    };

    match config {
        ByoHostnameConfigStatus::None | ByoHostnameConfigStatus::Invalid => {
            if let Some(mut rt) = runtime.take() {
                rt.stop(byo_dir);
            }
            write_byo_door_state(
                journal_root,
                &ByoDoorState {
                    hostname: None,
                    enabled: false,
                    generation: 0,
                    account_uri: None,
                    caa: None,
                    dns_verdict: None,
                    dns_observed_at: None,
                    socket_listening: false,
                    certificate_active: false,
                    socket_path: None,
                    socket_blocker: None,
                    next_action: None,
                    observed_at: Utc::now(),
                },
            );
        }
        ByoHostnameConfigStatus::Configured(cfg) => {
            if !cfg.enabled {
                if let Some(mut rt) = runtime.take() {
                    rt.stop(byo_dir);
                }
                write_byo_door_state(
                    journal_root,
                    &ByoDoorState {
                        hostname: cfg.hostname,
                        enabled: false,
                        generation: cfg.generation,
                        account_uri: None,
                        caa: None,
                        dns_verdict: None,
                        dns_observed_at: None,
                        socket_listening: false,
                        certificate_active: false,
                        socket_path: None,
                        socket_blocker: None,
                        next_action: None,
                        observed_at: Utc::now(),
                    },
                );
            } else {
                let hostname = match cfg.hostname {
                    Some(h) => h,
                    None => {
                        if let Some(mut rt) = runtime.take() {
                            rt.stop(byo_dir);
                        }
                        write_byo_door_state(
                            journal_root,
                            &ByoDoorState {
                                hostname: None,
                                enabled: true,
                                generation: cfg.generation,
                                account_uri: None,
                                caa: None,
                                dns_verdict: None,
                                dns_observed_at: None,
                                socket_listening: false,
                                certificate_active: false,
                                socket_path: None,
                                socket_blocker: None,
                                next_action: None,
                                observed_at: Utc::now(),
                            },
                        );
                        return;
                    }
                };

                let needs_restart = match runtime.as_ref() {
                    Some(rt) => rt.hostname != hostname || rt.generation != cfg.generation,
                    None => true,
                };

                if needs_restart {
                    if let Some(mut rt) = runtime.take() {
                        rt.stop(byo_dir);
                    }
                    *runtime =
                        ByoServiceRuntime::new(journal_root, byo_dir, hostname, cfg.generation);
                }

                if let Some(rt) = runtime.as_mut() {
                    rt.tick_service_admission(
                        journal_root,
                        byo_dir,
                        ingress_path,
                        ingress_path_str,
                    )
                    .await;
                }
            }
        }
    }
}

fn handle_cutover_stream(stream: &std::os::unix::net::UnixStream) -> bool {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let mut buf = [0u8; 16];
    if let Ok(n) = nix::unistd::read(stream, &mut buf) {
        let s = std::str::from_utf8(&buf[..n]).unwrap_or("");
        s.trim() == "apply"
    } else {
        false
    }
}

fn reply_cutover_ok(stream: &std::os::unix::net::UnixStream) {
    let _ = nix::unistd::write(stream, b"ok\n");
}

pub async fn run_byo_door_async(
    journal_root: PathBuf,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ()> {
    let root = JournalRoot::open(&journal_root).map_err(|_| ())?;
    let byo_dir = unix::open_byo_directory(&root).map_err(|_| ())?;
    let cutover_path = journal_root
        .join("mcp-endpoint")
        .join("byo")
        .join(unix::BYO_CUTOVER_SOCKET);
    let ingress_path = journal_root
        .join("mcp-endpoint")
        .join("byo")
        .join(unix::BYO_INGRESS_SOCKET);
    let ingress_path_str = ingress_path.to_string_lossy().to_string();

    let (std_cutover, cutover_inode) =
        unix::bind_byo_socket(&byo_dir, unix::BYO_CUTOVER_SOCKET, &cutover_path).map_err(|_| ())?;
    let _ = std_cutover.set_nonblocking(true);
    let cutover_listener = UnixListener::from_std(std_cutover).map_err(|_| ())?;

    let mut runtime: Option<ByoServiceRuntime> = None;
    let mut poll_interval = tokio::time::interval(Duration::from_millis(500));

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = poll_interval.tick() => {
                if *shutdown.borrow() {
                    break;
                }
                tick_byo_admission(&journal_root, &byo_dir, &mut runtime, &ingress_path, &ingress_path_str).await;
            }
            accepted = cutover_listener.accept() => {
                if let Ok((stream, _)) = accepted
                    && let Ok(std_stream) = stream.into_std()
                    && handle_cutover_stream(&std_stream)
                {
                    tick_byo_admission(&journal_root, &byo_dir, &mut runtime, &ingress_path, &ingress_path_str).await;
                    reply_cutover_ok(&std_stream);
                }
            }
        }
    }

    if let Some(mut rt) = runtime.take() {
        rt.stop(&byo_dir);
    }
    unix::unlink_byo_socket_if_inode_matches(&byo_dir, unix::BYO_CUTOVER_SOCKET, cutover_inode);
    write_byo_door_state(
        &journal_root,
        &ByoDoorState {
            hostname: None,
            enabled: false,
            generation: 0,
            account_uri: None,
            caa: None,
            dns_verdict: None,
            dns_observed_at: None,
            socket_listening: false,
            certificate_active: false,
            socket_path: None,
            socket_blocker: None,
            next_action: None,
            observed_at: Utc::now(),
        },
    );
    Ok(())
}

pub async fn run_single_byo_service(
    journal_root: PathBuf,
    hostname: String,
    generation: u64,
    mut shutdown: watch::Receiver<bool>,
) {
    let root = match JournalRoot::open(&journal_root) {
        Ok(r) => r,
        Err(_) => return,
    };

    let byo_dir = match unix::open_byo_directory(&root) {
        Ok(d) => d,
        Err(_) => return,
    };

    let ingress_path = journal_root
        .join("mcp-endpoint")
        .join("byo")
        .join(unix::BYO_INGRESS_SOCKET);
    let ingress_path_str = ingress_path.to_string_lossy().to_string();

    let mut runtime = match ByoServiceRuntime::new(&journal_root, &byo_dir, hostname, generation) {
        Some(rt) => rt,
        None => return,
    };

    let mut poll_interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = poll_interval.tick() => {
                if *shutdown.borrow() {
                    break;
                }
                runtime
                    .tick_service_admission(&journal_root, &byo_dir, &ingress_path, &ingress_path_str)
                    .await;
            }
        }
    }

    runtime.stop(&byo_dir);
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use solstone_core_journal_io::journal_root::JournalRoot;
    use tempfile::TempDir;

    use super::*;
    use crate::unix::{self, ByoSocketBlocker};

    fn test_journal() -> (TempDir, JournalRoot) {
        let dir = tempfile::Builder::new()
            .prefix("solstone-byo-state-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let root = JournalRoot::open(dir.path()).unwrap();
        (dir, root)
    }

    #[test]
    fn test_byo_door_state_roundtrip_and_permissions() {
        let (dir, _root) = test_journal();
        write_byo_door_state(
            dir.path(),
            &ByoDoorState {
                hostname: Some("mcp.example.com".to_string()),
                enabled: true,
                generation: 1,
                account_uri: Some("https://acme.example.com/acct/123".to_string()),
                caa: None,
                dns_verdict: Some("admitted".to_string()),
                dns_observed_at: Some(Utc::now()),
                socket_listening: true,
                certificate_active: true,
                socket_path: Some("/var/tmp/ingress.sock".to_string()),
                socket_blocker: None,
                next_action: None,
                observed_at: Utc::now(),
            },
        );

        let state = read_byo_door_state(dir.path()).expect("state reads back");
        assert_eq!(state.hostname.as_deref(), Some("mcp.example.com"));
        assert_eq!(state.generation, 1);
        assert!(state.socket_listening);
        assert!(state.certificate_active);

        // Verify mode 0o600
        let state_path = dir.path().join(BYO_DOOR_STATE_PATH);
        let meta = fs::metadata(&state_path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);

        // Verify write with blocker and caa_reason
        write_byo_door_state(
            dir.path(),
            &ByoDoorState {
                hostname: Some("mcp.example.com".to_string()),
                enabled: true,
                generation: 2,
                account_uri: None,
                caa: Some("caa_missing".to_string()),
                dns_verdict: Some("caa_missing".to_string()),
                dns_observed_at: Some(Utc::now()),
                socket_listening: false,
                certificate_active: false,
                socket_path: None,
                socket_blocker: Some(ByoSocketBlocker::Symlink),
                next_action: Some("fix_dns".to_string()),
                observed_at: Utc::now(),
            },
        );

        let state2 = read_byo_door_state(dir.path()).expect("state reads back");
        assert_eq!(state2.generation, 2);
        assert_eq!(state2.socket_blocker, Some(ByoSocketBlocker::Symlink));
        assert_eq!(state2.caa.as_deref(), Some("caa_missing"));
        assert_eq!(state2.next_action.as_deref(), Some("fix_dns"));
    }

    #[test]
    fn test_byo_account_directory_and_key_uri_persistence() {
        let (_dir, root) = test_journal();
        let byo_dir = unix::open_byo_directory(&root).expect("byo dir opens");
        let account_dir = unix::open_byo_account_directory(&byo_dir, "mcp.example.com")
            .expect("account dir opens");

        assert!(unix::read_byo_account_key(&account_dir).unwrap().is_none());
        assert!(unix::read_byo_account_uri(&account_dir).unwrap().is_none());

        let sample_key = vec![1, 2, 3, 4, 5];
        let sample_uri = "https://acme.example.com/acct/123";

        unix::persist_byo_account_key(&account_dir, &sample_key).unwrap();
        unix::persist_byo_account_uri(&account_dir, sample_uri).unwrap();

        assert_eq!(
            unix::read_byo_account_key(&account_dir).unwrap(),
            Some(sample_key)
        );
        assert_eq!(
            unix::read_byo_account_uri(&account_dir).unwrap().as_deref(),
            Some(sample_uri)
        );

        unix::delete_byo_account_pair(&account_dir).unwrap();
        assert!(unix::read_byo_account_key(&account_dir).unwrap().is_none());
        assert!(unix::read_byo_account_uri(&account_dir).unwrap().is_none());
    }

    #[test]
    fn test_byo_cert_directory_per_generation() {
        let (_dir, root) = test_journal();
        let byo_dir = unix::open_byo_directory(&root).expect("byo dir opens");
        let cert_dir_gen1 =
            unix::open_byo_cert_directory(&byo_dir, "mcp.example.com", 1).expect("gen1 opens");
        let cert_dir_gen2 =
            unix::open_byo_cert_directory(&byo_dir, "mcp.example.com", 2).expect("gen2 opens");

        // Persist dummy cert in gen1
        crate::unix::persist_byo_account_key(&cert_dir_gen1, b"gen1_data").unwrap();
        assert_eq!(
            crate::unix::read_byo_account_key(&cert_dir_gen1)
                .unwrap()
                .as_deref(),
            Some(b"gen1_data".as_slice())
        );
        // Gen2 should be empty
        assert!(
            crate::unix::read_byo_account_key(&cert_dir_gen2)
                .unwrap()
                .is_none()
        );
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod full_tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::Utc;
    use solstone_core_journal_io::journal_root::JournalRoot;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::watch;

    use super::*;
    use crate::byo_dns::{DnsVerdict, DnsVerdictCode};
    use crate::unix::{self, ByoSocketBlocker};

    // These fixtures deliberately replace process-wide DNS, registrar, and
    // clock inputs. Keep their complete lifetimes separate under parallel CI.
    static BYO_TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct TestOverridesGuard;
    impl Drop for TestOverridesGuard {
        fn drop(&mut self) {
            if let Ok(mut g) = crate::byo_dns::TEST_VERDICT_OVERRIDE.write() {
                *g = None;
            }
            if let Ok(mut g) = crate::owner_web::TEST_REGISTRAR.write() {
                *g = None;
            }
            crate::tls::set_test_now_override(None);
        }
    }

    fn test_journal() -> (TempDir, JournalRoot) {
        let dir = tempfile::Builder::new()
            .prefix("solstone-byo-full-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let root = JournalRoot::open(dir.path()).unwrap();
        (dir, root)
    }

    #[tokio::test]
    async fn test_byo_ingress_socket_bind_and_blockers() {
        let _serial = BYO_TEST_SERIAL.lock().await;
        let (dir, root) = test_journal();
        let byo_dir = unix::open_byo_directory(&root).expect("byo dir opens");
        let socket_path = dir.path().join("mcp-endpoint/byo/ingress.sock");

        // 1. Regular file blocker
        fs::File::create(&socket_path).unwrap();
        let bind_res = unix::bind_byo_socket(&byo_dir, unix::BYO_INGRESS_SOCKET, &socket_path);
        assert!(matches!(
            bind_res,
            Err((_, Some(ByoSocketBlocker::RegularFile)))
        ));
        fs::remove_file(&socket_path).unwrap();

        // 2. Symlink blocker: verify symlink is left in place
        let symlink_target = dir.path().join("dummy_target");
        fs::write(&symlink_target, b"target").unwrap();
        std::os::unix::fs::symlink(&symlink_target, &socket_path).unwrap();
        let bind_res2 = unix::bind_byo_socket(&byo_dir, unix::BYO_INGRESS_SOCKET, &socket_path);
        assert!(matches!(
            bind_res2,
            Err((_, Some(ByoSocketBlocker::Symlink)))
        ));
        assert!(
            fs::symlink_metadata(&socket_path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink must be left in place"
        );
        fs::remove_file(&socket_path).unwrap();

        // 3. Normal bind: mode 0o600 under 0o700 dir
        let (listener, inode) =
            unix::bind_byo_socket(&byo_dir, unix::BYO_INGRESS_SOCKET, &socket_path)
                .expect("bind succeeds");
        let sock_meta = fs::metadata(&socket_path).unwrap();
        assert_eq!(sock_meta.permissions().mode() & 0o777, 0o600);
        let parent_meta = fs::metadata(socket_path.parent().unwrap()).unwrap();
        assert_eq!(parent_meta.permissions().mode() & 0o777, 0o700);

        // 4. Second bind while live socket exists
        let bind_res3 = unix::bind_byo_socket(&byo_dir, unix::BYO_INGRESS_SOCKET, &socket_path);
        assert!(matches!(
            bind_res3,
            Err((_, Some(ByoSocketBlocker::LiveSocket)))
        ));

        drop(listener);

        // 5. Inode match unlink
        unix::unlink_byo_socket_if_inode_matches(&byo_dir, unix::BYO_INGRESS_SOCKET, inode);
        assert!(!socket_path.exists());
    }

    #[tokio::test]
    async fn test_byo_door_no_socket_without_valid_account_and_fresh_admitted_dns() {
        let _serial = BYO_TEST_SERIAL.lock().await;
        let _guard = TestOverridesGuard;
        let (dir, root) = test_journal();
        let journal_path = dir.path().to_path_buf();
        let byo_dir = unix::open_byo_directory(&root).unwrap();
        let acc_dir = unix::open_byo_account_directory(&byo_dir, "mcp.example.com").unwrap();
        let socket_path = journal_path.join("mcp-endpoint/byo/ingress.sock");

        // Case 1: Invalid account key -> validate fails
        assert!(crate::tls::validate_acme_account_key(b"not-der-key").is_err());

        // Case 2: Persist valid keypair and URI
        let keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let key_der = keypair.serialize_der();
        assert!(crate::tls::validate_acme_account_key(&key_der).is_ok());

        unix::persist_byo_account_key(&acc_dir, &key_der).unwrap();
        unix::persist_byo_account_uri(
            &acc_dir,
            "https://acme-v02.api.letsencrypt.org/acme/acct/12345",
        )
        .unwrap();

        // 3. DNS fixture unset or not admitted -> run_single_byo_service does not create ingress.sock
        {
            if let Ok(mut g) = crate::byo_dns::TEST_VERDICT_OVERRIDE.write() {
                *g = Some(DnsVerdict {
                    code: DnsVerdictCode::CaaMissing,
                    observed_at: Utc::now(),
                });
            }
            let (shutdown_send, shutdown_recv) = watch::channel(false);
            let root_clone = journal_path.clone();
            let handle = tokio::spawn(async move {
                run_single_byo_service(root_clone, "mcp.example.com".to_string(), 1, shutdown_recv)
                    .await;
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(
                !socket_path.exists(),
                "socket must not be created without admitted DNS"
            );
            let state = read_byo_door_state(&journal_path).expect("state written");
            assert!(!state.socket_listening);

            let _ = shutdown_send.send(true);
            let _ = handle.await;
        }

        // 4. Fresh admitted DNS fixture -> run_single_byo_service binds mode 0o600 under 0o700 directory
        {
            if let Ok(mut g) = crate::byo_dns::TEST_VERDICT_OVERRIDE.write() {
                *g = Some(DnsVerdict {
                    code: DnsVerdictCode::Admitted,
                    observed_at: Utc::now(),
                });
            }
            let (shutdown_send, shutdown_recv) = watch::channel(false);
            let root_clone = journal_path.clone();
            let handle = tokio::spawn(async move {
                run_single_byo_service(root_clone, "mcp.example.com".to_string(), 1, shutdown_recv)
                    .await;
            });
            for _ in 0..50 {
                if socket_path.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(
                socket_path.exists(),
                "socket must be created with admitted DNS and valid account"
            );
            let sock_meta = fs::metadata(&socket_path).unwrap();
            assert_eq!(sock_meta.permissions().mode() & 0o777, 0o600);
            let parent_meta = fs::metadata(socket_path.parent().unwrap()).unwrap();
            assert_eq!(parent_meta.permissions().mode() & 0o777, 0o700);

            let state = read_byo_door_state(&journal_path).expect("state written");
            assert!(state.socket_listening);

            let _ = shutdown_send.send(true);
            let _ = handle.await;
            assert!(!socket_path.exists(), "socket unlinked on shutdown");
        }
    }

    #[tokio::test]
    async fn test_byo_unix_socket_raw_tls_no_proxy_preface() {
        let _serial = BYO_TEST_SERIAL.lock().await;
        let _guard = TestOverridesGuard;
        let (dir, root) = test_journal();
        let journal_path = dir.path().to_path_buf();
        let byo_dir = unix::open_byo_directory(&root).unwrap();
        let acc_dir = unix::open_byo_account_directory(&byo_dir, "mcp.example.com").unwrap();
        let cert_dir = unix::open_byo_cert_directory(&byo_dir, "mcp.example.com", 1).unwrap();
        let socket_path = journal_path.join("mcp-endpoint/byo/ingress.sock");

        let base_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let not_before = base_now - 1000;
        let not_after = base_now + 10000;

        let keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params =
            rcgen::CertificateParams::new(vec!["mcp.example.com".to_string()]).unwrap();
        params.not_before = time::OffsetDateTime::from_unix_timestamp(not_before).unwrap();
        params.not_after = time::OffsetDateTime::from_unix_timestamp(not_after).unwrap();
        let cert = params.self_signed(&keypair).unwrap();
        let cert_der = cert.der().to_vec();
        let key_der = keypair.serialize_der();

        let acc_keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        unix::persist_byo_account_key(&acc_dir, &acc_keypair.serialize_der()).unwrap();
        unix::persist_byo_account_uri(
            &acc_dir,
            "https://acme-v02.api.letsencrypt.org/acme/acct/12345",
        )
        .unwrap();

        let tls_service = crate::tls::McpEndpointTlsService::for_byo_cert_directory(
            cert_dir,
            "mcp.example.com".to_string(),
        )
        .unwrap();
        crate::tls::set_test_now_override(Some(base_now));
        tls_service
            .install_ordinary_certificate(vec![cert_der.clone()], key_der, not_before, not_after)
            .unwrap();

        if let Ok(mut g) = crate::byo_dns::TEST_VERDICT_OVERRIDE.write() {
            *g = Some(DnsVerdict {
                code: DnsVerdictCode::Admitted,
                observed_at: Utc::now(),
            });
        }

        let (shutdown_send, shutdown_recv) = watch::channel(false);
        let root_clone = journal_path.clone();
        let handle = tokio::spawn(async move {
            run_single_byo_service(root_clone, "mcp.example.com".to_string(), 1, shutdown_recv)
                .await;
        });

        for _ in 0..50 {
            if socket_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(socket_path.exists());

        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(cert_der))
            .unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut client_config = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client_config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let client_config = Arc::new(client_config);

        // 1. Direct TLS ClientHello over Unix socket -> handshake completes without PROXY preface
        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let server_name =
            rustls::pki_types::ServerName::try_from("mcp.example.com".to_string()).unwrap();
        let mut tls_client = tokio_rustls::TlsConnector::from(client_config.clone())
            .connect(server_name, stream)
            .await
            .expect("TLS handshake completes without PROXY preface");
        tls_client.shutdown().await.unwrap();

        // 2. Client that writes PROXY preface first does not complete a handshake
        let mut stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        stream
            .write_all(b"PROXY TCP4 198.51.100.12 127.0.0.1 4321 443\r\n")
            .await
            .unwrap();
        let server_name =
            rustls::pki_types::ServerName::try_from("mcp.example.com".to_string()).unwrap();
        let handshake_res = tokio_rustls::TlsConnector::from(client_config)
            .connect(server_name, stream)
            .await;
        assert!(
            handshake_res.is_err(),
            "Handshake must fail if PROXY preface was prepended"
        );

        let _ = shutdown_send.send(true);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_byo_expired_certificate_inactive() {
        let _serial = BYO_TEST_SERIAL.lock().await;
        let _guard = TestOverridesGuard;
        let (_dir, root) = test_journal();
        let byo_dir = unix::open_byo_directory(&root).unwrap();
        let cert_dir = unix::open_byo_cert_directory(&byo_dir, "mcp.example.com", 1).unwrap();
        let tls_service = crate::tls::McpEndpointTlsService::for_byo_cert_directory(
            cert_dir,
            "mcp.example.com".to_string(),
        )
        .unwrap();

        let base_now = 1700000000_i64;
        let not_before = base_now - 1000;
        let not_after = base_now + 1000;
        crate::tls::set_test_now_override(Some(base_now));

        let keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params =
            rcgen::CertificateParams::new(vec!["mcp.example.com".to_string()]).unwrap();
        params.not_before = time::OffsetDateTime::from_unix_timestamp(not_before).unwrap();
        params.not_after = time::OffsetDateTime::from_unix_timestamp(not_after).unwrap();
        let cert = params.self_signed(&keypair).unwrap();
        let cert_der = cert.der().to_vec();
        let key_der = keypair.serialize_der();

        tls_service
            .install_ordinary_certificate(vec![cert_der], key_der, not_before, not_after)
            .unwrap();

        // 1. Within validity window
        assert!(tls_service.ordinary_certificate_is_active());

        // 2. Past validity window (advancing clock past not_after)
        crate::tls::set_test_now_override(Some(not_after + 100));
        assert!(!tls_service.ordinary_certificate_is_active());
    }

    #[tokio::test]
    async fn test_byo_cutover_barrier_disables_ingress_before_ok() {
        use tower::ServiceExt;

        let _serial = BYO_TEST_SERIAL.lock().await;
        let _guard = TestOverridesGuard;
        let (dir, root) = test_journal();
        let journal_path = dir.path().to_path_buf();
        let byo_dir = unix::open_byo_directory(&root).unwrap();
        let acc_dir = unix::open_byo_account_directory(&byo_dir, "mcp.example.com").unwrap();
        let socket_path = journal_path.join("mcp-endpoint/byo/ingress.sock");
        let cutover_path = journal_path.join("mcp-endpoint/byo/cutover.sock");

        // Write config enabled: true
        std::fs::create_dir_all(journal_path.join("config")).unwrap();
        std::fs::write(
            journal_path.join("config/journal.json"),
            r#"{"mcp_endpoint":{"byo_hostname":{"hostname":"mcp.example.com","enabled":true,"generation":1}}}"#,
        )
        .unwrap();

        // Valid account key and uri
        let keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        unix::persist_byo_account_key(&acc_dir, &keypair.serialize_der()).unwrap();
        unix::persist_byo_account_uri(
            &acc_dir,
            "https://acme-v02.api.letsencrypt.org/acme/acct/12345",
        )
        .unwrap();

        // Admitted DNS
        if let Ok(mut g) = crate::byo_dns::TEST_VERDICT_OVERRIDE.write() {
            *g = Some(DnsVerdict {
                code: DnsVerdictCode::Admitted,
                observed_at: Utc::now(),
            });
        }

        let (shutdown_send, shutdown_recv) = watch::channel(false);
        let root_clone = journal_path.clone();
        let handle = tokio::spawn(async move {
            let _ = run_byo_door_async(root_clone, shutdown_recv).await;
        });

        for _ in 0..50 {
            if socket_path.exists() && cutover_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(socket_path.exists(), "ingress socket must exist");
        assert!(cutover_path.exists(), "cutover socket must exist");
        let state = read_byo_door_state(&journal_path).expect("state written");
        assert!(state.socket_listening);

        // Now disable via owner_web PUT /app/agents/api/byo
        let mut request = axum::http::Request::builder()
            .method(axum::http::Method::PUT)
            .uri("/app/agents/api/byo")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"enabled":false}"#))
            .unwrap();
        request
            .extensions_mut()
            .insert(solstone_core_convey_http::identity::AccessBasis::Localhost);
        let response = crate::owner_web::owner_routes(journal_path.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        // Ingress socket MUST be gone immediately upon 200 OK response from cutover barrier
        assert!(
            !socket_path.exists(),
            "ingress socket must be disabled before cutover returns"
        );
        let state = read_byo_door_state(&journal_path).expect("state written");
        assert!(!state.socket_listening);

        let _ = shutdown_send.send(true);
        let _ = handle.await;
        assert!(
            !cutover_path.exists(),
            "cutover socket must be unlinked on shutdown"
        );
    }
}
