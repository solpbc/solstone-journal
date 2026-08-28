// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Birth-admitted parent lifetime for one hosted Journal service.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::Notify;

use super::{
    AdmissionIdentity, DeclaredParent, HostedServiceKind, ParentAdmissionFailure,
    ParentLossAdmissionError, ParentLossLedger, ParentLossPhase, ParentLossReason,
    ParentLossServiceWitnessDrop, ParentWatch, ParentWatchStatus, PlatformParentExitWatcher,
    acknowledge_parent_loss_admission, write_parent_loss_service_witness,
};
use crate::process::{
    HostedLaunchProvenance, InspectResult, ProcessInstance, ProcessInstanceSource,
    SystemProcessInstanceSource, terminate_descendants_exact,
};

const HOSTED_PARENT_ENV: &str = "SOL_SUPERVISOR_SPAWNED";
const PARENT_LOSS_DESCENDANT_TIMEOUT: Duration = Duration::from_secs(2);
const HOSTED_LAUNCH_ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(3);

/// The admitted hosted-service state. It is absent for ordinary, unhosted
/// command invocation so those commands keep their pre-existing lifecycle.
pub struct HostedServiceParentRuntime {
    journal: PathBuf,
    kind: HostedServiceKind,
    parent: ParentWatch,
    instance: ProcessInstance,
    uid: u32,
    admission: AdmissionIdentity,
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
    /// Derive exact generation provenance for one child launched by this
    /// already-admitted hosted service.
    pub fn child_launch_provenance(&self, launch_id: String) -> HostedLaunchProvenance {
        HostedLaunchProvenance {
            journal: self.journal.clone(),
            generation: self.admission.generation,
            launch_id,
            service: None,
            parent_launch_id: Some(self.admission.launch_id.clone()),
            acknowledgement_timeout: HOSTED_LAUNCH_ACKNOWLEDGEMENT_TIMEOUT,
        }
    }

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

    /// A hosted service may write its own shutdown witness during a requested
    /// graceful supervisor stop. The coordinator still authenticates the
    /// control drop with its private capability before it can publish
    /// `retired_expected`; this merely lets the service take its normal
    /// listener/runner cleanup path instead of being killed mid-cleanup.
    pub fn retire_expected_requested(&self) -> bool {
        ParentLossLedger::open(&self.journal)
            .map(|ledger| {
                ledger
                    .generation_path(self.admission.generation)
                    .join("control/retire-expected.json")
                    .is_file()
            })
            .unwrap_or(false)
    }

    /// Stop every still-owned exact descendant after the caller has stopped
    /// its own listener and service runner, then write only this service's
    /// immutable witness. The coordinator owns all terminal adjudication.
    pub fn finish_parent_loss(
        &self,
        shutdown: HostedServiceShutdownEvidence,
    ) -> Result<(), HostedServiceParentLossError> {
        let descendants = terminate_descendants_exact(
            self.instance,
            self.uid,
            PARENT_LOSS_DESCENDANT_TIMEOUT,
            &SystemProcessInstanceSource,
            || {},
        );

        let (descendants_retired, descendant_failure) = match descendants {
            Ok(_) => (true, None),
            Err(failure) => (false, Some(failure)),
        };
        let witness = ParentLossServiceWitnessDrop {
            schema: 1,
            service: self.kind,
            parent: self.parent.instance(),
            identity: self.admission.clone(),
            listener_stopped: shutdown.listener_stopped,
            service_runner_stopped: shutdown.service_runner_stopped,
            operational_artifacts_cleaned: shutdown.operational_artifacts_cleaned,
            descendants_retired,
            shutdown_complete: shutdown.listener_stopped && shutdown.service_runner_stopped,
            descendant_failure,
        };
        write_parent_loss_service_witness(&self.journal, &witness)?;
        Ok(())
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
    #[error("hosted service admission failed: {0}")]
    Admission(#[source] ParentLossAdmissionError),
    #[error("hosted service lifecycle state rejects admission")]
    LifecycleRejected,
}

/// Failure writing the service's own parent-loss evidence drop.
#[derive(Debug, Error)]
pub enum HostedServiceParentLossError {
    #[error("hosted service witness write failed: {0}")]
    Witness(#[from] ParentLossAdmissionError),
}

/// A failure installing the watcher that must make admission fail closed.
#[derive(Debug, Error)]
pub enum HostedServiceWatchError {
    #[error("hosted parent exit watcher failed: {0}")]
    Parent(#[from] super::ParentExitWatchError),
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
    let admission = super::parent_loss_admission::parse_hosted_admission_environment(instance, uid)
        .map_err(HostedServiceAdmissionFailure::Admission)?;
    let ledger = ParentLossLedger::open(journal).map_err(|error| {
        HostedServiceAdmissionFailure::Admission(ParentLossAdmissionError::Ledger(error))
    })?;
    let active = ledger
        .active_generation()
        .map_err(|error| {
            HostedServiceAdmissionFailure::Admission(ParentLossAdmissionError::Ledger(error))
        })?
        .ok_or(HostedServiceAdmissionFailure::LifecycleRejected)?;
    if active.generation != admission.generation
        || active.supervisor != parent.instance()
        || active.phase != ParentLossPhase::Admitting
        || active.coordinator.is_none()
    {
        return Err(HostedServiceAdmissionFailure::LifecycleRejected);
    }
    acknowledge_parent_loss_admission(journal, admission.clone())
        .map_err(HostedServiceAdmissionFailure::Admission)?;

    let watcher = PlatformParentExitWatcher::arm(parent.instance())
        .map_err(|error| HostedServiceAdmissionFailure::Watch(error.into()))?;
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
        admission,
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
