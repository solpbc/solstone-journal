// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Production process composition for the journal-local MCP endpoint.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use solstone_core_journal_config::{
    MCP_ENDPOINT_LOOPBACK_PORT, McpEndpointCapability, mcp_endpoint_capability, read_journal_config,
};
use solstone_core_system::lifecycle::{
    HostedServiceParentRuntime, HostedServiceShutdownEvidence, ParentLossReason,
};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinSet;

use crate::{bootstrap_mcp_endpoint_owner_identity, mcp_endpoint_server_config};

/// Class-only failures from starting or operating the hosted MCP endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpServiceError {
    /// The dedicated service runtime could not start.
    Runtime,
    /// The capability-enabled owner identity could not be opened.
    Bootstrap,
    /// The account-authorized TLS tunnel could not be established.
    Tunnel,
    /// The fixed loopback listener could not bind.
    Bind,
    /// The listener ended unexpectedly.
    Listener,
    /// The bridge forwarder ended unexpectedly.
    Forwarder,
    /// The ACME renewal loop ended with a certificate lifecycle error.
    Renewal,
    /// A hosted parent-loss witness could not be published.
    ParentLoss,
}

impl McpServiceError {
    /// Return the safe class for process output.
    #[must_use]
    pub const fn class(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Bootstrap => "bootstrap",
            Self::Tunnel => "tunnel",
            Self::Bind => "bind",
            Self::Listener => "listener",
            Self::Forwarder => "forwarder",
            Self::Renewal => "renewal",
            Self::ParentLoss => "parent-loss",
        }
    }
}

impl fmt::Display for McpServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.class())
    }
}

impl std::error::Error for McpServiceError {}

/// Run the production MCP service with optional supervisor parent tracking.
///
/// # Errors
///
/// Returns a class-only error when an enabled service cannot bootstrap, bind,
/// or keep its listener, bridge forwarder, and certificate renewal running.
pub fn run_native_service_with_hosted_parent(
    journal_root: PathBuf,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), McpServiceError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("mcp-service")
        .build()
        .map_err(|_| McpServiceError::Runtime)?;
    runtime.block_on(run_native_service_async(journal_root, hosted_parent))
}

async fn run_native_service_async(
    journal_root: PathBuf,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), McpServiceError> {
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

    let result = run_endpoint_topology(journal_root, shutdown_send.clone(), shutdown_receive).await;
    let service_stopped = result.is_ok();
    signal_task.abort();
    let _ = signal_task.await;
    if let Some(parent_task) = parent_task {
        parent_task.abort();
        let _ = parent_task.await;
    }
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

async fn run_endpoint_topology(
    journal_root: PathBuf,
    shutdown_send: watch::Sender<bool>,
    mut shutdown_receive: watch::Receiver<bool>,
) -> Result<(), McpServiceError> {
    while !capability_enabled(&journal_root) {
        tokio::select! {
            changed = shutdown_receive.changed() => {
                if changed.is_err() || *shutdown_receive.borrow_and_update() {
                    return Ok(());
                }
            }
            () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
        }
    }
    crate::owner_state::write_mcp_owner_state(
        &journal_root,
        "turning_on",
        None,
        ("in_progress", "waiting", "waiting"),
        None,
    );
    let Some(owner) = bootstrap_mcp_endpoint_owner_identity(&journal_root).map_err(|_| {
        crate::owner_state::write_mcp_owner_state(
            &journal_root,
            "failed",
            None,
            ("failed", "waiting", "waiting"),
            Some("your journal could not prepare its agent address"),
        );
        McpServiceError::Bootstrap
    })?
    else {
        return Ok(());
    };

    let mut tunnel_shutdown = shutdown_receive.clone();
    let tunnel = match owner
        .connect_mcp_endpoint_tunnel(&mut tunnel_shutdown)
        .await
    {
        Ok(tunnel) => tunnel,
        Err(_) if shutdown_requested(&shutdown_receive) => return Ok(()),
        Err(_) => {
            crate::owner_state::write_mcp_owner_state(
                &journal_root,
                "failed",
                None,
                ("failed", "waiting", "waiting"),
                Some(
                    "this computer could not reach services.solstone.app; it will try again when the service restarts",
                ),
            );
            return Err(McpServiceError::Tunnel);
        }
    };
    let (tls, forwarder_session) = tunnel.into_service_parts();
    let tls = Arc::new(tls);
    let endpoint_address = tls.authorized_hostname().to_owned();
    crate::owner_state::write_mcp_owner_state(
        &journal_root,
        if tls.ordinary_certificate_is_active() {
            "on"
        } else {
            "turning_on"
        },
        Some(&endpoint_address),
        if tls.ordinary_certificate_is_active() {
            ("done", "done", "in_progress")
        } else {
            ("done", "in_progress", "waiting")
        },
        None,
    );
    let tls_config = mcp_endpoint_server_config(&tls);
    let resource_origin = format!("https://{}", tls.authorized_hostname());
    let listener = TcpListener::bind(("127.0.0.1", MCP_ENDPOINT_LOOPBACK_PORT))
        .await
        .map_err(|_| McpServiceError::Bind)?;

    let mut tasks = JoinSet::new();
    let capability_root = journal_root.clone();
    let capability_shutdown = shutdown_send.clone();
    let mut capability_parent_shutdown = shutdown_receive.clone();
    tasks.spawn(async move {
        loop {
            tokio::select! {
                changed = capability_parent_shutdown.changed() => {
                    if changed.is_err() || *capability_parent_shutdown.borrow_and_update() {
                        return Ok(());
                    }
                }
                () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    if !capability_enabled(&capability_root) {
                        capability_shutdown.send_replace(true);
                        return Ok(());
                    }
                }
            }
        }
    });
    let state_root = journal_root.clone();
    let state_tls = Arc::clone(&tls);
    let state_address = endpoint_address.clone();
    let mut state_shutdown = shutdown_receive.clone();
    tasks.spawn(async move {
        loop {
            tokio::select! {
                changed = state_shutdown.changed() => {
                    if changed.is_err() || *state_shutdown.borrow_and_update() { return Ok(()); }
                }
                () = tokio::time::sleep(std::time::Duration::from_secs(3)) => {
                    let active = state_tls.ordinary_certificate_is_active();
                    let rate_limit_is_current = crate::owner_state::read_mcp_owner_state(&state_root)
                        .is_some_and(|state| state.certificate_leg == "not_this_week"
                            && state.next_attempt_at.is_some_and(|next| next > chrono::Utc::now()));
                    if rate_limit_is_current && !active {
                        continue;
                    }
                    crate::owner_state::write_mcp_owner_state(
                        &state_root,
                        if active { "on" } else { "turning_on" },
                        Some(&state_address),
                        if active { ("done", "done", "done") } else { ("done", "in_progress", "waiting") },
                        if active { None } else { Some("the journal keeps trying on its own") },
                    );
                }
            }
        }
    });
    let listener_shutdown = shutdown_receive.clone();
    let listener_root = Arc::new(journal_root);
    let server_root = Arc::clone(&listener_root);
    let renewal_root = Arc::clone(&listener_root);
    let oauth = Arc::new(crate::oauth::OAuthRuntime::new(
        listener_root.as_path(),
        resource_origin,
    ));
    tasks.spawn(async move {
        crate::server::serve(listener, tls_config, server_root, oauth, listener_shutdown)
            .await
            .map_err(|_| McpServiceError::Listener)
    });
    let mut forwarder_shutdown = shutdown_receive.clone();
    let forwarder_tls = Arc::clone(&tls);
    let forwarder_owner = owner.renewal_owner();
    tasks.spawn(async move {
        crate::bridge_forwarder::run_bound_session(
            forwarder_owner,
            forwarder_tls,
            forwarder_session,
            &mut forwarder_shutdown,
        )
        .await
        .map_err(|_| McpServiceError::Forwarder)
    });
    let renewal_tls = Arc::clone(&tls);
    let mut renewal_shutdown = shutdown_receive.clone();
    tasks.spawn(async move {
        renewal_tls
            .run_acme_renewal_with_owner_state(renewal_root.as_path(), &mut renewal_shutdown)
            .await
            .map_err(|_| McpServiceError::Renewal)
    });

    let result = tokio::select! {
        changed = shutdown_receive.changed() => {
            let _ = changed;
            Ok(())
        }
        joined = tasks.join_next() => match joined {
            Some(Ok(Ok(()))) if shutdown_requested(&shutdown_receive) => Ok(()),
            Some(Ok(Ok(()))) => Err(McpServiceError::Listener),
            Some(Ok(Err(error))) => Err(error),
            Some(Err(_)) | None => Err(McpServiceError::Listener),
        },
    };
    if matches!(result, Err(McpServiceError::Forwarder))
        && capability_enabled(listener_root.as_path())
    {
        crate::owner_state::write_mcp_owner_state(
            listener_root.as_path(),
            "offline",
            Some(&endpoint_address),
            ("done", "done", "waiting"),
            Some("your journal is offline right now. your agents will get through when it's back"),
        );
    }
    shutdown_send.send_replace(true);
    while tasks.join_next().await.is_some() {}
    result
}

fn capability_enabled(journal_root: &Path) -> bool {
    matches!(
        read_journal_config(journal_root)
            .ok()
            .and_then(|read| mcp_endpoint_capability(&read).ok()),
        Some(McpEndpointCapability::Enabled)
    )
}

fn shutdown_requested(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow()
}

async fn wait_for_hosted_parent(
    parent: Arc<HostedServiceParentRuntime>,
    shutdown: watch::Sender<bool>,
    parent_loss: oneshot::Sender<ParentLossReason>,
) {
    let reason = parent.await_parent_loss().await;
    let _ = parent_loss.send(reason);
    let _ = shutdown.send(true);
}

async fn wait_for_shutdown_signal(shutdown: watch::Sender<bool>) {
    let termination = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
    match termination {
        Ok(mut termination) => {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    let _ = result;
                }
                _ = termination.recv() => {}
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    let _ = shutdown.send(true);
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::fs;

    use super::capability_enabled;

    #[test]
    fn capability_recheck_fails_closed_before_network_or_listener_work() {
        let journal = tempfile::tempdir().expect("fixture journal");
        assert!(!capability_enabled(journal.path()));
        fs::create_dir_all(journal.path().join("config")).expect("fixture config directory");
        fs::write(journal.path().join("config/journal.json"), "not json")
            .expect("fixture malformed config");
        assert!(!capability_enabled(journal.path()));
    }
}
