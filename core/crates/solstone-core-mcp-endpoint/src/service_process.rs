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

/// Coordinator phase for the solstone.me ACME account URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AccountPhase {
    SettingUp,
    Ready { url: String, generation: u64 },
    Failed { status: &'static str },
    Rebuilding,
}

fn read_initial_account_file(
    journal_root: &Path,
    environment: solstone_core_journal_config::McpEndpointCertificateEnvironment,
) -> Option<String> {
    #[cfg(unix)]
    {
        let root_jr =
            solstone_core_journal_io::journal_root::JournalRoot::open(journal_root).ok()?;
        let tls_dir = crate::unix::open_tls_state_directory(&root_jr).ok()?;
        let is_prod = matches!(
            environment,
            solstone_core_journal_config::McpEndpointCertificateEnvironment::Production
        );
        let key_bytes = crate::unix::read_tls_acme_account_bytes(&tls_dir, is_prod).ok()??;
        let url_file_bytes =
            crate::unix::read_tls_acme_account_url_bytes(&tls_dir, is_prod).ok()??;
        let url_file =
            serde_json::from_slice::<crate::acme_account::AcmeAccountUrlFile>(&url_file_bytes)
                .ok()?;
        if crate::acme_account::validate_acme_account_url(&url_file.account_url, environment)
            .is_err()
        {
            return None;
        }
        let thumbprint = crate::acme_account::p256_jwk_thumbprint(&key_bytes).ok()?;
        if thumbprint != url_file.thumbprint {
            return None;
        }
        Some(url_file.account_url)
    }
    #[cfg(not(unix))]
    {
        let _ = (journal_root, environment);
        None
    }
}

fn read_account_pair_bytes(
    journal_root: &Path,
    environment: solstone_core_journal_config::McpEndpointCertificateEnvironment,
) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    #[cfg(unix)]
    {
        let Ok(root_jr) = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root)
        else {
            return (None, None);
        };
        let Ok(tls_dir) = crate::unix::open_tls_state_directory(&root_jr) else {
            return (None, None);
        };
        let is_prod = matches!(
            environment,
            solstone_core_journal_config::McpEndpointCertificateEnvironment::Production
        );
        let key_bytes = crate::unix::read_tls_acme_account_bytes(&tls_dir, is_prod)
            .ok()
            .flatten();
        let url_file_bytes = crate::unix::read_tls_acme_account_url_bytes(&tls_dir, is_prod)
            .ok()
            .flatten();
        (key_bytes, url_file_bytes)
    }
    #[cfg(not(unix))]
    {
        let _ = (journal_root, environment);
        (None, None)
    }
}

async fn run_account_coordinator(
    journal_root: Arc<PathBuf>,
    environment: solstone_core_journal_config::McpEndpointCertificateEnvironment,
    phase_send: watch::Sender<AccountPhase>,
    desired_gen_send: watch::Sender<u64>,
    mut applied_gen_receive: Option<watch::Receiver<u64>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), McpServiceError> {
    let mut last_published_pair = read_account_pair_bytes(journal_root.as_path(), environment);
    let mut initial_check_needed = true;
    let mut backoff = std::time::Duration::from_secs(1);
    loop {
        if shutdown_requested(&shutdown) {
            return Ok(());
        }
        let current_phase = phase_send.borrow().clone();
        let need_ensure = initial_check_needed
            || matches!(
                current_phase,
                AccountPhase::SettingUp | AccountPhase::Rebuilding
            );
        if need_ensure {
            initial_check_needed = false;
            let prev_phase = current_phase;
            match crate::acme_account::ensure_acme_account(journal_root.as_path(), environment)
                .await
            {
                crate::acme_account::EnsureAccountOutcome::Ready { account_url } => {
                    backoff = std::time::Duration::from_secs(1);
                    match prev_phase {
                        AccountPhase::Ready { ref url, .. } if url == &account_url => {
                            last_published_pair =
                                read_account_pair_bytes(journal_root.as_path(), environment);
                            crate::owner_state::delete_mcp_account_posture(journal_root.as_path());
                        }
                        AccountPhase::Rebuilding => {
                            let next_generation = desired_gen_send.borrow().wrapping_add(1);
                            desired_gen_send.send_replace(next_generation);
                            if let Some(applied_rx) = applied_gen_receive.as_mut() {
                                while *applied_rx.borrow() != next_generation {
                                    tokio::select! {
                                        changed = shutdown.changed() => {
                                            if changed.is_err() || *shutdown.borrow_and_update() {
                                                return Ok(());
                                            }
                                        }
                                        res = applied_rx.changed() => {
                                            if res.is_err() { break; }
                                        }
                                    }
                                }
                            }
                            last_published_pair =
                                read_account_pair_bytes(journal_root.as_path(), environment);
                            phase_send.send_replace(AccountPhase::Ready {
                                url: account_url,
                                generation: next_generation,
                            });
                            crate::owner_state::delete_mcp_account_posture(journal_root.as_path());
                        }
                        _ => {
                            last_published_pair =
                                read_account_pair_bytes(journal_root.as_path(), environment);
                            phase_send.send_replace(AccountPhase::Ready {
                                url: account_url,
                                generation: *desired_gen_send.borrow(),
                            });
                            crate::owner_state::delete_mcp_account_posture(journal_root.as_path());
                        }
                    }
                }
                crate::acme_account::EnsureAccountOutcome::Failed { status } => {
                    backoff = std::time::Duration::from_secs(1);
                    let next_generation = desired_gen_send.borrow().wrapping_add(1);
                    desired_gen_send.send_replace(next_generation);
                    last_published_pair =
                        read_account_pair_bytes(journal_root.as_path(), environment);
                    phase_send.send_replace(AccountPhase::Failed { status });
                    crate::owner_state::write_mcp_account_posture_state(
                        journal_root.as_path(),
                        status,
                        None,
                        ("done", "failed", "waiting"),
                        None,
                    );
                }
                crate::acme_account::EnsureAccountOutcome::Transient => {
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow_and_update() {
                                return Ok(());
                            }
                        }
                        () = tokio::time::sleep(backoff) => {
                            backoff = (backoff * 2).min(std::time::Duration::from_secs(60));
                            initial_check_needed = true;
                        }
                    }
                    continue;
                }
            }
        }

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    return Ok(());
                }
            }
            () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                let current_pair = read_account_pair_bytes(journal_root.as_path(), environment);
                if current_pair != last_published_pair {
                    phase_send.send_replace(AccountPhase::Rebuilding);
                    let next_generation = desired_gen_send.borrow().wrapping_add(1);
                    desired_gen_send.send_replace(next_generation);
                    if let Some(applied_rx) = applied_gen_receive.as_mut() {
                        while *applied_rx.borrow() != next_generation {
                            tokio::select! {
                                changed = shutdown.changed() => {
                                    if changed.is_err() || *shutdown.borrow_and_update() {
                                        return Ok(());
                                    }
                                }
                                res = applied_rx.changed() => {
                                    if res.is_err() { break; }
                                }
                            }
                        }
                    }
                    initial_check_needed = true;
                }
            }
        }
    }
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

    let cert_env = read_journal_config(&journal_root)
        .ok()
        .and_then(|read| {
            solstone_core_journal_config::mcp_endpoint_certificate_environment(&read).ok()
        })
        .unwrap_or(solstone_core_journal_config::McpEndpointCertificateEnvironment::Production);

    let initial_url = read_initial_account_file(&journal_root, cert_env);
    let initial_phase = match &initial_url {
        Some(url) => AccountPhase::Ready {
            url: url.clone(),
            generation: 1,
        },
        None => {
            crate::owner_state::write_mcp_account_posture_state(
                &journal_root,
                "certificate_account_setup",
                None,
                ("in_progress", "waiting", "waiting"),
                None,
            );
            AccountPhase::SettingUp
        }
    };
    let (account_phase_send, mut account_phase_receive) = watch::channel(initial_phase);
    let (desired_generation_send, desired_generation_receive) = watch::channel(1_u64);
    let (applied_generation_send, applied_generation_receive) = watch::channel(0_u64);

    let coordinator_root = Arc::new(journal_root.clone());
    let coordinator_phase_send = account_phase_send.clone();
    let coordinator_desired_send = desired_generation_send.clone();
    let coordinator_applied_receive = applied_generation_receive.clone();
    let coordinator_shutdown = shutdown_receive.clone();
    let coordinator_task = tokio::spawn(run_account_coordinator(
        coordinator_root,
        cert_env,
        coordinator_phase_send,
        coordinator_desired_send,
        Some(coordinator_applied_receive),
        coordinator_shutdown,
    ));

    let Some(tunnel) = acquire_tunnel(
        &journal_root,
        &mut account_phase_receive,
        &shutdown_receive,
        |account_url| {
            let account_url = account_url.map(str::to_owned);
            let mut attempt_shutdown = shutdown_receive.clone();
            let owner = &owner;
            async move {
                owner
                    .connect_mcp_endpoint_tunnel(account_url.as_deref(), &mut attempt_shutdown)
                    .await
            }
        },
    )
    .await?
    else {
        coordinator_task.abort();
        let _ = coordinator_task.await;
        return Ok(());
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
                    let current_state = crate::owner_state::read_mcp_owner_state(&state_root);
                    if keeps_stored_state(current_state.as_ref(), active, chrono::Utc::now()) {
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
    let forwarder_phase = account_phase_receive.clone();
    tasks.spawn(async move {
        crate::bridge_forwarder::run_bound_session(
            forwarder_owner,
            forwarder_tls,
            forwarder_session,
            forwarder_phase,
            &mut forwarder_shutdown,
        )
        .await
        .map_err(|_| McpServiceError::Forwarder)
    });
    let renewal_tls = Arc::clone(&tls);
    let mut renewal_shutdown = shutdown_receive.clone();
    let mut renewal_desired = desired_generation_receive.clone();
    let renewal_applied = applied_generation_send.clone();
    let renewal_phase = account_phase_send.clone();
    tasks.spawn(async move {
        renewal_tls
            .run_acme_renewal_with_owner_state(
                renewal_root.as_path(),
                Some(&mut renewal_desired),
                Some(&renewal_applied),
                Some(&renewal_phase),
                &mut renewal_shutdown,
            )
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
    coordinator_task.abort();
    let _ = coordinator_task.await;
    shutdown_send.send_replace(true);
    while tasks.join_next().await.is_some() {}
    result
}

/// Wait for the first account-authorized tunnel.
///
/// `Ok(None)` means the service was asked to stop, or the capability was turned
/// off, while waiting. A missing subscription is held for
/// `first_connect_on_carrier_error`'s delay instead of ending the process, so
/// the supervisor does not restart it every few seconds.
async fn acquire_tunnel<T, F, Fut>(
    journal_root: &Path,
    account_phase: &mut watch::Receiver<AccountPhase>,
    shutdown: &watch::Receiver<bool>,
    mut connect: F,
) -> Result<Option<T>, McpServiceError>
where
    F: FnMut(Option<&str>) -> Fut,
    Fut: std::future::Future<Output = Result<T, crate::bridge_carrier::McpBridgeCarrierError>>,
{
    let mut attempt_shutdown = shutdown.clone();
    loop {
        if shutdown_requested(shutdown) || !capability_enabled(journal_root) {
            return Ok(None);
        }
        let current_phase = account_phase.borrow().clone();
        let account_url = match current_phase {
            AccountPhase::Ready { url, .. } => Some(url),
            AccountPhase::SettingUp | AccountPhase::Rebuilding | AccountPhase::Failed { .. } => {
                tokio::select! {
                    changed = account_phase.changed() => {
                        if changed.is_err() {
                            return Ok(None);
                        }
                        continue;
                    }
                    changed = attempt_shutdown.changed() => {
                        if changed.is_err() || *attempt_shutdown.borrow_and_update() {
                            return Ok(None);
                        }
                        continue;
                    }
                }
            }
        };

        match connect(account_url.as_deref()).await {
            Ok(tunnel) => return Ok(Some(tunnel)),
            Err(_) if shutdown_requested(shutdown) => return Ok(None),
            Err(ref error) => match first_connect_on_carrier_error(error) {
                FirstConnectAction::StayAlive(hold, delay) => {
                    let next_attempt = chrono::Utc::now()
                        + chrono::Duration::from_std(delay)
                            .unwrap_or_else(|_| chrono::Duration::seconds(300));
                    crate::owner_state::write_mcp_hold_state(
                        journal_root,
                        hold,
                        None,
                        ("waiting", "waiting", "waiting"),
                        next_attempt,
                    );
                    let check_intent = || {
                        !capability_enabled(journal_root)
                            || check_replace_intent_present(journal_root)
                    };
                    crate::bridge_carrier::wait_until_hold_elapsed_or_replace_intent(
                        delay,
                        &mut attempt_shutdown,
                        check_intent,
                    )
                    .await;
                    if shutdown_requested(shutdown) || !capability_enabled(journal_root) {
                        return Ok(None);
                    }
                }
                FirstConnectAction::ExitTunnel => {
                    crate::owner_state::write_mcp_owner_state(
                        journal_root,
                        "failed",
                        None,
                        ("failed", "waiting", "waiting"),
                        Some(
                            "this computer could not reach services.solstone.app; it will try again when the service restarts",
                        ),
                    );
                    return Err(McpServiceError::Tunnel);
                }
            },
        }
    }
}

fn check_replace_intent_present(journal_root: &Path) -> bool {
    #[cfg(unix)]
    {
        if let Ok(root_jr) = solstone_core_journal_io::JournalRoot::open(journal_root)
            && let Ok(dir) = solstone_core_journal_io::FlatDirectory::open(
                &root_jr,
                std::path::Path::new("mcp-endpoint/tls"),
            )
        {
            solstone_core_journal_io::read_observed_file_bounded(
                &dir,
                std::ffi::OsStr::new(crate::unix::TLS_REPLACE_INTENT_FILE),
                crate::unix::MAX_TLS_REPLACE_INTENT_BYTES,
            )
            .ok()
            .flatten()
            .is_some()
        } else {
            false
        }
    }
    #[cfg(not(unix))]
    {
        let _ = journal_root;
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FirstConnectAction {
    StayAlive(crate::bridge_carrier::RegistrationHold, std::time::Duration),
    ExitTunnel,
}

pub(crate) fn first_connect_on_carrier_error(
    error: &crate::bridge_carrier::McpBridgeCarrierError,
) -> FirstConnectAction {
    if let Some((hold, delay)) = crate::bridge_carrier::registration_hold(error) {
        FirstConnectAction::StayAlive(hold, delay)
    } else {
        FirstConnectAction::ExitTunnel
    }
}

/// A subscription hold outlives its `next_attempt_at` by the longest a retry can
/// take (ten seconds for the account request, ten for the bridge) plus one state
/// writer tick, so the writer does not report the address as open while that
/// retry is still in flight.
const SUBSCRIPTION_RETRY_GRACE_SECONDS: i64 = 30;

/// Whether the periodic state writer must leave the stored state alone.
fn keeps_stored_state(
    state: Option<&crate::owner_state::McpOwnerState>,
    certificate_active: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let Some(state) = state else {
        return false;
    };
    let Some(next_attempt_at) = state.next_attempt_at else {
        return false;
    };
    if matches!(
        state.status.as_str(),
        "needs_subscription"
            | "not_accepted"
            | "journal_update_required"
            | "acme_account_changed"
            | "address_not_ready"
            | "address_refused"
    ) {
        return next_attempt_at + chrono::Duration::seconds(SUBSCRIPTION_RETRY_GRACE_SECONDS) > now;
    }
    state.certificate_leg == "not_this_week" && next_attempt_at > now && !certificate_active
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
    crate::signals::termination_requested().await;
    let _ = shutdown.send(true);
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::sync::watch;
    use tokio::task::JoinHandle;

    use super::{
        AccountPhase, FirstConnectAction, McpServiceError, acquire_tunnel, capability_enabled,
        first_connect_on_carrier_error, keeps_stored_state, read_initial_account_file,
        run_account_coordinator,
    };
    use crate::bridge_carrier::{McpBridgeCarrierError, RegistrationHold};
    use crate::owner_state::read_mcp_owner_state;
    use solstone_core_journal_config::McpEndpointCertificateEnvironment;

    fn enabled_journal() -> tempfile::TempDir {
        let journal = tempfile::TempDir::new_in(crate::test_scratch()).expect("fixture journal");
        fs::create_dir_all(journal.path().join("config")).expect("config directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ep = journal.path().join("mcp-endpoint");
            fs::create_dir_all(&ep).expect("state directory");
            fs::set_permissions(&ep, fs::Permissions::from_mode(0o700)).expect("chmod");
        }
        #[cfg(not(unix))]
        {
            fs::create_dir_all(journal.path().join("mcp-endpoint")).expect("state directory");
        }
        fs::write(
            journal.path().join("config/journal.json"),
            r#"{"mcp_endpoint":{"enabled":true}}"#,
        )
        .expect("enabled config");
        journal
    }

    fn spawn_first_connect(
        root: PathBuf,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<AtomicUsize>,
        first_failure: McpBridgeCarrierError,
    ) -> JoinHandle<Result<Option<()>, McpServiceError>> {
        tokio::spawn(async move {
            let (_phase_tx, mut phase_rx) = watch::channel(AccountPhase::Ready {
                url: "https://acme-staging-v02.api.letsencrypt.org/acme/acct/12345".into(),
                generation: 1,
            });
            acquire_tunnel(&root, &mut phase_rx, &shutdown, |_url| {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                async move {
                    if attempt == 0 {
                        Err(first_failure)
                    } else {
                        Ok(())
                    }
                }
            })
            .await
        })
    }

    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    async fn assert_a_hold_waits_five_minutes(failure: McpBridgeCarrierError, status: &str) {
        let journal = enabled_journal();
        let (_keep, shutdown) = watch::channel(false);
        let attempts = Arc::new(AtomicUsize::new(0));
        let task = spawn_first_connect(
            journal.path().to_path_buf(),
            shutdown,
            Arc::clone(&attempts),
            failure,
        );
        settle().await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        let state = read_mcp_owner_state(journal.path()).expect("owner state");
        assert_eq!(state.status, status);
        let wait = state.next_attempt_at.expect("next attempt") - chrono::Utc::now();
        assert!((295..=300).contains(&wait.num_seconds()), "{wait}");
        assert!(
            !state
                .detail
                .unwrap_or_default()
                .contains("could not reach services.solstone.app")
        );

        tokio::time::advance(Duration::from_secs(299)).await;
        settle().await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "a second attempt inside five minutes"
        );
        assert!(!task.is_finished());

        tokio::time::advance(Duration::from_secs(2)).await;
        settle().await;
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(task.await.expect("task"), Ok(Some(())));
    }

    #[tokio::test(start_paused = true)]
    async fn a_missing_subscription_waits_five_minutes_before_the_next_attempt() {
        assert_a_hold_waits_five_minutes(
            McpBridgeCarrierError::NeedsSubscription,
            "needs_subscription",
        )
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_the_service_did_not_accept_waits_five_minutes_before_the_next_attempt() {
        assert_a_hold_waits_five_minutes(McpBridgeCarrierError::NotAccepted, "not_accepted").await;
    }

    #[tokio::test(start_paused = true)]
    async fn any_other_first_connect_failure_still_ends_the_service_at_once() {
        for error in [
            McpBridgeCarrierError::Account,
            McpBridgeCarrierError::Deadline,
            McpBridgeCarrierError::Connect,
            McpBridgeCarrierError::Tls,
            McpBridgeCarrierError::Io,
            McpBridgeCarrierError::Pop,
            McpBridgeCarrierError::State,
        ] {
            let journal = enabled_journal();
            let (_keep, shutdown) = watch::channel(false);
            let (_phase_tx, mut phase_rx) = watch::channel(AccountPhase::Ready {
                url: "https://acme-staging-v02.api.letsencrypt.org/acme/acct/12345".into(),
                generation: 1,
            });
            let attempts = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&attempts);
            let result = acquire_tunnel(journal.path(), &mut phase_rx, &shutdown, |_url| {
                counter.fetch_add(1, Ordering::SeqCst);
                async move { Err::<(), _>(error) }
            })
            .await;
            assert_eq!(result, Err(McpServiceError::Tunnel), "{error}");
            assert_eq!(attempts.load(Ordering::SeqCst), 1, "{error}");
            let state = read_mcp_owner_state(journal.path()).expect("owner state");
            assert_eq!(state.status, "failed", "{error}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn turning_the_capability_off_ends_the_subscription_wait() {
        let journal = enabled_journal();
        let (_keep, shutdown) = watch::channel(false);
        let attempts = Arc::new(AtomicUsize::new(0));
        let task = spawn_first_connect(
            journal.path().to_path_buf(),
            shutdown,
            Arc::clone(&attempts),
            McpBridgeCarrierError::NeedsSubscription,
        );
        settle().await;
        fs::write(
            journal.path().join("config/journal.json"),
            r#"{"mcp_endpoint":{"enabled":false}}"#,
        )
        .expect("disabled config");
        tokio::time::advance(Duration::from_secs(2)).await;
        settle().await;
        assert!(task.is_finished());
        assert_eq!(task.await.expect("task"), Ok(None));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_state_writer_leaves_a_subscription_hold_alone_while_its_retry_can_still_answer() {
        let journal = enabled_journal();
        let next = chrono::Utc::now() + chrono::Duration::seconds(300);
        crate::owner_state::write_mcp_hold_state(
            journal.path(),
            RegistrationHold::NeedsSubscription,
            Some("aaaqeaye.solstone.me"),
            ("done", "done", "waiting"),
            next,
        );
        let state = read_mcp_owner_state(journal.path()).expect("owner state");
        let at = |seconds: i64| next + chrono::Duration::seconds(seconds);
        assert!(
            keeps_stored_state(Some(&state), true, at(-1)),
            "hold running"
        );
        assert!(
            keeps_stored_state(Some(&state), true, at(10)),
            "retry in flight"
        );
        assert!(
            keeps_stored_state(Some(&state), true, at(29)),
            "retry still bounded"
        );
        assert!(
            !keeps_stored_state(Some(&state), true, at(31)),
            "a retry that ended without rewriting the state means the subscription is active"
        );
        assert!(!keeps_stored_state(None, true, at(0)));
    }

    #[test]
    fn the_state_writer_leaves_a_not_accepted_hold_alone_for_the_same_window() {
        let journal = enabled_journal();
        let next = chrono::Utc::now() + chrono::Duration::seconds(300);
        crate::owner_state::write_mcp_hold_state(
            journal.path(),
            RegistrationHold::NotAccepted,
            None,
            ("waiting", "waiting", "waiting"),
            next,
        );
        let state = read_mcp_owner_state(journal.path()).expect("owner state");
        let at = |seconds: i64| next + chrono::Duration::seconds(seconds);
        assert!(keeps_stored_state(Some(&state), true, at(-1)));
        assert!(keeps_stored_state(Some(&state), true, at(29)));
        assert!(!keeps_stored_state(Some(&state), true, at(31)));
    }

    #[test]
    fn the_certificate_allowance_hold_keeps_its_original_rules() {
        let journal = enabled_journal();
        let next = chrono::Utc::now() + chrono::Duration::seconds(300);
        crate::owner_state::write_mcp_rate_limit_state(
            journal.path(),
            "aaaqeaye.solstone.me",
            next,
        );
        let state = read_mcp_owner_state(journal.path()).expect("owner state");
        let now = next - chrono::Duration::seconds(10);
        assert!(
            keeps_stored_state(Some(&state), false, now),
            "held while no certificate is active"
        );
        assert!(
            !keeps_stored_state(Some(&state), true, now),
            "an active certificate ends the hold"
        );
        assert!(
            !keeps_stored_state(Some(&state), false, next + chrono::Duration::seconds(1)),
            "no grace after the allowance window: the subscription grace does not apply here"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_shutdown_request_ends_the_subscription_wait() {
        let journal = enabled_journal();
        let (send, shutdown) = watch::channel(false);
        let attempts = Arc::new(AtomicUsize::new(0));
        let task = spawn_first_connect(
            journal.path().to_path_buf(),
            shutdown,
            Arc::clone(&attempts),
            McpBridgeCarrierError::NeedsSubscription,
        );
        settle().await;
        send.send_replace(true);
        tokio::time::advance(Duration::from_secs(2)).await;
        settle().await;
        assert!(task.is_finished());
        assert_eq!(task.await.expect("task"), Ok(None));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn capability_recheck_fails_closed_before_network_or_listener_work() {
        let journal = tempfile::tempdir().expect("fixture journal");
        assert!(!capability_enabled(journal.path()));
        fs::create_dir_all(journal.path().join("config")).expect("fixture config directory");
        fs::write(journal.path().join("config/journal.json"), "not json")
            .expect("fixture malformed config");
        assert!(!capability_enabled(journal.path()));
    }

    #[test]
    fn first_connect_error_classification() {
        assert_eq!(
            first_connect_on_carrier_error(&McpBridgeCarrierError::NeedsSubscription),
            FirstConnectAction::StayAlive(
                RegistrationHold::NeedsSubscription,
                Duration::from_secs(300)
            )
        );
        assert_eq!(
            first_connect_on_carrier_error(&McpBridgeCarrierError::NotAccepted),
            FirstConnectAction::StayAlive(RegistrationHold::NotAccepted, Duration::from_secs(300))
        );
        for other_error in [
            McpBridgeCarrierError::Account,
            McpBridgeCarrierError::Cancelled,
            McpBridgeCarrierError::Deadline,
            McpBridgeCarrierError::Connect,
            McpBridgeCarrierError::Tls,
            McpBridgeCarrierError::Io,
            McpBridgeCarrierError::Pop,
            McpBridgeCarrierError::State,
        ] {
            assert_eq!(
                first_connect_on_carrier_error(&other_error),
                FirstConnectAction::ExitTunnel
            );
        }
    }

    struct MockAcmeTransport {
        directory_url: String,
        nonce_url: String,
        account_url: String,
        returned_account_url: String,
    }

    impl crate::acme_account::AcmeTransportSeam for MockAcmeTransport {
        fn exchange(
            &self,
            method: &str,
            url: &str,
            _headers: &[(&str, &str)],
            _body: &[u8],
        ) -> Result<crate::acme_account::RawResponse, crate::acme_account::TransportError> {
            if method == "GET" && url == self.directory_url {
                let body = format!(
                    r#"{{"newNonce":"{}","newAccount":"{}"}}"#,
                    self.nonce_url, self.account_url
                );
                return Ok(crate::acme_account::RawResponse {
                    status: 200,
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: body.into_bytes(),
                });
            }
            if method == "HEAD" && url == self.nonce_url {
                return Ok(crate::acme_account::RawResponse {
                    status: 200,
                    headers: vec![("Replay-Nonce".into(), "test-mock-nonce".into())],
                    body: vec![],
                });
            }
            if method == "POST" && url == self.account_url {
                return Ok(crate::acme_account::RawResponse {
                    status: 201,
                    headers: vec![("Location".into(), self.returned_account_url.clone())],
                    body: b"{}".to_vec(),
                });
            }
            Err(crate::acme_account::TransportError::Unreachable)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn no_bridge_token_build_until_ensure_ready_and_clean_drop() {
        let journal = enabled_journal();
        let cert_env = McpEndpointCertificateEnvironment::Staging;
        assert_eq!(read_initial_account_file(journal.path(), cert_env), None);

        let mock = Arc::new(MockAcmeTransport {
            directory_url: "https://acme-staging-v02.api.letsencrypt.org/directory".into(),
            nonce_url: "https://acme-staging-v02.api.letsencrypt.org/acme/new-nonce".into(),
            account_url: "https://acme-staging-v02.api.letsencrypt.org/acme/new-acct".into(),
            returned_account_url:
                "https://acme-staging-v02.api.letsencrypt.org/acme/acct/987654321".into(),
        });
        *crate::acme_account::TEST_ACME_TRANSPORT.write().unwrap() = Some(mock);

        let (account_phase_send, account_phase_receive) = watch::channel(AccountPhase::SettingUp);
        let (desired_generation_send, _) = watch::channel(1_u64);
        let (shutdown_send, shutdown_receive) = watch::channel(false);

        let coordinator_root = Arc::new(journal.path().to_path_buf());
        let coordinator = tokio::spawn(run_account_coordinator(
            coordinator_root,
            cert_env,
            account_phase_send,
            desired_generation_send,
            None,
            shutdown_receive.clone(),
        ));

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempt_counter = Arc::clone(&attempts);
        let mut phase_rx = account_phase_receive.clone();
        let shut_rx = shutdown_receive.clone();
        let root_buf = journal.path().to_path_buf();
        let connect_task = tokio::spawn(async move {
            acquire_tunnel(&root_buf, &mut phase_rx, &shut_rx, |account_url| {
                let count = attempt_counter.fetch_add(1, Ordering::SeqCst);
                let url = account_url.map(str::to_owned);
                async move {
                    if count == 0 {
                        assert_eq!(
                            url.as_deref(),
                            Some(
                                "https://acme-staging-v02.api.letsencrypt.org/acme/acct/987654321"
                            )
                        );
                    }
                    Ok(())
                }
            })
            .await
        });

        for _ in 0..5 {
            if matches!(*account_phase_receive.borrow(), AccountPhase::Ready { .. }) {
                break;
            }
            tokio::time::advance(Duration::from_secs(1)).await;
            for _ in 0..32 {
                tokio::task::yield_now().await;
            }
        }

        let current_phase = account_phase_receive.borrow().clone();
        match &current_phase {
            AccountPhase::Ready { url, .. } => {
                assert_eq!(
                    url, "https://acme-staging-v02.api.letsencrypt.org/acme/acct/987654321",
                    "unexpected URL in phase {current_phase:?}"
                );
            }
            _ => panic!("expected Ready phase, got {current_phase:?}"),
        }

        for _ in 0..5 {
            if connect_task.is_finished() {
                break;
            }
            tokio::time::advance(Duration::from_secs(1)).await;
            for _ in 0..32 {
                tokio::task::yield_now().await;
            }
        }

        let result = connect_task.await.expect("connect task finished");
        assert_eq!(result, Ok(Some(())));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        shutdown_send.send_replace(true);
        let coord_res = coordinator.await.expect("coordinator joined");
        assert_eq!(coord_res, Ok(()));
        *crate::acme_account::TEST_ACME_TRANSPORT.write().unwrap() = None;
    }
}
