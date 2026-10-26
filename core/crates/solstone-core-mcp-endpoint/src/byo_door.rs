// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct BYO owner-hostname MCP ingress service.
//!
//! The owner's forwarder delivers raw TLS to an ingress the journal binds
//! only while the hostname is admitted: a socket in the journal's endpoint
//! directory on desktop Unix, and a loopback TCP port on Windows and mobile.
//! Forwarders deliver the same TLS stream on every platform.

use crate::door_state_writer::DoorStateWriter;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::byo_dns::resolve_byo_dns;
use crate::oauth::OAuthRuntime;
use crate::permits::try_acquire_connection_permit;
use crate::server::{RequestGuard, serve_stream};
use crate::session::SessionTable;
use crate::tls::{McpEndpointTlsService, mcp_endpoint_server_config};
use crate::unix;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_journal_config::{
    ByoHostnameConfigStatus, ByoRelayConfig, byo_hostname_config, byo_relay_config,
    read_journal_config,
};
use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_journal_io::{JsonWriteOptions, write_json};
use tokio::sync::{Semaphore, watch};
use tokio_rustls::TlsAcceptor;

/// Why the owner-hostname ingress could not be bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ByoSocketBlocker {
    LiveSocket,
    Symlink,
    RegularFile,
    InodeReplaced,
    PathTooLong,
    /// Another program already listens on the TCP ingress port.
    PortInUse,
}

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
    pub relay_state: String,
    pub relay_detail: Option<String>,
    pub relay_observed_at: Option<DateTime<Utc>>,
    pub dns_addresses: Vec<String>,
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

/// Where the ingress listens, as the owner's forwarder addresses it.
pub(crate) fn ingress_address(journal_root: &Path) -> String {
    ingress::address(journal_root)
}

/// The Unix ingress: a socket under the owner-only `byo/` directory.
#[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
mod ingress {
    use super::*;
    use tokio::net::UnixListener;

    pub(super) type Listener = UnixListener;
    /// The bound socket's inode, so release removes only our own entry.
    pub(super) type Bound = u64;

    fn path(journal_root: &Path) -> PathBuf {
        journal_root
            .join("mcp-endpoint")
            .join(unix::BYO_DIRECTORY)
            .join(unix::BYO_INGRESS_SOCKET)
    }

    pub(super) fn address(journal_root: &Path) -> String {
        path(journal_root).to_string_lossy().to_string()
    }

    pub(super) fn bind(
        byo_dir: &unix::ByoDirectory,
        journal_root: &Path,
    ) -> Result<(Listener, Bound), Option<ByoSocketBlocker>> {
        let (std_ingress, inode) =
            unix::bind_byo_socket(byo_dir, unix::BYO_INGRESS_SOCKET, &path(journal_root))
                .map_err(|(_, blocker)| blocker)?;
        let _ = std_ingress.set_nonblocking(true);
        match UnixListener::from_std(std_ingress) {
            Ok(listener) => Ok((listener, inode)),
            Err(_) => {
                unix::unlink_byo_socket_if_inode_matches(byo_dir, unix::BYO_INGRESS_SOCKET, inode);
                Err(None)
            }
        }
    }

    pub(super) fn release(byo_dir: &unix::ByoDirectory, inode: Bound) {
        unix::unlink_byo_socket_if_inode_matches(byo_dir, unix::BYO_INGRESS_SOCKET, inode);
    }
}

/// The TCP ingress for Windows and mobile app containers. It is bound like the other
/// doors' listeners, without `SO_REUSEADDR`, so no other program can take the
/// port over while the journal holds it, and it closes when the accept task
/// that owns it ends.
#[cfg(any(
    windows,
    target_os = "ios",
    target_os = "android",
    all(test, feature = "full-tests")
))]
mod tcp_ingress {
    use super::*;
    use solstone_core_journal_config::MCP_BYO_INGRESS_PORT;
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::net::TcpListener;

    pub(super) type Listener = TcpListener;
    pub(super) type Bound = ();

    pub(super) fn address(_journal_root: &Path) -> String {
        format!("127.0.0.1:{MCP_BYO_INGRESS_PORT}")
    }

    pub(super) fn bind(
        _byo_dir: &unix::ByoDirectory,
        _journal_root: &Path,
    ) -> Result<(Listener, Bound), Option<ByoSocketBlocker>> {
        crate::lan_door::bind_admitted_address(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            MCP_BYO_INGRESS_PORT,
        )
        .map(|listener| (listener, ()))
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied => {
                Some(ByoSocketBlocker::PortInUse)
            }
            _ => None,
        })
    }

    pub(super) fn release(_byo_dir: &unix::ByoDirectory, _bound: Bound) {}
}

#[cfg(any(windows, target_os = "ios", target_os = "android"))]
use tcp_ingress as ingress;

#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
mod tcp_ingress_tests {
    use super::*;

    #[tokio::test]
    async fn long_private_root_uses_exclusive_tcp_ingress_and_releases_it() {
        let temp = tempfile::tempdir_in(crate::test_scratch()).unwrap();
        let directory = temp.path().join("j".repeat(120));
        std::fs::create_dir_all(&directory).unwrap();
        let root = JournalRoot::open(&directory).unwrap();
        let byo = unix::open_byo_directory(&root).unwrap();
        let address = tcp_ingress::address(&directory);
        let (listener, bound) = tcp_ingress::bind(&byo, &directory).unwrap();
        assert_eq!(listener.local_addr().unwrap().to_string(), address);
        assert!(tcp_ingress::bind(&byo, &directory).is_err());
        drop(listener);
        tcp_ingress::release(&byo, bound);
        let (listener, bound) = tcp_ingress::bind(&byo, &directory).unwrap();
        drop(listener);
        tcp_ingress::release(&byo, bound);
    }
}

pub(super) struct ByoBoundResources {
    pub(super) ingress_task: tokio::task::JoinHandle<()>,
    pub(super) acme_task: Option<tokio::task::JoinHandle<()>>,
    pub(super) ingress_bound: ingress::Bound,
    tunnel_task: Option<tokio::task::JoinHandle<()>>,
    tunnel_shutdown_tx: Option<watch::Sender<bool>>,
    tunnel_status_rx: Option<watch::Receiver<crate::byo_relay::RelayStatus>>,
    tunnel_channel_abort_handles: Arc<std::sync::Mutex<Vec<tokio::task::AbortHandle>>>,
    active_relay: Option<(std::net::Ipv4Addr, u16)>,
}

impl ByoBoundResources {
    async fn stop_relay_tunnel(&mut self) {
        if let Some(tx) = self.tunnel_shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(mut task) = self.tunnel_task.take()
            && tokio::time::timeout(Duration::from_secs(4), &mut task)
                .await
                .is_err()
        {
            log::error!("BYO relay shutdown timed out");
            task.abort();
            let _ = task.await;
        }
        for handle in self.tunnel_channel_abort_handles.lock().unwrap().drain(..) {
            handle.abort();
        }
        self.tunnel_status_rx = None;
        self.active_relay = None;
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RelayTestState {
    ManualForwarding,
    #[cfg_attr(not(feature = "full-tests"), allow(dead_code))]
    RelayConfiguredNotConnected,
    #[cfg_attr(not(feature = "full-tests"), allow(dead_code))]
    RelayConnected,
}

pub(super) struct ByoServiceRuntime {
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
    pub(super) bound_resources: Option<ByoBoundResources>,
    last_dns_verdict: Option<crate::byo_dns::DnsVerdict>,
    last_dns_addresses: Vec<String>,
    last_dns_account_uri: Option<String>,
    last_dns_check: tokio::time::Instant,
    state: Arc<DoorStateWriter>,
    #[cfg(test)]
    pub(crate) relay_test_state: RelayTestState,
}

impl ByoServiceRuntime {
    pub(super) fn new(
        state: Arc<DoorStateWriter>,
        byo_dir: &unix::ByoDirectory,
        hostname: String,
        generation: u64,
    ) -> Option<Self> {
        let journal_root = state.root().canonical_path();
        if state.root().revalidate_canonical_binding().is_err() {
            return None;
        }
        let account_dir = unix::open_byo_account_directory(byo_dir, &hostname).ok()?;
        let cert_dir = unix::open_byo_cert_directory(byo_dir, &hostname, generation).ok()?;
        let tls_service =
            McpEndpointTlsService::for_byo_cert_directory(cert_dir, hostname.clone(), generation)
                .ok()?;
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
            last_dns_addresses: Vec::new(),
            last_dns_account_uri: None,
            last_dns_check: tokio::time::Instant::now() - Duration::from_secs(120),
            state,
            #[cfg(test)]
            relay_test_state: RelayTestState::ManualForwarding,
        })
    }

    #[cfg(test)]
    #[cfg_attr(not(feature = "full-tests"), allow(dead_code))]
    pub(crate) fn set_relay_test_state(&mut self, state: RelayTestState) {
        self.relay_test_state = state;
    }

    fn renewal_spawn_allowed(&self) -> bool {
        #[cfg(test)]
        match self.relay_test_state {
            RelayTestState::ManualForwarding => true,
            RelayTestState::RelayConfiguredNotConnected => false,
            RelayTestState::RelayConnected => true,
        }
        #[cfg(not(test))]
        true
    }

    fn spawn_acme_renewal_task(
        &self,
        byo_dir: &unix::ByoDirectory,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let acc_dir_clone = unix::open_byo_account_directory(byo_dir, &self.hostname).ok()?;
        let tls_clone = Arc::clone(&self.tls_service);
        let mut acme_shutdown = self.service_shutdown_rx.clone();
        #[cfg(all(test, feature = "test-hooks"))]
        {
            let observer = crate::byo_dns::BYO_TEST_SEAM
                .acme_spawn_observer
                .read()
                .unwrap()
                .clone();
            if let Some(tx) = observer {
                let _ = tx.send(format!(
                    "{} {:?}",
                    self.hostname,
                    self.state.root().canonical_path()
                ));
                return Some(tokio::spawn(async move {
                    let _ = acme_shutdown.changed().await;
                }));
            }
        }
        Some(tokio::spawn(async move {
            let mut backoff = 30_u64;
            loop {
                match tls_clone
                    .run_byo_acme_renewal(&acc_dir_clone, &mut acme_shutdown)
                    .await
                {
                    Ok(()) => break,
                    Err(_err) => {
                        log::error!("BYO ACME renewal failed");
                        tokio::select! {
                            changed = acme_shutdown.changed() => {
                                if changed.is_err() || *acme_shutdown.borrow() {
                                    break;
                                }
                            }
                            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {
                                backoff = (backoff * 2).min(3600);
                            }
                        }
                    }
                }
            }
        }))
    }

    /// Withdraw the ingress. It returns once the accept task has ended, so the
    /// listener is closed before any state written after it says so.
    async fn unbind_ingress(&mut self, byo_dir: &unix::ByoDirectory) {
        if let Some(mut res) = self.bound_resources.take() {
            // Close accepted keep-alive streams too. Otherwise a client could
            // start another MCP request after CAA drift withdrew the ingress.
            // Local admission must be withdrawn before waiting on the relay.
            let _ = self.service_shutdown_tx.send(true);
            if let Some(a) = res.acme_task.take() {
                a.abort();
                let _ = a.await;
            }
            ingress::release(byo_dir, res.ingress_bound);
            let _ = (&mut res.ingress_task).await;
            res.stop_relay_tunnel().await;
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            self.service_shutdown_tx = shutdown_tx;
            self.service_shutdown_rx = shutdown_rx;
            self.sessions = Arc::new(SessionTable::new());
        }
    }

    pub(super) async fn stop(&mut self, byo_dir: &unix::ByoDirectory) {
        let _ = self.service_shutdown_tx.send(true);
        self.unbind_ingress(byo_dir).await;
    }

    pub(super) async fn tick_service_admission(
        &mut self,
        journal_root: &Path,
        byo_dir: &unix::ByoDirectory,
        ingress_address: &str,
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
            self.unbind_ingress(byo_dir).await;
            self.state.publish_byo(&ByoDoorState {
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
                relay_state: "none".to_string(),
                relay_detail: None,
                relay_observed_at: None,
                dns_addresses: Vec::new(),
                observed_at: Utc::now(),
            });
            return;
        }

        let uri_str = account_uri.clone().unwrap();

        // 2. DNS Check (recheck every 60s)
        if self.last_dns_account_uri.as_deref() != Some(uri_str.as_str()) {
            // A replacement ACME account has no inherited CAA verdict.
            self.unbind_ingress(byo_dir).await;
            self.last_dns_verdict = None;
            self.last_dns_addresses.clear();
            self.last_dns_account_uri = Some(uri_str.clone());
            self.state.publish_byo(&ByoDoorState {
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
                relay_state: "none".to_string(),
                relay_detail: None,
                relay_observed_at: None,
                dns_addresses: Vec::new(),
                observed_at: Utc::now(),
            });
        }

        let target_relay = match read_journal_config(journal_root) {
            Ok(cfg) => match byo_relay_config(&cfg) {
                ByoRelayConfig::Valid { address, port } => Some((address, port)),
                _ => None,
            },
            Err(_) => None,
        };

        #[cfg(all(test, feature = "test-hooks"))]
        let dns_due = self.last_dns_check.elapsed() >= Duration::from_secs(60)
            || self.last_dns_verdict.is_none()
            || crate::byo_dns::TEST_VERDICT_OVERRIDE
                .read()
                .unwrap()
                .is_some();
        #[cfg(not(all(test, feature = "test-hooks")))]
        let dns_due = self.last_dns_check.elapsed() >= Duration::from_secs(60)
            || self.last_dns_verdict.is_none();
        if dns_due {
            if self
                .bound_resources
                .as_ref()
                .is_some_and(|r| r.tunnel_task.is_some())
            {
                let (relay_state, relay_detail, relay_observed_at) = self
                    .bound_resources
                    .as_ref()
                    .and_then(|r| r.tunnel_status_rx.as_ref())
                    .map(|rx| {
                        let s = rx.borrow().clone();
                        (s.state.to_string(), s.detail, s.observed_at)
                    })
                    .unwrap_or_else(|| ("none".to_string(), None, None));

                let cert_active = self.tls_service.ordinary_certificate_is_active();
                let next_action = if cert_active {
                    Some("none".to_string())
                } else {
                    Some("issue_certificate".to_string())
                };

                self.state.publish_byo(&ByoDoorState {
                    hostname: Some(self.hostname.clone()),
                    enabled: true,
                    generation: self.generation,
                    account_uri: Some(uri_str.clone()),
                    caa: self
                        .last_dns_verdict
                        .as_ref()
                        .map(|v| v.code.as_str().to_string()),
                    dns_verdict: self
                        .last_dns_verdict
                        .as_ref()
                        .map(|v| v.code.as_str().to_string()),
                    dns_observed_at: self.last_dns_verdict.as_ref().map(|v| v.observed_at),
                    socket_listening: true,
                    certificate_active: cert_active,
                    socket_path: Some(ingress_address.to_string()),
                    socket_blocker: None,
                    next_action,
                    relay_state,
                    relay_detail,
                    relay_observed_at,
                    dns_addresses: self.last_dns_addresses.clone(),
                    observed_at: Utc::now(),
                });
            }

            let (verdict, addresses) = resolve_byo_dns(&self.hostname, &uri_str, Utc::now()).await;
            self.last_dns_verdict = Some(verdict);
            self.last_dns_addresses = addresses;
            self.last_dns_account_uri = Some(uri_str.clone());
            self.last_dns_check = tokio::time::Instant::now();
        }

        let verdict = self.last_dns_verdict.clone().unwrap();
        let dns_fresh = (Utc::now() - verdict.observed_at) <= chrono::Duration::seconds(60);
        let dns_admitted = verdict.is_admitted() && dns_fresh;

        if !dns_admitted {
            self.unbind_ingress(byo_dir).await;
            self.state.publish_byo(&ByoDoorState {
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
                relay_state: "none".to_string(),
                relay_detail: None,
                relay_observed_at: None,
                dns_addresses: self.last_dns_addresses.clone(),
                observed_at: Utc::now(),
            });
            return;
        }

        // 3. Both account and DNS are valid! Bind if not already bound.
        if self.bound_resources.is_none() {
            let (ingress_listener, ingress_bound) = match ingress::bind(byo_dir, journal_root) {
                Ok(bound) => bound,
                Err(blocker) => {
                    self.state.publish_byo(&ByoDoorState {
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
                        relay_state: "none".to_string(),
                        relay_detail: None,
                        relay_observed_at: None,
                        dns_addresses: self.last_dns_addresses.clone(),
                        observed_at: Utc::now(),
                    });
                    return;
                }
            };

            // Ingress accept task
            let ingress_root = Arc::new(journal_root.to_path_buf());
            let ingress_oauth = Arc::clone(&self.oauth);
            let ingress_sessions = Arc::clone(&self.sessions);
            let ingress_permits = Arc::clone(&self.permits);
            let ingress_tls_config = Arc::clone(&self.tls_config);
            let ingress_host_arc: Arc<str> = Arc::from(self.hostname.as_str());
            let mut ingress_shut = self.service_shutdown_rx.clone();

            let ingress_task = tokio::spawn(async move {
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        _ = connections.join_next(), if !connections.is_empty() => {}
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

                            connections.spawn(async move {
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
                drop(ingress_listener);
                connections.shutdown().await;
            });

            self.bound_resources = Some(ByoBoundResources {
                ingress_task,
                acme_task: None,
                ingress_bound,
                tunnel_task: None,
                tunnel_shutdown_tx: None,
                tunnel_status_rx: None,
                tunnel_channel_abort_handles: Arc::new(std::sync::Mutex::new(Vec::new())),
                active_relay: None,
            });
        }

        if let Some(ref mut res) = self.bound_resources {
            let tunnel_needs_restart = if res.active_relay != target_relay {
                true
            } else if let (Some((_addr, _port)), Some(rx)) =
                (res.active_relay, res.tunnel_status_rx.as_ref())
            {
                if rx.borrow().state == "host_key_changed" {
                    crate::byo_relay::known_host_file_is_absent(journal_root)
                } else {
                    false
                }
            } else {
                false
            };

            if tunnel_needs_restart {
                res.stop_relay_tunnel().await;

                if let Some((addr, port)) = target_relay {
                    let (shutdown_tx, shutdown_rx) = watch::channel(false);
                    let (status_tx, status_rx) = watch::channel(crate::byo_relay::RelayStatus {
                        state: "connecting",
                        detail: None,
                        observed_at: Some(Utc::now()),
                    });
                    let abort_handles = Arc::new(std::sync::Mutex::new(Vec::new()));
                    let task = tokio::spawn(crate::byo_relay::run_byo_relay_tunnel(
                        journal_root.to_path_buf(),
                        addr,
                        port,
                        shutdown_rx,
                        status_tx,
                        Arc::clone(&abort_handles),
                    ));
                    res.tunnel_task = Some(task);
                    res.tunnel_shutdown_tx = Some(shutdown_tx);
                    res.tunnel_status_rx = Some(status_rx);
                    res.tunnel_channel_abort_handles = abort_handles;
                    res.active_relay = Some((addr, port));
                }
            }
        }

        // Single spawn site for ACME renewal:
        // Manual forwarding spawns when ingress binds.
        // Unconnected relay leaves acme_task as None.
        // Connected relay on a later tick spawns then, without unbinding or rebinding ingress.
        let should_spawn = self
            .bound_resources
            .as_ref()
            .map(|b| b.acme_task.is_none())
            .unwrap_or(false)
            && self.renewal_spawn_allowed()
            && (target_relay.is_none()
                || self
                    .bound_resources
                    .as_ref()
                    .and_then(|bound| bound.tunnel_status_rx.as_ref())
                    .is_some_and(|rx| rx.borrow().state == "connected"));
        if should_spawn {
            let task = self.spawn_acme_renewal_task(byo_dir);
            if let Some(ref mut bound) = self.bound_resources {
                bound.acme_task = task;
            }
        }

        let (relay_state, relay_detail, relay_observed_at) = self
            .bound_resources
            .as_ref()
            .and_then(|r| r.tunnel_status_rx.as_ref())
            .map(|rx| {
                let s = rx.borrow().clone();
                (s.state.to_string(), s.detail, s.observed_at)
            })
            .unwrap_or_else(|| ("none".to_string(), None, None));

        let cert_active = self.tls_service.ordinary_certificate_is_active();
        let next_action = if cert_active {
            Some("none".to_string())
        } else {
            Some("issue_certificate".to_string())
        };

        self.state.publish_byo(&ByoDoorState {
            hostname: Some(self.hostname.clone()),
            enabled: true,
            generation: self.generation,
            account_uri: Some(uri_str),
            caa: None,
            dns_verdict: Some(verdict.code.as_str().to_string()),
            dns_observed_at: Some(verdict.observed_at),
            socket_listening: true,
            certificate_active: cert_active,
            socket_path: Some(ingress_address.to_string()),
            socket_blocker: None,
            next_action,
            relay_state,
            relay_detail,
            relay_observed_at,
            dns_addresses: self.last_dns_addresses.clone(),
            observed_at: Utc::now(),
        });
    }
}

fn disabled_state(hostname: Option<String>, enabled: bool, generation: u64) -> ByoDoorState {
    ByoDoorState {
        hostname,
        enabled,
        generation,
        account_uri: None,
        caa: None,
        dns_verdict: None,
        dns_observed_at: None,
        socket_listening: false,
        certificate_active: false,
        socket_path: None,
        socket_blocker: None,
        next_action: None,
        relay_state: "none".to_string(),
        relay_detail: None,
        relay_observed_at: None,
        dns_addresses: Vec::new(),
        observed_at: Utc::now(),
    }
}

async fn tick_byo_admission(
    state: &Arc<DoorStateWriter>,
    byo_dir: &unix::ByoDirectory,
    runtime: &mut Option<ByoServiceRuntime>,
    ingress_address: &str,
) {
    let journal_root = state.root().canonical_path();
    if state.root().revalidate_canonical_binding().is_err() {
        if let Some(mut rt) = runtime.take() {
            rt.stop(byo_dir).await;
        }
        return;
    }
    let config = match read_journal_config(journal_root) {
        Ok(cfg) => byo_hostname_config(&cfg),
        Err(_) => ByoHostnameConfigStatus::Invalid,
    };

    match config {
        ByoHostnameConfigStatus::None | ByoHostnameConfigStatus::Invalid => {
            if let Some(mut rt) = runtime.take() {
                rt.stop(byo_dir).await;
            }
            state.publish_byo(&disabled_state(None, false, 0));
        }
        ByoHostnameConfigStatus::Configured(cfg) => {
            if !cfg.enabled {
                if let Some(mut rt) = runtime.take() {
                    rt.stop(byo_dir).await;
                }
                state.publish_byo(&disabled_state(cfg.hostname, false, cfg.generation));
            } else {
                let hostname = match cfg.hostname {
                    Some(h) => h,
                    None => {
                        if let Some(mut rt) = runtime.take() {
                            rt.stop(byo_dir).await;
                        }
                        state.publish_byo(&disabled_state(None, true, cfg.generation));
                        return;
                    }
                };

                let needs_restart = match runtime.as_ref() {
                    Some(rt) => rt.hostname != hostname || rt.generation != cfg.generation,
                    None => true,
                };

                if needs_restart {
                    if let Some(mut rt) = runtime.take() {
                        rt.stop(byo_dir).await;
                    }
                    *runtime = ByoServiceRuntime::new(
                        Arc::clone(state),
                        byo_dir,
                        hostname,
                        cfg.generation,
                    );
                }

                if let Some(rt) = runtime.as_mut() {
                    rt.tick_service_admission(journal_root, byo_dir, ingress_address)
                        .await;
                }
            }
        }
    }
}

#[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
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

#[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
fn reply_cutover_ok(stream: &std::os::unix::net::UnixStream) {
    let _ = nix::unistd::write(stream, b"ok\n");
}

#[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
type CutoverAccept = std::io::Result<(tokio::net::UnixStream, tokio::net::unix::SocketAddr)>;

#[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
async fn apply_cutover(
    accepted: CutoverAccept,
    state: &Arc<DoorStateWriter>,
    byo_dir: &unix::ByoDirectory,
    runtime: &mut Option<ByoServiceRuntime>,
    ingress_address: &str,
) {
    if let Ok((stream, _)) = accepted
        && let Ok(std_stream) = stream.into_std()
        && handle_cutover_stream(&std_stream)
    {
        tick_byo_admission(state, byo_dir, runtime, ingress_address).await;
        reply_cutover_ok(&std_stream);
    }
}

/// TCP ingress hosts apply changes on the existing configuration tick.
#[cfg(any(windows, target_os = "ios", target_os = "android"))]
enum CutoverAccept {}

#[cfg(any(windows, target_os = "ios", target_os = "android"))]
async fn apply_cutover(
    accepted: CutoverAccept,
    _state: &Arc<DoorStateWriter>,
    _byo_dir: &unix::ByoDirectory,
    _runtime: &mut Option<ByoServiceRuntime>,
    _ingress_address: &str,
) {
    match accepted {}
}

/// Run the owner-hostname door until shutdown.
///
/// Desktop Unix applies owner changes through the cutover socket. TCP ingress
/// hosts apply changes on the next half-second tick; the agents page waits for
/// the state written after each change is applied.
pub async fn run_byo_door_async(
    journal_root: PathBuf,
    shutdown: watch::Receiver<bool>,
) -> Result<(), ()> {
    let root = Arc::new(JournalRoot::open(&journal_root).map_err(|_| ())?);
    run_byo_door_at_root(Arc::new(DoorStateWriter::new(root)), shutdown).await
}

pub(crate) async fn run_byo_door_at_root(
    state: Arc<DoorStateWriter>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ()> {
    let root = state.root();
    let journal_root = root.canonical_path().to_path_buf();
    let byo_dir = unix::open_byo_directory(root).map_err(|_| ())?;
    let ingress_address = ingress_address(&journal_root);

    #[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
    let (cutover_listener, cutover_inode) = {
        let cutover_path = journal_root
            .join("mcp-endpoint")
            .join(unix::BYO_DIRECTORY)
            .join(unix::BYO_CUTOVER_SOCKET);
        let (std_cutover, cutover_inode) =
            unix::bind_byo_socket(&byo_dir, unix::BYO_CUTOVER_SOCKET, &cutover_path)
                .map_err(|_| ())?;
        let _ = std_cutover.set_nonblocking(true);
        (
            tokio::net::UnixListener::from_std(std_cutover).map_err(|_| ())?,
            cutover_inode,
        )
    };

    let mut runtime: Option<ByoServiceRuntime> = None;
    let mut poll_interval = tokio::time::interval(Duration::from_millis(500));

    loop {
        #[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
        let cutover = cutover_listener.accept();
        #[cfg(any(windows, target_os = "ios", target_os = "android"))]
        let cutover = std::future::pending::<CutoverAccept>();

        #[cfg(all(test, feature = "test-hooks"))]
        let tick_held = crate::byo_dns::BYO_TEST_SEAM
            .tick_hold
            .load(std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(all(test, feature = "test-hooks")))]
        let _tick_held = false;

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = async {
                #[cfg(all(test, feature = "test-hooks"))]
                if tick_held {
                    crate::byo_dns::BYO_TEST_SEAM.tick_notify.notified().await;
                    return;
                }
                poll_interval.tick().await;
            } => {
                if *shutdown.borrow() {
                    break;
                }
                tick_byo_admission(&state, &byo_dir, &mut runtime, &ingress_address).await;
            }
            accepted = cutover => {
                apply_cutover(accepted, &state, &byo_dir, &mut runtime, &ingress_address).await;
            }
        }
    }

    if let Some(mut rt) = runtime.take() {
        rt.stop(&byo_dir).await;
    }
    #[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
    unix::unlink_byo_socket_if_inode_matches(&byo_dir, unix::BYO_CUTOVER_SOCKET, cutover_inode);
    state.publish_byo(&disabled_state(None, false, 0));
    Ok(())
}

pub async fn run_single_byo_service(
    journal_root: PathBuf,
    hostname: String,
    generation: u64,
    mut shutdown: watch::Receiver<bool>,
) {
    let root = match JournalRoot::open(&journal_root) {
        Ok(r) => Arc::new(r),
        Err(_) => return,
    };

    let byo_dir = match unix::open_byo_directory(&root) {
        Ok(d) => d,
        Err(_) => return,
    };

    let ingress_address = ingress_address(&journal_root);

    let state = Arc::new(DoorStateWriter::new(root));
    let mut runtime = match ByoServiceRuntime::new(state, &byo_dir, hostname, generation) {
        Some(rt) => rt,
        None => return,
    };

    let mut poll_interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        #[cfg(all(test, feature = "test-hooks"))]
        let tick_held = crate::byo_dns::BYO_TEST_SEAM
            .tick_hold
            .load(std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(all(test, feature = "test-hooks")))]
        let _tick_held = false;

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = async {
                #[cfg(all(test, feature = "test-hooks"))]
                if tick_held {
                    crate::byo_dns::BYO_TEST_SEAM.tick_notify.notified().await;
                    return;
                }
                poll_interval.tick().await;
            } => {
                if *shutdown.borrow() {
                    break;
                }
                runtime
                    .tick_service_admission(&journal_root, &byo_dir, &ingress_address)
                    .await;
            }
        }
    }

    runtime.stop(&byo_dir).await;
}

#[cfg(all(test, not(feature = "full-tests")))]
// The owner-hostname door runs on Unix only.
#[cfg(unix)]
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
            .tempdir_in(crate::test_scratch())
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
                relay_state: "none".to_string(),
                relay_detail: None,
                relay_observed_at: None,
                dns_addresses: Vec::new(),
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
                relay_state: "none".to_string(),
                relay_detail: None,
                relay_observed_at: None,
                dns_addresses: Vec::new(),
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
#[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
mod full_tests {
    #[tokio::test]
    async fn live_byo_tick_preserves_terminal_state_and_never_writes_a_replacement_root() {
        for replace in [false, true] {
            let temporary = tempfile::TempDir::new_in(crate::test_scratch()).unwrap();
            let journal = temporary.path().join("journal");
            let retained = temporary.path().join("retained");
            std::fs::create_dir(&journal).unwrap();
            let state = Arc::new(DoorStateWriter::new(Arc::new(
                JournalRoot::open(&journal).unwrap(),
            )));
            let (stop, stopped) = watch::channel(false);
            let service = tokio::spawn(run_byo_door_at_root(state, stopped));
            let first = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(state) = read_byo_door_state(&journal) {
                        break state;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            if replace {
                std::fs::rename(&journal, &retained).unwrap();
                std::fs::create_dir(&journal).unwrap();
            }
            // Permit the maintained half-second admission tick while still live.
            tokio::time::sleep(Duration::from_millis(700)).await;
            if !replace {
                assert!(read_byo_door_state(&journal).unwrap().observed_at > first.observed_at);
            }
            stop.send_replace(true);
            tokio::time::timeout(Duration::from_secs(5), service)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if replace {
                let entries = std::fs::read_dir(&journal)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect::<Vec<_>>();
                assert!(
                    entries.is_empty(),
                    "live BYO published into replacement root: {entries:?}"
                );
            } else {
                let stopped = read_byo_door_state(&journal).unwrap();
                assert!(!stopped.socket_listening);
                assert!(!stopped.enabled);
                assert_eq!(stopped.generation, 0);
            }
        }
    }

    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::Utc;
    use solstone_core_journal_io::journal_root::JournalRoot;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;

    use super::*;
    use crate::byo_dns::{DnsVerdict, DnsVerdictCode};
    use crate::unix::{self, ByoSocketBlocker};

    // These fixtures deliberately replace process-wide DNS, registrar, and
    // clock inputs. Keep their complete lifetimes separate under parallel CI.
    use crate::byo_dns::BYO_TEST_SERIAL;

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
            .tempdir_in(crate::test_scratch())
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
                *g = Some((
                    DnsVerdict {
                        code: DnsVerdictCode::CaaMissing,
                        observed_at: Utc::now(),
                    },
                    Vec::new(),
                ));
            }
            let (shutdown_send, shutdown_recv) = watch::channel(false);
            let root_clone = journal_path.clone();
            let handle = tokio::spawn(async move {
                run_single_byo_service(root_clone, "mcp.example.com".to_string(), 1, shutdown_recv)
                    .await;
            });
            // Wait for the service to publish its state rather than a fixed
            // interval, which a loaded host can outlast.
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let state = loop {
                if let Some(state) = read_byo_door_state(&journal_path) {
                    break state;
                }
                assert!(std::time::Instant::now() < deadline, "state written");
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            assert!(
                !socket_path.exists(),
                "socket must not be created without admitted DNS"
            );
            assert!(!state.socket_listening);

            let _ = shutdown_send.send(true);
            let _ = handle.await;
        }

        // 4. Fresh admitted DNS fixture -> run_single_byo_service binds mode 0o600 under 0o700 directory
        {
            if let Ok(mut g) = crate::byo_dns::TEST_VERDICT_OVERRIDE.write() {
                *g = Some((
                    DnsVerdict {
                        code: DnsVerdictCode::Admitted,
                        observed_at: Utc::now(),
                    },
                    vec!["198.51.100.1".to_string()],
                ));
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
            1,
        )
        .unwrap();
        let _clock = crate::tls::lock_test_clock(Some(base_now));
        tls_service
            .install_ordinary_certificate(vec![cert_der.clone()], key_der, not_before, not_after)
            .unwrap();

        if let Ok(mut g) = crate::byo_dns::TEST_VERDICT_OVERRIDE.write() {
            *g = Some((
                DnsVerdict {
                    code: DnsVerdictCode::Admitted,
                    observed_at: Utc::now(),
                },
                vec!["198.51.100.1".to_string()],
            ));
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

        // Stop also owns a peer that connects but never completes TLS.
        let mut runtime = ByoServiceRuntime::new(
            Arc::new(DoorStateWriter::new(Arc::new(
                JournalRoot::open(&journal_path).unwrap(),
            ))),
            &byo_dir,
            "mcp.example.com".to_string(),
            1,
        )
        .unwrap();
        runtime
            .tick_service_admission(&journal_path, &byo_dir, &ingress_address(&journal_path))
            .await;
        let mut pending = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.permits.available_permits() == DEFAULT_BYO_CONNECTION_PERMITS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        runtime.stop(&byo_dir).await;
        assert_eq!(
            runtime.permits.available_permits(),
            DEFAULT_BYO_CONNECTION_PERMITS
        );
        let mut byte = [0];
        assert_eq!(pending.read(&mut byte).await.unwrap(), 0);
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
            1,
        )
        .unwrap();

        let base_now = 1700000000_i64;
        let not_before = base_now - 1000;
        let not_after = base_now + 1000;
        let _clock = crate::tls::lock_test_clock(Some(base_now));

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
            *g = Some((
                DnsVerdict {
                    code: DnsVerdictCode::Admitted,
                    observed_at: Utc::now(),
                },
                vec!["198.51.100.1".to_string()],
            ));
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

    #[tokio::test]
    async fn relay_spawn_lifecycle_governed_by_relay_state() {
        let _serial = BYO_TEST_SERIAL.lock().await;
        let _guard = TestOverridesGuard;
        let (dir, root) = test_journal();
        let journal_path = dir.path().to_path_buf();
        let byo_dir = unix::open_byo_directory(&root).unwrap();
        let host = "mcp.example.com";
        let generation = 1;
        let acc_dir = unix::open_byo_account_directory(&byo_dir, host).unwrap();

        let keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        unix::persist_byo_account_key(&acc_dir, &keypair.serialize_der()).unwrap();
        unix::persist_byo_account_uri(
            &acc_dir,
            "https://acme-v02.api.letsencrypt.org/acme/acct/12345",
        )
        .unwrap();

        if let Ok(mut g) = crate::byo_dns::TEST_VERDICT_OVERRIDE.write() {
            *g = Some((
                DnsVerdict {
                    code: DnsVerdictCode::Admitted,
                    observed_at: Utc::now(),
                },
                vec![],
            ));
        }

        {
            let mut rt = ByoServiceRuntime::new(
                Arc::new(DoorStateWriter::new(Arc::new(
                    JournalRoot::open(&journal_path).unwrap(),
                ))),
                &byo_dir,
                host.to_string(),
                generation,
            )
            .expect("runtime");
            assert_eq!(rt.relay_test_state, RelayTestState::ManualForwarding);
            rt.tick_service_admission(&journal_path, &byo_dir, "mcp-endpoint/byo/ingress.sock")
                .await;
            let bound = rt.bound_resources.as_ref().expect("bound resources");
            assert!(
                bound.acme_task.is_some(),
                "ManualForwarding must spawn ACME renewal task"
            );
            rt.stop(&byo_dir).await;
        }

        {
            let mut rt = ByoServiceRuntime::new(
                Arc::new(DoorStateWriter::new(Arc::new(
                    JournalRoot::open(&journal_path).unwrap(),
                ))),
                &byo_dir,
                host.to_string(),
                generation,
            )
            .expect("runtime");
            rt.set_relay_test_state(RelayTestState::RelayConfiguredNotConnected);
            rt.tick_service_admission(&journal_path, &byo_dir, "mcp-endpoint/byo/ingress.sock")
                .await;
            let initial_ingress_id = {
                let bound = rt.bound_resources.as_ref().expect("ingress bound");
                assert!(
                    bound.acme_task.is_none(),
                    "RelayConfiguredNotConnected must not spawn ACME task"
                );
                bound.ingress_task.id()
            };

            rt.set_relay_test_state(RelayTestState::RelayConnected);
            rt.tick_service_admission(&journal_path, &byo_dir, "mcp-endpoint/byo/ingress.sock")
                .await;
            let bound = rt.bound_resources.as_ref().expect("ingress still bound");
            assert!(
                bound.acme_task.is_some(),
                "RelayConnected must spawn ACME task on later tick"
            );
            assert_eq!(
                bound.ingress_task.id(),
                initial_ingress_id,
                "ingress task id must be unchanged (no second bind)"
            );
            rt.stop(&byo_dir).await;
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(windows)]
mod windows_full_tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::Utc;
    use solstone_core_journal_config::MCP_BYO_INGRESS_PORT;
    use solstone_core_journal_io::journal_root::JournalRoot;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::watch;

    use super::*;
    use crate::byo_dns::{DnsVerdict, DnsVerdictCode};

    // The ingress port and the DNS, registrar and clock fixtures are
    // process-wide. Keep each test's whole lifetime separate.
    use crate::byo_dns::BYO_TEST_SERIAL;
    const HOST: &str = "mcp.example.com";
    const ACCOUNT_URI: &str = "https://acme-v02.api.letsencrypt.org/acme/acct/12345";

    struct TestOverridesGuard;
    impl Drop for TestOverridesGuard {
        fn drop(&mut self) {
            if let Ok(mut g) = crate::byo_dns::TEST_VERDICT_OVERRIDE.write() {
                *g = None;
            }
            crate::tls::set_test_now_override(None);
        }
    }

    fn ingress() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, MCP_BYO_INGRESS_PORT))
    }

    fn test_journal() -> (TempDir, JournalRoot) {
        let dir = tempfile::Builder::new()
            .prefix("solstone-byo-win-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        let root = JournalRoot::open(dir.path()).unwrap();
        (dir, root)
    }

    fn set_dns(code: DnsVerdictCode) {
        *crate::byo_dns::TEST_VERDICT_OVERRIDE.write().unwrap() = Some((
            DnsVerdict {
                code,
                observed_at: Utc::now(),
            },
            vec!["198.51.100.1".to_string()],
        ));
    }

    fn register_account(byo_dir: &unix::ByoDirectory) {
        let account = unix::open_byo_account_directory(byo_dir, HOST).unwrap();
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        unix::persist_byo_account_key(&account, &key.serialize_der()).unwrap();
        unix::persist_byo_account_uri(&account, ACCOUNT_URI).unwrap();
    }

    async fn answers() -> bool {
        matches!(
            tokio::time::timeout(
                Duration::from_millis(500),
                tokio::net::TcpStream::connect(ingress())
            )
            .await,
            Ok(Ok(_))
        )
    }

    async fn wait_for(journal: &Path, test: impl Fn(&ByoDoorState) -> bool) -> ByoDoorState {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(state) = read_byo_door_state(journal)
                && test(&state)
            {
                return state;
            }
            assert!(std::time::Instant::now() < deadline, "door state");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The ingress opens only for a registered account and freshly admitted
    /// DNS, serves the journal's own TLS directly with no proxy preface, and
    /// closes when the door stops.
    #[tokio::test]
    async fn the_loopback_ingress_opens_only_when_admitted_and_serves_raw_tls() {
        let _serial = BYO_TEST_SERIAL.lock().await;
        let _guard = TestOverridesGuard;
        let (dir, root) = test_journal();
        let journal = dir.path().to_path_buf();
        let byo_dir = unix::open_byo_directory(&root).unwrap();
        register_account(&byo_dir);

        set_dns(DnsVerdictCode::CaaMissing);
        let (stop, stopped) = watch::channel(false);
        let service = tokio::spawn(run_single_byo_service(
            journal.clone(),
            HOST.to_string(),
            1,
            stopped,
        ));
        let state = wait_for(&journal, |_| true).await;
        assert!(!state.socket_listening);
        assert!(!answers().await, "no ingress without admitted DNS");
        let _ = stop.send(true);
        service.await.unwrap();

        let base_now = Utc::now().timestamp();
        let (not_before, not_after) = (base_now - 1000, base_now + 10000);
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = rcgen::CertificateParams::new(vec![HOST.to_string()]).unwrap();
        params.not_before = time::OffsetDateTime::from_unix_timestamp(not_before).unwrap();
        params.not_after = time::OffsetDateTime::from_unix_timestamp(not_after).unwrap();
        let cert = params.self_signed(&key).unwrap();
        let cert_der = cert.der().to_vec();
        let cert_dir = unix::open_byo_cert_directory(&byo_dir, HOST, 1).unwrap();
        let _clock = crate::tls::lock_test_clock(Some(base_now));
        crate::tls::McpEndpointTlsService::for_byo_cert_directory(cert_dir, HOST.to_string(), 1)
            .unwrap()
            .install_ordinary_certificate(
                vec![cert_der.clone()],
                key.serialize_der(),
                not_before,
                not_after,
            )
            .unwrap();

        set_dns(DnsVerdictCode::Admitted);
        let (stop, stopped) = watch::channel(false);
        let service = tokio::spawn(run_single_byo_service(
            journal.clone(),
            HOST.to_string(),
            1,
            stopped,
        ));
        let state = wait_for(&journal, |state| state.socket_listening).await;
        assert_eq!(
            state.socket_path.as_deref(),
            Some(format!("127.0.0.1:{MCP_BYO_INGRESS_PORT}").as_str())
        );

        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(cert_der))
            .unwrap();
        let mut client = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        client.alpn_protocols = vec![b"http/1.1".to_vec()];
        let client = Arc::new(client);
        let name = rustls::pki_types::ServerName::try_from(HOST.to_string()).unwrap();
        let stream = tokio::net::TcpStream::connect(ingress()).await.unwrap();
        let mut tls = tokio_rustls::TlsConnector::from(Arc::clone(&client))
            .connect(name.clone(), stream)
            .await
            .expect("TLS completes with no proxy preface");
        tls.shutdown().await.unwrap();

        let mut stream = tokio::net::TcpStream::connect(ingress()).await.unwrap();
        stream
            .write_all(b"PROXY TCP4 198.51.100.12 127.0.0.1 4321 443\r\n")
            .await
            .unwrap();
        assert!(
            tokio_rustls::TlsConnector::from(client)
                .connect(name, stream)
                .await
                .is_err(),
            "a proxy preface does not complete a handshake"
        );

        let _ = stop.send(true);
        service.await.unwrap();
        assert!(!answers().await, "the ingress closes when the door stops");
    }

    /// While the door holds the port, no other program can take it over with
    /// `SO_REUSEADDR`; if another program holds it first, the door reports
    /// the port as blocked instead of listening.
    #[tokio::test]
    async fn the_ingress_port_cannot_be_taken_over_and_a_held_port_is_reported() {
        use socket2::{Domain, Socket, Type};

        let _serial = BYO_TEST_SERIAL.lock().await;
        let _guard = TestOverridesGuard;
        let (dir, root) = test_journal();
        let journal = dir.path().to_path_buf();
        let byo_dir = unix::open_byo_directory(&root).unwrap();
        register_account(&byo_dir);
        set_dns(DnsVerdictCode::Admitted);

        let (listener, ()) = ingress::bind(&byo_dir, &journal).expect("ingress binds");
        // The listener does not set SO_EXCLUSIVEADDRUSE. Windows refuses a
        // SO_REUSEADDR bind on the exact address. It admits one on the
        // wildcard, but connections to 127.0.0.1, the only address the
        // owner's forward targets, still reach the journal's listener.
        let probe = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
        probe.set_reuse_address(true).unwrap();
        assert_eq!(
            probe.bind(&ingress().into()).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let wildcard = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
        wildcard.set_reuse_address(true).unwrap();
        if wildcard
            .bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, MCP_BYO_INGRESS_PORT)).into())
            .is_ok()
        {
            wildcard.listen(8).unwrap();
            wildcard.set_nonblocking(true).unwrap();
            for _ in 0..3 {
                let _client = tokio::net::TcpStream::connect(ingress()).await.unwrap();
                tokio::time::timeout(Duration::from_secs(2), listener.accept())
                    .await
                    .expect("the journal's listener takes the connection")
                    .unwrap();
                assert_eq!(
                    wildcard.accept().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock,
                    "a wildcard listener never sees loopback traffic"
                );
            }
        }
        assert!(matches!(
            ingress::bind(&byo_dir, &journal),
            Err(Some(ByoSocketBlocker::PortInUse))
        ));

        let (stop, stopped) = watch::channel(false);
        let service = tokio::spawn(run_single_byo_service(
            journal.clone(),
            HOST.to_string(),
            1,
            stopped,
        ));
        let state = wait_for(&journal, |state| state.socket_blocker.is_some()).await;
        assert_eq!(state.socket_blocker, Some(ByoSocketBlocker::PortInUse));
        assert_eq!(state.next_action.as_deref(), Some("socket_blocked"));
        assert!(!state.socket_listening);
        let _ = stop.send(true);
        service.await.unwrap();
        drop(listener);
    }

    /// Turning the hostname off withdraws the ingress before the agents page
    /// is told it is off.
    #[tokio::test]
    async fn turning_the_hostname_off_closes_the_ingress_before_the_page_hears_it() {
        let _serial = BYO_TEST_SERIAL.lock().await;
        let _guard = TestOverridesGuard;
        let (dir, root) = test_journal();
        let journal = dir.path().to_path_buf();
        let byo_dir = unix::open_byo_directory(&root).unwrap();
        register_account(&byo_dir);
        set_dns(DnsVerdictCode::Admitted);
        let write_config = |enabled: bool| {
            fs::create_dir_all(journal.join("config")).unwrap();
            fs::write(
                journal.join("config/journal.json"),
                serde_json::json!({"mcp_endpoint": {"byo_hostname": {
                    "hostname": HOST, "enabled": enabled, "generation": 1
                }}})
                .to_string(),
            )
            .unwrap();
        };
        write_config(true);

        let (stop, stopped) = watch::channel(false);
        let door = tokio::spawn(run_byo_door_async(journal.clone(), stopped));
        wait_for(&journal, |state| state.socket_listening).await;
        assert!(answers().await);

        write_config(false);
        let response = crate::owner_web::perform_byo_cutover(Arc::new(journal.clone())).await;
        assert!(response.status().is_success(), "{}", response.status());
        assert!(!answers().await, "closed before the page hears it");
        let state = read_byo_door_state(&journal).unwrap();
        assert!(!state.enabled && !state.socket_listening);

        let _ = stop.send(true);
        door.await.unwrap().unwrap();
    }
}
