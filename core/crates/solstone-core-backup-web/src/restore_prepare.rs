// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{http::StatusCode, response::Response};

use crate::{
    operation::{self, SharedOperationSlot},
    response,
};

/// How long an abandoned lease that never advanced past `Prepared` (nobody entered a
/// recovery key yet) may block a fresh `prepare()` attempt before a new attempt reclaims
/// it. Unchanged in value from the prior borrowed constant -- only the reclaim role ever
/// needed to be this short.
pub(crate) const RESTORE_PREPARE_RECLAIM_WINDOW: Duration = Duration::from_secs(15);

/// How long an owner has, from `prepare()`, to read the portal handoff and complete
/// consent (key -> arm -> activate) before the lease is considered dead. Real read-plus-click
/// time, not a retry bound. Stays under the Worker's 5-minute `HANDOFF_TTL_MS` service-side
/// handoff-row expiry (`account/src/enable-constants.js`) so the lease never outlives the
/// row it depends on.
pub(crate) const RESTORE_PREPARE_CONSENT_WINDOW: Duration = Duration::from_secs(180);

pub(crate) type SharedRestorePrepare = Arc<Mutex<Option<RestorePrepareLease>>>;

pub(crate) struct RestorePrepareLease {
    token: String,
    issued_at: Instant,
    baseline_generation: u64,
    stage: RestorePrepareStage,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestorePrepareStage {
    Prepared,
    Keyed,
    Armed,
    Activated,
}

pub(crate) struct Prepared {
    pub(crate) capability: String,
}

pub(crate) struct Keyed {
    pub(crate) portal_url: String,
}

pub(crate) enum Activation {
    Spawn { nonce: String, generation: u64 },
    AlreadyActivated,
}

pub(crate) fn new_shared() -> SharedRestorePrepare {
    Arc::new(Mutex::new(None))
}

pub(crate) fn prepare(
    shared: &SharedRestorePrepare,
    operations: &SharedOperationSlot,
) -> Result<Prepared, Response> {
    reconcile(shared, operations);
    let mut guard = shared.lock().expect("restore prepare lock");
    if guard.is_some() {
        return Err(refusal(
            "restore_prepare_unavailable",
            "A hosted restore handoff is already being prepared.",
        ));
    }
    if operation::is_busy(operations) {
        return Err(operation::busy_response());
    }
    let capability = operation::mint_capability().map_err(|_| {
        response::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't prepare the hosted restore handoff.",
            "failed",
            "",
        )
    })?;
    let baseline_generation = operation::generation_of(operations).unwrap_or(0);
    *guard = Some(RestorePrepareLease {
        token: capability.clone(),
        issued_at: Instant::now(),
        baseline_generation,
        stage: RestorePrepareStage::Prepared,
    });
    Ok(Prepared { capability })
}

pub(crate) fn key<F>(
    shared: &SharedRestorePrepare,
    operations: &SharedOperationSlot,
    capability: &str,
    recovery_key: String,
    mint_portal: F,
) -> Result<Keyed, Response>
where
    F: FnOnce() -> Result<(String, String), Response>,
{
    if take_expired(shared, operations).is_some() {
        return Err(expired_response());
    }
    let mut guard = shared.lock().expect("restore prepare lock");
    let lease = guard.as_mut().ok_or_else(invalid_capability_response)?;
    require_capability(lease, capability)?;
    if lease.stage != RestorePrepareStage::Prepared {
        return Err(wrong_stage_response());
    }
    if operation::generation_of(operations).unwrap_or(0) != lease.baseline_generation
        || operation::is_busy(operations)
    {
        return Err(refusal(
            "restore_prepare_generation_changed",
            "The hosted restore handoff is no longer current.",
        ));
    }

    let (nonce, portal_url) = mint_portal()?;
    let started = operation::begin(
        operations,
        "restore_hosted",
        Some(portal_url.clone()),
        Some(nonce),
        Some(recovery_key),
    )?;
    lease.baseline_generation = started.generation;
    lease.stage = RestorePrepareStage::Keyed;
    Ok(Keyed { portal_url })
}

pub(crate) fn arm(
    shared: &SharedRestorePrepare,
    operations: &SharedOperationSlot,
    capability: &str,
) -> Result<(), Response> {
    if take_expired(shared, operations).is_some() {
        return Err(expired_response());
    }
    let mut guard = shared.lock().expect("restore prepare lock");
    let lease = guard.as_mut().ok_or_else(invalid_capability_response)?;
    require_capability(lease, capability)?;
    match lease.stage {
        RestorePrepareStage::Keyed | RestorePrepareStage::Armed => {
            require_live_generation(lease, operations)?;
            lease.stage = RestorePrepareStage::Armed;
        }
        RestorePrepareStage::Prepared | RestorePrepareStage::Activated => {
            return Err(wrong_stage_response());
        }
    }
    Ok(())
}

pub(crate) fn activate(
    shared: &SharedRestorePrepare,
    operations: &SharedOperationSlot,
    capability: &str,
) -> Result<Activation, Response> {
    if take_expired(shared, operations).is_some() {
        return Err(expired_response());
    }
    let mut guard = shared.lock().expect("restore prepare lock");
    let lease = guard.as_mut().ok_or_else(invalid_capability_response)?;
    require_capability(lease, capability)?;
    match lease.stage {
        RestorePrepareStage::Armed => {
            require_live_generation(lease, operations)?;
            let nonce = operation::nonce_for_generation(operations, lease.baseline_generation)
                .ok_or_else(wrong_stage_response)?;
            lease.stage = RestorePrepareStage::Activated;
            Ok(Activation::Spawn {
                nonce,
                generation: lease.baseline_generation,
            })
        }
        RestorePrepareStage::Activated => {
            require_live_generation(lease, operations)?;
            Ok(Activation::AlreadyActivated)
        }
        RestorePrepareStage::Prepared | RestorePrepareStage::Keyed => Err(wrong_stage_response()),
    }
}

pub(crate) fn cancel(
    shared: &SharedRestorePrepare,
    operations: &SharedOperationSlot,
    capability: &str,
) -> Result<(), Response> {
    if take_expired(shared, operations).is_some() {
        return Err(expired_response());
    }
    let mut guard = shared.lock().expect("restore prepare lock");
    let lease = guard.as_ref().ok_or_else(invalid_capability_response)?;
    require_capability(lease, capability)?;
    let stage = lease.stage;
    let generation = lease.baseline_generation;
    if stage != RestorePrepareStage::Prepared {
        require_live_generation(lease, operations)?;
        operation::mark_cancelled(operations, generation);
    }
    *guard = None;
    Ok(())
}

pub(crate) fn reconcile(shared: &SharedRestorePrepare, operations: &SharedOperationSlot) {
    let _ = take_expired(shared, operations);
    let should_drop_terminal = {
        let guard = shared.lock().expect("restore prepare lock");
        guard.as_ref().is_some_and(|lease| {
            lease.stage != RestorePrepareStage::Prepared && !operation::is_busy(operations)
        })
    };
    if should_drop_terminal {
        *shared.lock().expect("restore prepare lock") = None;
    }
}

fn take_expired(
    shared: &SharedRestorePrepare,
    operations: &SharedOperationSlot,
) -> Option<RestorePrepareLease> {
    let expired = {
        let mut guard = shared.lock().expect("restore prepare lock");
        guard
            .as_ref()
            .and_then(|lease| {
                expiry_window(lease.stage).map(|window| lease.issued_at.elapsed() >= window)
            })
            .unwrap_or(false)
            .then(|| guard.take())
            .flatten()
    };
    if let Some(lease) = expired.as_ref()
        && lease.stage != RestorePrepareStage::Prepared
    {
        operation::mark_prepare_lease_expired(operations, lease.baseline_generation);
    }
    expired
}

/// Returns the lease's own reap window, or `None` if this stage's lifetime is governed
/// elsewhere. `Activated` is never reaped here — an activated lease's lifetime is governed
/// by `operation::HANDOFF_TTL` via `handoff_poll`, not by `restore_prepare`'s own timeout.
fn expiry_window(stage: RestorePrepareStage) -> Option<Duration> {
    match stage {
        RestorePrepareStage::Prepared => Some(RESTORE_PREPARE_RECLAIM_WINDOW),
        RestorePrepareStage::Keyed | RestorePrepareStage::Armed => {
            Some(RESTORE_PREPARE_CONSENT_WINDOW)
        }
        RestorePrepareStage::Activated => None,
    }
}

fn require_capability(lease: &RestorePrepareLease, capability: &str) -> Result<(), Response> {
    if lease.token == capability {
        Ok(())
    } else {
        Err(invalid_capability_response())
    }
}

fn require_live_generation(
    lease: &RestorePrepareLease,
    operations: &SharedOperationSlot,
) -> Result<(), Response> {
    if operation::generation_of(operations) == Some(lease.baseline_generation)
        && operation::is_busy(operations)
    {
        Ok(())
    } else {
        Err(refusal(
            "restore_prepare_generation_changed",
            "The hosted restore handoff is no longer current.",
        ))
    }
}

fn invalid_capability_response() -> Response {
    refusal(
        "restore_prepare_invalid_capability",
        "That hosted restore handoff capability is not valid.",
    )
}

fn expired_response() -> Response {
    refusal(
        "restore_prepare_expired",
        "That hosted restore handoff capability has expired.",
    )
}

fn wrong_stage_response() -> Response {
    refusal(
        "restore_prepare_wrong_stage",
        "That hosted restore handoff is not ready for this step.",
    )
}

fn refusal(reason_code: &str, detail: &str) -> Response {
    response::error(
        StatusCode::CONFLICT,
        "I couldn't complete the hosted restore handoff.",
        reason_code,
        detail,
    )
}

#[cfg(test)]
pub(crate) fn backdate_restore_prepare_issued_at(shared: &SharedRestorePrepare, age: Duration) {
    let mut guard = shared.lock().expect("restore prepare lock");
    if let Some(lease) = guard.as_mut() {
        lease.issued_at = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
    }
}

#[cfg(test)]
mod tests {
    use super::RESTORE_PREPARE_CONSENT_WINDOW;
    use std::time::Duration;

    #[test]
    fn consent_window_stays_under_the_worker_handoff_row_ttl() {
        assert!(RESTORE_PREPARE_CONSENT_WINDOW < Duration::from_secs(5 * 60));
    }
}
