// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Birth-admitted parent lifetime for one hosted Journal service.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::Notify;

#[cfg(target_os = "macos")]
use super::darwin_parent_watch::{DarwinParentExitWatcher, DarwinParentWatchError};
use super::{
    DeclaredParent, HostedServiceKind, ParentAdmissionFailure, ParentLossHandoffError,
    ParentLossHandoffPublishResult, ParentLossHandoffUnresolvedReason, ParentLossReason,
    ParentLossServiceRegistration, ParentLossServiceWitness, ParentWatch, ParentWatchStatus,
    finalize_parent_loss_handoff, read_parent_loss_handoff, record_parent_loss_service_unresolved,
    record_parent_loss_service_witness, register_parent_loss_service,
};
use crate::process::{
    InspectResult, ProcessInstance, ProcessInstanceSource, SystemProcessInstanceSource,
    terminate_descendants_exact,
};

const HOSTED_PARENT_ENV: &str = "SOL_SUPERVISOR_SPAWNED";
const PARENT_WATCH_INTERVAL: Duration = Duration::from_secs(1);
const PARENT_LOSS_DESCENDANT_TIMEOUT: Duration = Duration::from_secs(2);
const PARENT_LOSS_HANDOFF_WAIT: Duration = Duration::from_secs(3);

/// The admitted hosted-service state. It is absent for ordinary, unhosted
/// command invocation so those commands keep their pre-existing lifecycle.
pub struct HostedServiceParentRuntime {
    journal: PathBuf,
    kind: HostedServiceKind,
    parent: ParentWatch,
    instance: ProcessInstance,
    uid: u32,
    loss_signal: Arc<ParentLossSignal>,
}

/// Service-owned shutdown facts captured before its parent-loss witness is
/// published. The service, rather than the generic admission layer, owns
/// these observations because only it knows its listener and runner topology.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostedServiceShutdownEvidence {
    pub listener_stopped: bool,
    pub service_runner_stopped: bool,
    pub operational_artifacts_cleaned: bool,
}

impl HostedServiceParentRuntime {
    /// Wait until the armed platform watcher confirms (or fail-closed cannot
    /// verify) the admitted parent has gone away.
    pub async fn await_parent_loss(&self) -> ParentLossReason {
        loop {
            let notified = self.loss_signal.notify.notified();
            if let Some(reason) = *self
                .loss_signal
                .reason
                .lock()
                .expect("hosted parent-loss signal lock poisoned")
            {
                return reason;
            }
            notified.await;
        }
    }

    /// Stop every still-owned exact descendant after the caller has stopped
    /// its own listener and service runner, then publish this service's
    /// terminal handoff evidence.
    pub fn finish_parent_loss(
        &self,
        parent_loss: ParentLossReason,
        shutdown: HostedServiceShutdownEvidence,
    ) -> Result<ParentLossHandoffPublishResult, ParentLossHandoffError> {
        let descendants = terminate_descendants_exact(
            self.instance,
            self.uid,
            PARENT_LOSS_DESCENDANT_TIMEOUT,
            &SystemProcessInstanceSource,
            || {},
        );

        if matches!(parent_loss, ParentLossReason::Unverifiable) {
            return record_parent_loss_service_unresolved(
                &self.journal,
                self.parent.instance(),
                self.kind,
                ParentLossHandoffUnresolvedReason::ParentUnverifiable,
            );
        }
        if !shutdown.operational_artifacts_cleaned {
            return record_parent_loss_service_unresolved(
                &self.journal,
                self.parent.instance(),
                self.kind,
                ParentLossHandoffUnresolvedReason::ArtifactFailure,
            );
        }

        let (descendants_retired, census_complete, descendant_failure) = match descendants {
            Ok(_) => (true, true, None),
            Err(failure) => (false, false, Some(failure)),
        };
        let witness = ParentLossServiceWitness {
            parent: self.parent.instance(),
            instance: self.instance,
            uid: self.uid,
            listener_stopped: shutdown.listener_stopped,
            service_runner_stopped: shutdown.service_runner_stopped,
            initial_census_complete: census_complete,
            post_term_census_complete: census_complete,
            final_census_complete: census_complete,
            descendants_retired,
            shutdown_complete: shutdown.listener_stopped && shutdown.service_runner_stopped,
            descendant_failure,
        };
        let result = record_parent_loss_service_witness(
            &self.journal,
            self.parent.instance(),
            self.kind,
            witness,
        )?;
        if !matches!(result, ParentLossHandoffPublishResult::Recorded) {
            return Ok(result);
        }
        self.wait_for_peer_witnesses()
    }

    fn wait_for_peer_witnesses(
        &self,
    ) -> Result<ParentLossHandoffPublishResult, ParentLossHandoffError> {
        let deadline = Instant::now() + PARENT_LOSS_HANDOFF_WAIT;
        while Instant::now() < deadline {
            if read_parent_loss_handoff(&self.journal)?.is_some() {
                return Ok(ParentLossHandoffPublishResult::RejectedTerminal);
            }
            thread::sleep(Duration::from_millis(25));
        }
        finalize_parent_loss_handoff(&self.journal, self.parent.instance())
    }
}

/// The typed reason hosted service admission refused to start service work.
#[derive(Debug, Error)]
pub enum HostedServiceAdmissionFailure {
    #[error("hosted service parent admission failed: {0:?}")]
    Parent(ParentAdmissionFailure),
    #[error("hosted service parent was lost before serving: {0:?}")]
    ParentLostBeforeServing(ParentLossReason),
    #[error("hosted service parent watch registration failed: {0}")]
    Watch(#[source] HostedServiceWatchError),
    #[error("hosted service could not verify its own process identity")]
    SelfUnverifiable,
    #[error("hosted service handoff registration failed: {0}")]
    Handoff(#[source] ParentLossHandoffError),
}

/// A failure installing the watcher that must make admission fail closed.
#[derive(Debug, Error)]
pub enum HostedServiceWatchError {
    #[cfg(target_os = "macos")]
    #[error("Darwin kqueue parent watch failed: {0}")]
    Darwin(#[from] DarwinParentWatchError),
    #[error("could not start hosted parent watcher: {0}")]
    Thread(#[source] std::io::Error),
}

/// Admit and arm the hosted-parent contract. A missing or non-`"1"` marker
/// deliberately leaves the service unhosted.
pub fn admit_hosted_service_parent(
    journal: &Path,
    kind: HostedServiceKind,
) -> Result<Option<HostedServiceParentRuntime>, HostedServiceAdmissionFailure> {
    if std::env::var_os(HOSTED_PARENT_ENV).as_deref() != Some(OsStr::new("1")) {
        return Ok(None);
    }

    let declared =
        DeclaredParent::capture_current().map_err(HostedServiceAdmissionFailure::Parent)?;
    let source = SystemProcessInstanceSource;
    let parent =
        ParentWatch::admit(declared, &source).map_err(HostedServiceAdmissionFailure::Parent)?;
    let (instance, uid) = match source.inspect(std::process::id()) {
        InspectResult::Present { instance, uid, .. } => (instance, uid),
        InspectResult::Absent | InspectResult::Unverifiable => {
            return Err(HostedServiceAdmissionFailure::SelfUnverifiable);
        }
    };
    let registration = register_parent_loss_service(
        journal,
        parent.instance(),
        kind,
        ParentLossServiceRegistration { instance, uid },
    )
    .map_err(HostedServiceAdmissionFailure::Handoff)?;
    if !matches!(registration, ParentLossHandoffPublishResult::Recorded) {
        return Err(HostedServiceAdmissionFailure::Handoff(
            ParentLossHandoffUnresolvedReason::ArtifactFailure.into(),
        ));
    }

    let watcher = PlatformParentExitWatcher::arm(parent.instance())
        .map_err(HostedServiceAdmissionFailure::Watch)?;
    if let ParentWatchStatus::Lost(reason) = parent.check(&source) {
        return Err(HostedServiceAdmissionFailure::ParentLostBeforeServing(
            reason,
        ));
    }
    let loss_signal = Arc::new(ParentLossSignal::default());
    let watcher_signal = Arc::clone(&loss_signal);
    thread::Builder::new()
        .name("hosted-parent-watch".to_owned())
        .spawn(move || watcher_signal.publish(watcher.wait_for_loss(parent)))
        .map_err(HostedServiceWatchError::Thread)
        .map_err(HostedServiceAdmissionFailure::Watch)?;

    Ok(Some(HostedServiceParentRuntime {
        journal: journal.to_path_buf(),
        kind,
        parent,
        instance,
        uid,
        loss_signal,
    }))
}

#[derive(Default)]
struct ParentLossSignal {
    reason: Mutex<Option<ParentLossReason>>,
    notify: Notify,
}

impl ParentLossSignal {
    fn publish(&self, reason: ParentLossReason) {
        *self
            .reason
            .lock()
            .expect("hosted parent-loss signal lock poisoned") = Some(reason);
        self.notify.notify_waiters();
    }
}

enum PlatformParentExitWatcher {
    #[cfg(target_os = "macos")]
    Darwin(DarwinParentExitWatcher),
    Poll,
}

impl PlatformParentExitWatcher {
    fn arm(parent: ProcessInstance) -> Result<Self, HostedServiceWatchError> {
        #[cfg(target_os = "macos")]
        {
            return DarwinParentExitWatcher::register(parent)
                .map(Self::Darwin)
                .map_err(HostedServiceWatchError::from);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = parent;
            Ok(Self::Poll)
        }
    }

    fn wait_for_loss(self, parent: ParentWatch) -> ParentLossReason {
        match self {
            #[cfg(target_os = "macos")]
            Self::Darwin(watcher) => match watcher.wait_for_exit() {
                Ok(()) => match parent.check(&SystemProcessInstanceSource) {
                    ParentWatchStatus::Lost(reason) => reason,
                    ParentWatchStatus::Live => ParentLossReason::Unverifiable,
                },
                Err(_) => ParentLossReason::Unverifiable,
            },
            Self::Poll => loop {
                thread::sleep(PARENT_WATCH_INTERVAL);
                match parent.check(&SystemProcessInstanceSource) {
                    ParentWatchStatus::Live => {}
                    ParentWatchStatus::Lost(reason) => return reason,
                }
            },
        }
    }
}
