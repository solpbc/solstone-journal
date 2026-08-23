// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{self, AssertUnwindSafe};
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value};
use solstone_core_backup::HostedBinding;
use solstone_core_backup_runtime::hosted_runtime::HttpError;
use solstone_core_backup_runtime::{HttpRequest, HttpResponse};

use crate::operation::{self, SharedOperationSlot};
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

pub(crate) fn spawn(deps: BackupWebDeps, nonce: String, generation: u64) {
    let poll_deps = deps.clone();
    let poll_nonce = nonce;
    thread::spawn(move || {
        let panicked = panic::catch_unwind(AssertUnwindSafe(|| {
            poll_loop(&poll_deps, &poll_nonce, generation);
        }));
        if panicked.is_err() {
            operation::finish(
                &poll_deps.operations,
                generation,
                "error",
                Some("failed".into()),
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
            }) => match parse_poll_body(&body) {
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

fn parse_poll_body(body: &[u8]) -> Result<HandoffPollOutcome, ()> {
    let value = serde_json::from_slice::<Value>(body).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    match object.get("status").and_then(Value::as_str) {
        Some("approved") => Ok(HandoffPollOutcome::Approved(binding_from_object(object)?)),
        Some("needs_subscription") => {
            let url = object
                .get("subscribe_url")
                .and_then(Value::as_str)
                .ok_or(())?;
            if !url.starts_with("https://") {
                return Err(());
            }
            Ok(HandoffPollOutcome::NeedsSubscription)
        }
        _ => Err(()),
    }
}

fn binding_from_object(object: &Map<String, Value>) -> Result<HostedBinding, ()> {
    let field = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or(())
    };
    Ok(HostedBinding {
        broker_endpoint: field("broker_endpoint")?,
        account_id: field("account_id")?,
        instance_id: field("instance_id")?,
        bucket: field("bucket")?,
        prefix: field("prefix")?,
        broker_token: field("broker_token")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn approved_body() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "status": "approved",
            "nonce": "SHOULD-BE-IGNORED",
            "broker_endpoint": "https://broker.example",
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
        let HandoffPollOutcome::Approved(binding) = parse_poll_body(&approved_body()).unwrap()
        else {
            panic!("expected approved");
        };
        assert_eq!(binding.broker_token, "broker-token-secret");
        assert_eq!(binding.bucket, "bucket");
    }

    #[test]
    fn needs_subscription_cannot_carry_broker_token() {
        let body = serde_json::to_vec(&json!({
            "status": "needs_subscription",
            "subscribe_url": "https://services.solstone.app/services/backup",
            "broker_token": "broker-token-secret",
            "bucket": "bucket",
            "prefix": "owner/prefix"
        }))
        .unwrap();
        assert!(matches!(
            parse_poll_body(&body),
            Ok(HandoffPollOutcome::NeedsSubscription)
        ));
    }

    #[test]
    fn approved_blank_required_field_is_rejected() {
        let body = serde_json::to_vec(&json!({
            "status": "approved",
            "broker_endpoint": "https://broker.example",
            "account_id": "account",
            "instance_id": "instance",
            "bucket": " ",
            "prefix": "owner/prefix",
            "broker_token": "broker-token-secret"
        }))
        .unwrap();
        assert!(parse_poll_body(&body).is_err());
    }
}
