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
    if !capability_enabled(&journal_root) {
        return Ok(());
    }
    let Some(owner) = bootstrap_mcp_endpoint_owner_identity(&journal_root)
        .map_err(|_| McpServiceError::Bootstrap)?
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
        Err(_) => return Err(McpServiceError::Tunnel),
    };
    let (tls, forwarder_session) = tunnel.into_service_parts();
    let tls = Arc::new(tls);
    let tls_config = mcp_endpoint_server_config(&tls);
    let resource_origin = format!("https://{}", tls.authorized_hostname());
    let listener = TcpListener::bind(("127.0.0.1", MCP_ENDPOINT_LOOPBACK_PORT))
        .await
        .map_err(|_| McpServiceError::Bind)?;

    let mut tasks = JoinSet::new();
    let listener_shutdown = shutdown_receive.clone();
    let listener_root = Arc::new(journal_root);
    let oauth = Arc::new(crate::oauth::OAuthRuntime::new(
        listener_root.as_path(),
        resource_origin,
    ));
    tasks.spawn(async move {
        crate::server::serve(
            listener,
            tls_config,
            listener_root,
            oauth,
            listener_shutdown,
        )
        .await
        .map_err(|_| McpServiceError::Listener)
    });
    let mut forwarder_shutdown = shutdown_receive.clone();
    tasks.spawn(async move {
        crate::bridge_forwarder::run_session(forwarder_session, &mut forwarder_shutdown)
            .await
            .map_err(|_| McpServiceError::Forwarder)
    });
    let renewal_tls = Arc::clone(&tls);
    let mut renewal_shutdown = shutdown_receive.clone();
    tasks.spawn(async move {
        renewal_tls
            .run_acme_renewal(&mut renewal_shutdown)
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
