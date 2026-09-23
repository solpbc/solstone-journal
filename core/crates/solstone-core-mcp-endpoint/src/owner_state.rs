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
            "solstone.me needs an active subscription. your address is kept, and your journal keeps trying on its own.",
        ),
        (RegistrationHold::NeedsSubscription, false) => (
            "needs_subscription",
            "solstone.me needs an active subscription. your journal keeps trying on its own.",
        ),
        (RegistrationHold::NotAccepted, true) => (
            "not_accepted",
            "solstone.me didn't accept this journal's request. if you haven't allowed solstone.me for this journal yet, turn solstone.me off and back on to do that. your address is kept, and your journal keeps trying on its own.",
        ),
        (RegistrationHold::NotAccepted, false) => (
            "not_accepted",
            "solstone.me didn't accept this journal's request. if you haven't allowed solstone.me for this journal yet, turn solstone.me off and back on to do that. your journal keeps trying on its own.",
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
