// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use solstone_core_backup::HostedBinding;
use solstone_core_backup::archive_admission::ArchiveCancellation;
use solstone_core_backup_runtime::hosted_runtime::HttpError;
use solstone_core_backup_runtime::{HttpRequest, HttpResponse};

use crate::operation::{self, SharedOperationSlot};
use crate::validation::{self, RefusedReason};
use crate::{BackupWebDeps, persist_and_consume_hosted};

pub const HANDOFF_POLL_TIMEOUT: Duration = Duration::from_secs(15);
pub const HANDOFF_POLL_INTERVAL: Duration = Duration::from_secs(1);
pub const HANDOFF_WATCHDOG_TICK: Duration = Duration::from_millis(100);

enum HandoffPollOutcome {
    Approved(HostedBinding),
    NeedsSubscription,
    Refused(RefusedReason),
}

enum LiveWait {
    Gone,
    Expired,
    Live { remaining: Duration },
}

pub(crate) fn poll_url(portal_base: &str, nonce: &str) -> String {
    format!(
        "{}/handoff/backup?nonce={nonce}",
        portal_base.trim_end_matches('/')
    )
}

struct PollLease {
    flag: Arc<AtomicBool>,
}

impl PollLease {
    fn try_acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self {
                flag: Arc::clone(flag),
            })
    }
}

impl Drop for PollLease {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

pub(crate) fn spawn(deps: BackupWebDeps, nonce: String, generation: u64) {
    let Some(lease) = PollLease::try_acquire(&deps.handoff_poll_lease) else {
        return;
    };
    if validation::require_configured_portal_base(&deps.portal_base).is_err() {
        // The refusal is recorded on the poll slot. No poll thread is started.
        if operation::start_handoff_poll(&deps.operations, generation).is_some() {
            finish_error(&deps, &nonce, generation, "failed");
            operation::finish_handoff_poll(&deps.operations, generation);
        }
        drop(lease);
        return;
    }
    let mut guard = match deps.operations.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.closed {
        if let Some(current) = guard.current.as_mut()
            && current.generation == generation
            && !operation::is_terminal(&current.view.phase)
            && !current.worker_active
        {
            operation::finish_or_defer(current, operation::Terminal::error("failed"));
        }
        drop(guard);
        deps.operations.notify_waiters();
        return;
    }
    let Some(cancellation) = operation::start_handoff_poll_locked(&mut guard, generation) else {
        return;
    };
    let poll_deps = deps.clone();
    let failed_nonce = nonce.clone();
    let poll_started = spawn_poll_thread(move || {
        let panicked = panic::catch_unwind(AssertUnwindSafe(|| {
            poll_loop(&poll_deps, &nonce, generation, &cancellation);
        }));
        if panicked.is_err() {
            finish_error(&poll_deps, &nonce, generation, "failed");
        }
        drop(lease);
        operation::finish_handoff_poll(&poll_deps.operations, generation);
    });
    let poll_handle = match poll_started {
        Ok(handle) => handle,
        Err(_) => {
            drop(guard);
            finish_error(&deps, &failed_nonce, generation, "failed");
            operation::finish_handoff_poll(&deps.operations, generation);
            return;
        }
    };
    guard.threads.push(poll_handle);

    let watchdog_slot = deps.operations.clone();
    let watchdog_started =
        thread::Builder::new().spawn(move || watchdog_loop(&watchdog_slot, generation));
    match watchdog_started {
        Ok(handle) => {
            guard.threads.push(handle);
        }
        Err(_) => {
            drop(guard);
            finish_error(&deps, &failed_nonce, generation, "failed");
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static FAIL_POLL_START: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(not(windows))]
#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn fail_next_poll_start() {
    FAIL_POLL_START.with(|fail| fail.set(true));
}

fn spawn_poll_thread(
    work: impl FnOnce() + Send + 'static,
) -> std::io::Result<thread::JoinHandle<()>> {
    #[cfg(all(test, feature = "full-tests"))]
    if FAIL_POLL_START.with(|fail| fail.replace(false)) {
        return Err(std::io::Error::other(
            "injected hosted poll creation failure",
        ));
    }
    thread::Builder::new().spawn(work)
}

fn poll_loop(
    deps: &BackupWebDeps,
    nonce: &str,
    generation: u64,
    cancellation: &ArchiveCancellation,
) {
    loop {
        let remaining = match live_wait(&deps.operations, generation) {
            LiveWait::Gone => return,
            LiveWait::Expired => {
                operation::mark_expired(&deps.operations, generation);
                return;
            }
            LiveWait::Live { remaining } => remaining,
        };
        let request = HttpRequest {
            method: "GET".into(),
            url: poll_url(&deps.portal_base, nonce),
            headers: vec![
                (
                    "User-Agent".into(),
                    format!("solstone-backup/{}", deps.version),
                ),
                ("Connection".into(), "close".into()),
            ],
            body: Vec::new(),
            timeout: HANDOFF_POLL_TIMEOUT.min(remaining),
        };
        match deps.http.execute_cancellable(&request, cancellation) {
            Err(HttpError::Timeout) => {
                if !sleep_retry(deps, generation) {
                    return;
                }
            }
            Err(HttpError::Unreachable) => {
                finish_error(deps, nonce, generation, "unreachable");
                return;
            }
            Err(HttpError::Other) => {
                finish_error(deps, nonce, generation, "failed");
                return;
            }
            Ok(HttpResponse { status: 204, .. }) => {
                if !sleep_retry(deps, generation) {
                    return;
                }
            }
            Ok(HttpResponse { status: 410, .. }) => {
                finish_error(deps, nonce, generation, "expired");
                return;
            }
            Ok(HttpResponse {
                status: 200, body, ..
            }) => match parse_poll_body(&body, &deps.portal_base) {
                Ok(HandoffPollOutcome::Approved(binding)) => {
                    apply_approved(deps, nonce, generation, binding);
                    return;
                }
                Ok(HandoffPollOutcome::NeedsSubscription) => {
                    apply_needs_subscription(deps, nonce, generation);
                    return;
                }
                Ok(HandoffPollOutcome::Refused(reason)) => {
                    apply_refused(deps, nonce, generation, reason);
                    return;
                }
                Err(()) => {
                    finish_error(deps, nonce, generation, "failed");
                    return;
                }
            },
            Ok(_) => {
                finish_error(deps, nonce, generation, "failed");
                return;
            }
        }
    }
}

fn watchdog_loop(slot: &SharedOperationSlot, generation: u64) {
    loop {
        thread::sleep(HANDOFF_WATCHDOG_TICK);
        match live_wait(slot, generation) {
            LiveWait::Gone => return,
            LiveWait::Expired => {
                operation::mark_expired(slot, generation);
                return;
            }
            LiveWait::Live { .. } => {}
        }
    }
}

fn live_wait(slot: &SharedOperationSlot, generation: u64) -> LiveWait {
    let guard = slot.lock().expect("operation slot lock");
    let Some(current) = guard.current.as_ref() else {
        return LiveWait::Gone;
    };
    if current.generation != generation
        || operation::is_terminal(&current.view.phase)
        || current.nonce.is_none()
        || !matches!(
            current.view.kind.as_str(),
            "enable_hosted" | "restore_hosted"
        )
    {
        return LiveWait::Gone;
    }
    let elapsed = current.started.elapsed();
    if elapsed >= operation::HANDOFF_TTL {
        return LiveWait::Expired;
    }
    LiveWait::Live {
        remaining: operation::HANDOFF_TTL - elapsed,
    }
}

fn sleep_retry(deps: &BackupWebDeps, generation: u64) -> bool {
    match live_wait(&deps.operations, generation) {
        LiveWait::Gone => false,
        LiveWait::Expired => {
            operation::mark_expired(&deps.operations, generation);
            false
        }
        LiveWait::Live { remaining } => {
            thread::sleep(HANDOFF_POLL_INTERVAL.min(remaining));
            true
        }
    }
}

fn finish_error(deps: &BackupWebDeps, nonce: &str, generation: u64, reason: &str) {
    operation::fail_handoff_poll(&deps.operations, generation, nonce, reason);
}

#[cfg(all(test, feature = "full-tests"))]
static APPLY_APPROVED_PAUSE_HOOK: std::sync::Mutex<
    Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
> = std::sync::Mutex::new(None);

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn arm_apply_approved_pause()
-> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *APPLY_APPROVED_PAUSE_HOOK.lock().unwrap() = Some((entered_tx, release_rx));
    (entered_rx, release_tx)
}

#[cfg(all(test, feature = "full-tests"))]
fn pause_apply_approved() {
    let pair = {
        let mut guard = APPLY_APPROVED_PAUSE_HOOK.lock().unwrap();
        guard.take()
    };
    if let Some((entered_tx, release_rx)) = pair {
        let _ = entered_tx.send(());
        let _ = release_rx.recv();
    }
}

fn apply_approved(deps: &BackupWebDeps, nonce: &str, generation: u64, binding: HostedBinding) {
    #[cfg(all(test, feature = "full-tests"))]
    pause_apply_approved();
    match live_wait(&deps.operations, generation) {
        LiveWait::Gone => return,
        LiveWait::Expired => {
            operation::mark_expired(&deps.operations, generation);
            return;
        }
        LiveWait::Live { .. } => {}
    }
    if let Ok(matched) = operation::match_handoff(&deps.operations, nonce, Some(generation)) {
        persist_and_consume_hosted(
            deps,
            matched.generation,
            matched.kind,
            binding,
            matched.restore_key,
        );
    }
}

fn apply_needs_subscription(deps: &BackupWebDeps, nonce: &str, generation: u64) {
    match live_wait(&deps.operations, generation) {
        LiveWait::Gone => return,
        LiveWait::Expired => {
            operation::mark_expired(&deps.operations, generation);
            return;
        }
        LiveWait::Live { .. } => {}
    }
    if operation::match_handoff(&deps.operations, nonce, Some(generation)).is_ok() {
        operation::mark_needs_subscription(&deps.operations, generation);
    }
}

fn apply_refused(deps: &BackupWebDeps, nonce: &str, generation: u64, reason: RefusedReason) {
    match live_wait(&deps.operations, generation) {
        LiveWait::Gone => return,
        LiveWait::Expired => {
            operation::mark_expired(&deps.operations, generation);
            return;
        }
        LiveWait::Live { .. } => {}
    }
    if operation::match_handoff(&deps.operations, nonce, Some(generation)).is_ok() {
        operation::mark_refused(&deps.operations, generation, reason);
    }
}

fn parse_poll_body(body: &[u8], portal_base: &str) -> Result<HandoffPollOutcome, ()> {
    let value = serde_json::from_slice::<Value>(body).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    match object.get("status").and_then(Value::as_str) {
        Some("approved") => Ok(HandoffPollOutcome::Approved(approved_binding(
            object,
            portal_base,
        )?)),
        Some("needs_subscription") => {
            let _ = approved_binding(object, portal_base)?;
            let url = validation::nonempty_string(object, "subscribe_url").map_err(|_| ())?;
            validation::require_https_portal_url(&url, portal_base).map_err(|_| ())?;
            Ok(HandoffPollOutcome::NeedsSubscription)
        }
        Some("refused") => Ok(HandoffPollOutcome::Refused(
            validation::refused_reason(object).map_err(|_| ())?,
        )),
        _ => Err(()),
    }
}

fn approved_binding(
    object: &serde_json::Map<String, Value>,
    portal_base: &str,
) -> Result<HostedBinding, ()> {
    let binding = validation::hosted_binding(object).map_err(|_| ())?;
    validation::require_portal_origin(&binding.broker_endpoint, portal_base).map_err(|_| ())?;
    Ok(binding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn approved_body() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "status": "approved",
            "nonce": "SHOULD-BE-IGNORED",
            "broker_endpoint": crate::test_support::PORTAL_BASE,
            "account_id": "account",
            "instance_id": "instance",
            "bucket": "bucket",
            "prefix": "owner/prefix",
            "broker_token": "broker-token-secret"
        }))
        .unwrap()
    }

    fn needs_subscription_body() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "status": "needs_subscription",
            "subscribe_url": format!("{}/services/backup", crate::test_support::PORTAL_BASE),
            "broker_endpoint": crate::test_support::PORTAL_BASE,
            "account_id": "account",
            "instance_id": "instance",
            "bucket": "bucket",
            "prefix": "owner/prefix",
            "broker_token": "broker-token-secret"
        }))
        .unwrap()
    }

    #[test]
    fn poll_url_is_handoff_backup_without_instance() {
        let url = poll_url("https://services.solstone.app/", "ABC123");
        assert_eq!(
            url,
            "https://services.solstone.app/handoff/backup?nonce=ABC123"
        );
        assert!(!url.contains("instance"));
        assert!(!url.contains("/enable/backup"));
    }

    #[test]
    fn parse_ignores_body_nonce_on_approved() {
        let HandoffPollOutcome::Approved(binding) =
            parse_poll_body(&approved_body(), crate::test_support::PORTAL_BASE).unwrap()
        else {
            panic!("expected approved");
        };
        assert_eq!(binding.broker_token, "broker-token-secret");
        assert_eq!(binding.bucket, "bucket");
    }

    #[test]
    fn needs_subscription_requires_full_binding_then_discards_it() {
        assert!(matches!(
            parse_poll_body(&needs_subscription_body(), crate::test_support::PORTAL_BASE),
            Ok(HandoffPollOutcome::NeedsSubscription)
        ));
    }

    #[test]
    fn needs_subscription_missing_binding_field_is_rejected() {
        let mut value = serde_json::from_slice::<Value>(&needs_subscription_body()).unwrap();
        value.as_object_mut().unwrap().remove("bucket");
        let body = serde_json::to_vec(&value).unwrap();
        assert!(parse_poll_body(&body, crate::test_support::PORTAL_BASE).is_err());
    }

    #[test]
    fn refused_poll_outcomes_require_the_shared_exact_envelope() {
        for (reason_code, expected) in [
            ("no_hosted_backup", RefusedReason::NoHostedBackup),
            ("hosted_backup_expired", RefusedReason::HostedBackupExpired),
        ] {
            let body = serde_json::to_vec(&json!({
                "status": "refused",
                "reason_code": reason_code,
            }))
            .unwrap();
            assert!(matches!(
                parse_poll_body(&body, crate::test_support::PORTAL_BASE),
                Ok(HandoffPollOutcome::Refused(reason)) if reason == expected
            ));
        }

        for body in [
            json!({"status": "refused"}),
            json!({"status": "refused", "reason_code": "unknown"}),
            json!({"reason_code": "no_hosted_backup"}),
            json!({"status": "refused", "reason_code": "no_hosted_backup", "extra": true}),
        ] {
            assert!(
                parse_poll_body(
                    &serde_json::to_vec(&body).unwrap(),
                    crate::test_support::PORTAL_BASE
                )
                .is_err()
            );
        }
    }

    #[test]
    fn approved_blank_required_field_is_rejected() {
        let body = serde_json::to_vec(&json!({
            "status": "approved",
            "broker_endpoint": crate::test_support::PORTAL_BASE,
            "account_id": "account",
            "instance_id": "instance",
            "bucket": " ",
            "prefix": "owner/prefix",
            "broker_token": "broker-token-secret"
        }))
        .unwrap();
        assert!(parse_poll_body(&body, crate::test_support::PORTAL_BASE).is_err());
    }
}
