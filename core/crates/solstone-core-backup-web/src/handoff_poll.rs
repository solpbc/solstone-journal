// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use solstone_core_backup::HostedBinding;
use solstone_core_backup_runtime::hosted_runtime::HttpError;
use solstone_core_backup_runtime::{HttpRequest, HttpResponse};

use crate::operation::{self, SharedOperationSlot};
use crate::validation;
use crate::{BackupWebDeps, persist_and_consume_hosted};

pub const HANDOFF_POLL_TIMEOUT: Duration = Duration::from_secs(15);
pub const HANDOFF_POLL_INTERVAL: Duration = Duration::from_secs(1);
pub const HANDOFF_WATCHDOG_TICK: Duration = Duration::from_millis(100);

enum HandoffPollOutcome {
    Approved(HostedBinding),
    NeedsSubscription,
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
    if validation::require_configured_portal_base(&deps.portal_base).is_err() {
        operation::finish(
            &deps.operations,
            generation,
            "error",
            Some("failed".into()),
            None,
        );
        return;
    }
    let Some(lease) = PollLease::try_acquire(&deps.handoff_poll_lease) else {
        operation::finish(
            &deps.operations,
            generation,
            "error",
            Some("failed".into()),
            None,
        );
        return;
    };
    let poll_deps = deps.clone();
    let poll_nonce = nonce;
    thread::spawn(move || {
        let _lease = lease;
        let panicked = panic::catch_unwind(AssertUnwindSafe(|| {
            poll_loop(&poll_deps, &poll_nonce, generation);
        }));
        if panicked.is_err() {
            operation::finish(
                &poll_deps.operations,
                generation,
                "error",
                Some("failed".into()),
                None,
            );
        }
    });
    thread::spawn(move || watchdog_loop(&deps.operations, generation));
}

fn poll_loop(deps: &BackupWebDeps, nonce: &str, generation: u64) {
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
        match deps.http.execute(&request) {
            Err(HttpError::Timeout) => {
                if !sleep_retry(deps, generation) {
                    return;
                }
            }
            Err(HttpError::Unreachable) => {
                finish_error(deps, generation, "unreachable");
                return;
            }
            Err(HttpError::Other) => {
                finish_error(deps, generation, "failed");
                return;
            }
            Ok(HttpResponse { status: 204, .. }) => {
                if !sleep_retry(deps, generation) {
                    return;
                }
            }
            Ok(HttpResponse { status: 410, .. }) => {
                operation::mark_expired(&deps.operations, generation);
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
                Err(()) => {
                    finish_error(deps, generation, "failed");
                    return;
                }
            },
            Ok(_) => {
                finish_error(deps, generation, "failed");
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
    let Some(current) = guard.as_ref() else {
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

fn finish_error(deps: &BackupWebDeps, generation: u64, reason: &str) {
    operation::finish(
        &deps.operations,
        generation,
        "error",
        Some(reason.to_owned()),
        None,
    );
}

fn apply_approved(deps: &BackupWebDeps, nonce: &str, generation: u64, binding: HostedBinding) {
    match live_wait(&deps.operations, generation) {
        LiveWait::Gone => return,
        LiveWait::Expired => {
            operation::mark_expired(&deps.operations, generation);
            return;
        }
        LiveWait::Live { .. } => {}
    }
    if let Ok(matched) = operation::match_handoff(&deps.operations, nonce) {
        let _ = persist_and_consume_hosted(
            deps,
            generation,
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
    if operation::match_handoff(&deps.operations, nonce).is_ok() {
        operation::mark_needs_subscription(&deps.operations, generation);
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
