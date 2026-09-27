// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Small, non-secret service posture projection for the owner UI.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{JsonWriteOptions, write_json};

use crate::bridge_carrier::RegistrationHold;

const STATE_PATH: &str = "mcp-endpoint/owner-state.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOwnerState {
    pub schema: u32,
    pub status: String,
    pub address: Option<String>,
    pub address_leg: String,
    pub certificate_leg: String,
    pub relay_leg: String,
    pub detail: Option<String>,
    #[serde(default)]
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
}

pub fn read_mcp_owner_state(journal_root: &Path) -> Option<McpOwnerState> {
    let state: McpOwnerState =
        serde_json::from_slice(&std::fs::read(journal_root.join(STATE_PATH)).ok()?).ok()?;
    (state.schema == 1).then_some(state)
}

pub(crate) fn write_mcp_owner_state(
    journal_root: &Path,
    status: &str,
    address: Option<&str>,
    legs: (&str, &str, &str),
    detail: Option<&str>,
) {
    let state = McpOwnerState {
        schema: 1,
        status: status.to_owned(),
        address: address.map(str::to_owned),
        address_leg: legs.0.to_owned(),
        certificate_leg: legs.1.to_owned(),
        relay_leg: legs.2.to_owned(),
        detail: detail.map(str::to_owned),
        next_attempt_at: None,
        observed_at: Utc::now(),
    };
    let _ = write_json(
        journal_root.join(STATE_PATH),
        &state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    );
}

pub(crate) fn write_mcp_rate_limit_state(
    journal_root: &Path,
    address: &str,
    next_attempt_at: DateTime<Utc>,
) {
    let state = McpOwnerState {
        schema: 1,
        status: "turning_on".to_owned(),
        address: Some(address.to_owned()),
        address_leg: "done".to_owned(),
        certificate_leg: "not_this_week".to_owned(),
        relay_leg: "waiting".to_owned(),
        detail: Some("the certificate authority's current allowance is used up. your journal is in line and will try again on its own. no action needed; you'll see it here when it's ready.".to_owned()),
        next_attempt_at: Some(next_attempt_at),
        observed_at: Utc::now(),
    };
    let _ = write_json(
        journal_root.join(STATE_PATH),
        &state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    );
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpAccountPosture {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpAccountReplacedNotice {
    pub seen: String,
    pub dateless: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed: Option<String>,
}

pub(crate) fn account_status_detail(status: &str) -> Option<&'static str> {
    match status {
        "journal_update_required" => Some("this journal needs an update"),
        "acme_account_changed" => Some("account changed — confirm replacing"),
        "address_not_ready" => Some("address not ready yet"),
        "address_refused" => Some("the address request was refused"),
        "certificate_account_setup" => Some("setting up the address's certificate account"),
        "certificate_account_unknown" => {
            Some("the certificate authority no longer knows this account")
        }
        "certificate_account_deactivated" => Some("the certificate account is deactivated"),
        "certificate_account_refused" => Some("the certificate account request was refused"),
        "certificate_account_unreadable" => Some("the certificate account key cannot be read"),
        "certificate_account_missing" => Some("the certificate account key is missing"),
        "certificate_order_refused" => Some("the certificate authority refused this account"),
        _ => None,
    }
}

pub(crate) fn write_mcp_hold_state(
    journal_root: &Path,
    hold: RegistrationHold,
    address: Option<&str>,
    legs: (&str, &str, &str),
    next_attempt_at: DateTime<Utc>,
) {
    let (status, detail) = match (hold, address.is_some()) {
        (RegistrationHold::NeedsSubscription, true) => (
            "needs_subscription",
            "solstone.me isn't on yet. your address is kept, and your journal keeps trying on its own. finish turning it on in the services portal.",
        ),
        (RegistrationHold::NeedsSubscription, false) => (
            "needs_subscription",
            "solstone.me isn't on yet. your journal keeps trying on its own. finish turning it on in the services portal.",
        ),
        (RegistrationHold::NotAccepted, true) => (
            "not_accepted",
            "solstone.me didn't accept this journal's request. if you haven't allowed solstone.me for this journal yet, turn solstone.me off and back on to do that. your address is kept, and your journal keeps trying on its own.",
        ),
        (RegistrationHold::NotAccepted, false) => (
            "not_accepted",
            "solstone.me didn't accept this journal's request. if you haven't allowed solstone.me for this journal yet, turn solstone.me off and back on to do that. your journal keeps trying on its own.",
        ),
        (RegistrationHold::JournalUpdateRequired, _) => (
            "journal_update_required",
            account_status_detail("journal_update_required").unwrap(),
        ),
        (RegistrationHold::AcmeAccountChanged, _) => (
            "acme_account_changed",
            account_status_detail("acme_account_changed").unwrap(),
        ),
        (RegistrationHold::AddressNotReady, _) => (
            "address_not_ready",
            account_status_detail("address_not_ready").unwrap(),
        ),
        (RegistrationHold::AddressRefused, _) => (
            "address_refused",
            account_status_detail("address_refused").unwrap(),
        ),
    };
    let state = McpOwnerState {
        schema: 1,
        status: status.to_owned(),
        address: address.map(str::to_owned),
        address_leg: legs.0.to_owned(),
        certificate_leg: legs.1.to_owned(),
        relay_leg: legs.2.to_owned(),
        detail: Some(detail.to_owned()),
        next_attempt_at: Some(next_attempt_at),
        observed_at: Utc::now(),
    };
    let _ = write_json(
        journal_root.join(STATE_PATH),
        &state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    );
}

pub(crate) fn write_mcp_account_posture_state(
    journal_root: &Path,
    status: &str,
    address: Option<&str>,
    legs: (&str, &str, &str),
    next_attempt_at: Option<DateTime<Utc>>,
) {
    let detail = account_status_detail(status).unwrap_or(status);
    let state = McpOwnerState {
        schema: 1,
        status: status.to_owned(),
        address: address.map(str::to_owned),
        address_leg: legs.0.to_owned(),
        certificate_leg: legs.1.to_owned(),
        relay_leg: legs.2.to_owned(),
        detail: Some(detail.to_owned()),
        next_attempt_at,
        observed_at: Utc::now(),
    };
    let _ = write_json(
        journal_root.join(STATE_PATH),
        &state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    );
    let posture = McpAccountPosture {
        status: status.to_owned(),
        account_url: None,
        until: next_attempt_at,
    };
    write_mcp_account_posture(journal_root, &posture);
}

#[cfg(unix)]
pub(crate) fn read_mcp_account_posture(journal_root: &Path) -> Option<McpAccountPosture> {
    let root = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root).ok()?;
    let dir = crate::unix::open_tls_state_directory(&root).ok()?;
    let bytes = crate::unix::read_tls_account_posture_bytes(&dir).ok()??;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(not(unix))]
pub(crate) fn read_mcp_account_posture(_journal_root: &Path) -> Option<McpAccountPosture> {
    None
}

#[cfg(unix)]
pub(crate) fn write_mcp_account_posture(journal_root: &Path, posture: &McpAccountPosture) {
    if let Ok(root) = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root) {
        if let Ok(dir) = crate::unix::open_tls_state_directory(&root) {
            if let Ok(bytes) = serde_json::to_vec(posture) {
                let _ = crate::unix::persist_tls_account_posture_bytes(&dir, &bytes);
            }
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn write_mcp_account_posture(_journal_root: &Path, _posture: &McpAccountPosture) {}

#[cfg(unix)]
pub(crate) fn delete_mcp_account_posture(journal_root: &Path) {
    if let Ok(root) = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root) {
        if let Ok(dir) = crate::unix::open_tls_state_directory(&root) {
            let _ = crate::unix::delete_tls_account_posture(&dir);
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn delete_mcp_account_posture(_journal_root: &Path) {}

#[cfg(unix)]
pub(crate) fn read_mcp_account_replaced_state(
    journal_root: &Path,
) -> Option<McpAccountReplacedNotice> {
    let root = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root).ok()?;
    let dir = crate::unix::open_tls_state_directory(&root).ok()?;
    let bytes = crate::unix::read_tls_account_replaced_bytes(&dir).ok()??;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(not(unix))]
pub(crate) fn read_mcp_account_replaced_state(
    _journal_root: &Path,
) -> Option<McpAccountReplacedNotice> {
    None
}

#[cfg(unix)]
pub(crate) fn write_mcp_account_replaced_state(
    journal_root: &Path,
    notice: &McpAccountReplacedNotice,
) {
    if let Ok(root) = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root) {
        if let Ok(dir) = crate::unix::open_tls_state_directory(&root) {
            if let Ok(bytes) = serde_json::to_vec(notice) {
                let _ = crate::unix::persist_tls_account_replaced_bytes(&dir, &bytes);
            }
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn write_mcp_account_replaced_state(
    _journal_root: &Path,
    _notice: &McpAccountReplacedNotice,
) {
}

#[cfg(unix)]
pub(crate) fn delete_mcp_account_replaced_state(journal_root: &Path) {
    if let Ok(root) = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root) {
        if let Ok(dir) = crate::unix::open_tls_state_directory(&root) {
            let _ = crate::unix::delete_tls_account_replaced_bytes(&dir);
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn delete_mcp_account_replaced_state(_journal_root: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_not_accepted_state_says_what_the_owner_can_do_and_never_blames_the_network() {
        let dir = tempdir().expect("tempdir");
        let journal_root = dir.path();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).expect("mkdir");
        let next_attempt = Utc::now() + chrono::Duration::seconds(300);
        for address in [None, Some("aaaqeaye.solstone.me")] {
            write_mcp_hold_state(
                journal_root,
                RegistrationHold::NotAccepted,
                address,
                ("waiting", "waiting", "waiting"),
                next_attempt,
            );
            let state = read_mcp_owner_state(journal_root).expect("owner state");
            assert_eq!(state.status, "not_accepted");
            assert_eq!(state.address.as_deref(), address);
            assert_eq!(state.next_attempt_at, Some(next_attempt));
            let detail = state.detail.as_deref().unwrap_or_default();
            assert!(
                detail.contains("turn solstone.me off and back on"),
                "{detail}"
            );
            assert!(!detail.contains("could not reach"), "{detail}");
            assert!(!detail.contains("subscription"), "{detail}");
        }
    }

    #[test]
    fn write_and_read_mcp_needs_subscription_state() {
        let dir = tempdir().expect("tempdir");
        let journal_root = dir.path();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).expect("mkdir");
        let next_attempt = Utc::now() + chrono::Duration::seconds(300);

        // Without address (first-connect 402)
        write_mcp_hold_state(
            journal_root,
            crate::bridge_carrier::RegistrationHold::NeedsSubscription,
            None,
            ("waiting", "waiting", "waiting"),
            next_attempt,
        );
        let state = read_mcp_owner_state(journal_root).expect("owner state");
        assert_eq!(state.status, "needs_subscription");
        assert_eq!(state.address, None);
        assert_eq!(state.address_leg, "waiting");
        assert_eq!(state.certificate_leg, "waiting");
        assert_eq!(state.relay_leg, "waiting");
        assert_eq!(state.next_attempt_at, Some(next_attempt));
        let detail = state.detail.as_deref().unwrap_or_default();
        assert!(!detail.contains("this computer could not reach services.solstone.app; it will try again when the service restarts"));
        assert!(!detail.contains("couldn't reach services.solstone.app"));

        // With address (renewal/reconnect 402)
        write_mcp_hold_state(
            journal_root,
            crate::bridge_carrier::RegistrationHold::NeedsSubscription,
            Some("aaaqeaye.solstone.me"),
            ("done", "done", "waiting"),
            next_attempt,
        );
        let state = read_mcp_owner_state(journal_root).expect("owner state");
        assert_eq!(state.status, "needs_subscription");
        assert_eq!(state.address.as_deref(), Some("aaaqeaye.solstone.me"));
        assert_eq!(state.address_leg, "done");
        assert_eq!(state.certificate_leg, "done");
        assert_eq!(state.relay_leg, "waiting");
        assert_eq!(state.next_attempt_at, Some(next_attempt));
        let detail = state.detail.as_deref().unwrap_or_default();
        assert!(!detail.contains("this computer could not reach services.solstone.app; it will try again when the service restarts"));
        assert!(!detail.contains("couldn't reach services.solstone.app"));
    }
}
