// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Small, non-secret service posture projection for the owner UI.

use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::window_catalog::{ENGLISH, text};
use solstone_core_journal_io::{JsonWriteOptions, read_regular_file_capped, write_json};

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
    #[serde(default, with = "solstone_core_time::serde::timestamp::option")]
    pub next_attempt_at: Option<Timestamp>,
    #[serde(with = "solstone_core_time::serde::timestamp")]
    pub observed_at: Timestamp,
}

pub fn read_mcp_owner_state(journal_root: &Path) -> Option<McpOwnerState> {
    let state: McpOwnerState =
        serde_json::from_slice(&std::fs::read(journal_root.join(STATE_PATH)).ok()?).ok()?;
    (state.schema == 1).then_some(state)
}

/// Keep display metadata beside the strict, older owner-state format.
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OwnerDetail {
    BootstrapFailed,
    Retrying,
    Offline,
    ServicesUnavailable,
    CertificateRateLimited,
    SubscriptionAddressKept,
    Subscription,
    NotAcceptedAddressKept,
    NotAccepted,
    UpdateRequired,
    AccountChanged,
    AddressNotReady,
    AddressRefused,
}

impl OwnerDetail {
    fn render(self, locale: &str) -> String {
        let id = match self {
            Self::BootstrapFailed => "journal.window.agents.service.bootstrap.failed",
            Self::Retrying => "journal.window.agents.service.retrying",
            Self::Offline => "journal.window.agents.service.offline",
            Self::ServicesUnavailable => "journal.window.agents.service.services.unavailable",
            Self::CertificateRateLimited => "journal.window.agents.service.certificate.wait",
            Self::SubscriptionAddressKept => {
                "journal.window.agents.service.subscription.address.kept"
            }
            Self::Subscription => "journal.window.agents.service.subscription",
            Self::NotAcceptedAddressKept => {
                "journal.window.agents.service.not.accepted.address.kept"
            }
            Self::NotAccepted => "journal.window.agents.service.not.accepted",
            Self::UpdateRequired => "journal.window.agents.service.update.required",
            Self::AccountChanged => "journal.window.agents.service.account.changed",
            Self::AddressNotReady => "journal.window.agents.service.address.not.ready",
            Self::AddressRefused => "journal.window.agents.service.address.refused",
        };
        text(locale, id)
    }
}

const DISPLAY_PATH: &str = "mcp-endpoint/owner-state.display.json";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DisplayDetail {
    state_sha256: String,
    detail: OwnerDetail,
}

fn state_digest(state: &McpOwnerState) -> Option<String> {
    let bytes = serde_json::to_vec(state).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

fn localized_detail(root: &Path, state: &McpOwnerState, locale: &str) -> Option<String> {
    let english = state.detail.as_deref()?;
    let bytes = read_regular_file_capped(root.join(DISPLAY_PATH), 512).ok()?;
    let display: DisplayDetail = serde_json::from_slice(&bytes).ok()?;
    (display.state_sha256 == state_digest(state)? && display.detail.render(ENGLISH) == english)
        .then(|| display.detail.render(locale))
}

pub(crate) fn read_owner_state_for_display(root: &Path, locale: &str) -> Option<McpOwnerState> {
    let mut state = read_mcp_owner_state(root)?;
    if locale != ENGLISH
        && let Some(detail) = localized_detail(root, &state, locale)
    {
        state.detail = Some(detail);
    }
    Some(state)
}

fn save(journal_root: &Path, state: McpOwnerState, detail: Option<OwnerDetail>) {
    let options = JsonWriteOptions {
        mode: Some(0o600),
        ..JsonWriteOptions::default()
    };
    if write_json(journal_root.join(STATE_PATH), &state, options).is_ok()
        && let Some(detail) = detail
        && let Some(state_sha256) = state_digest(&state)
    {
        let _ = write_json(
            journal_root.join(DISPLAY_PATH),
            &DisplayDetail {
                state_sha256,
                detail,
            },
            options,
        );
    }
}

pub(crate) fn write_mcp_owner_state(
    journal_root: &Path,
    status: &str,
    address: Option<&str>,
    legs: (&str, &str, &str),
    detail: Option<OwnerDetail>,
) {
    let state = McpOwnerState {
        schema: 1,
        status: status.to_owned(),
        address: address.map(str::to_owned),
        address_leg: legs.0.to_owned(),
        certificate_leg: legs.1.to_owned(),
        relay_leg: legs.2.to_owned(),
        detail: detail.map(|message| message.render(ENGLISH)),
        next_attempt_at: None,
        observed_at: Timestamp::now(),
    };
    save(journal_root, state, detail);
}

pub(crate) fn write_mcp_rate_limit_state(
    journal_root: &Path,
    address: &str,
    next_attempt_at: Timestamp,
) {
    let detail = OwnerDetail::CertificateRateLimited;
    let state = McpOwnerState {
        schema: 1,
        status: "turning_on".to_owned(),
        address: Some(address.to_owned()),
        address_leg: "done".to_owned(),
        certificate_leg: "not_this_week".to_owned(),
        relay_leg: "waiting".to_owned(),
        detail: Some(detail.render(ENGLISH)),
        next_attempt_at: Some(next_attempt_at),
        observed_at: Timestamp::now(),
    };
    save(journal_root, state, Some(detail));
}

pub(crate) fn write_mcp_hold_state(
    journal_root: &Path,
    hold: RegistrationHold,
    address: Option<&str>,
    legs: (&str, &str, &str),
    next_attempt_at: Timestamp,
) {
    let (status, detail) = match (hold, address.is_some()) {
        (RegistrationHold::NeedsSubscription, true) => {
            ("needs_subscription", OwnerDetail::SubscriptionAddressKept)
        }
        (RegistrationHold::NeedsSubscription, false) => {
            ("needs_subscription", OwnerDetail::Subscription)
        }
        (RegistrationHold::NotAccepted, true) => {
            ("not_accepted", OwnerDetail::NotAcceptedAddressKept)
        }
        (RegistrationHold::NotAccepted, false) => ("not_accepted", OwnerDetail::NotAccepted),
        (RegistrationHold::UpdateRequired, _) => ("update_required", OwnerDetail::UpdateRequired),
        (RegistrationHold::AccountChanged, _) => ("account_changed", OwnerDetail::AccountChanged),
        (RegistrationHold::AddressNotReady, _) => {
            ("address_not_ready", OwnerDetail::AddressNotReady)
        }
        (RegistrationHold::AddressRefused, _) => ("address_refused", OwnerDetail::AddressRefused),
    };
    let state = McpOwnerState {
        schema: 1,
        status: status.to_owned(),
        address: address.map(str::to_owned),
        address_leg: legs.0.to_owned(),
        certificate_leg: legs.1.to_owned(),
        relay_leg: legs.2.to_owned(),
        detail: Some(detail.render(ENGLISH)),
        next_attempt_at: Some(next_attempt_at),
        observed_at: Timestamp::now(),
    };
    save(journal_root, state, Some(detail));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn an_older_or_unrelated_state_keeps_its_own_detail() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("mcp-endpoint")).unwrap();
        write_mcp_owner_state(
            root,
            "offline",
            None,
            ("done", "done", "waiting"),
            Some(OwnerDetail::Offline),
        );
        let mut state = read_mcp_owner_state(root).unwrap();
        assert_eq!(localized_detail(root, &state, ENGLISH), state.detail);

        state.address = Some("other.solstone.me".into());
        assert!(localized_detail(root, &state, ENGLISH).is_none());
        state.address = None;
        state.detail = Some("existing journal text".into());
        assert!(localized_detail(root, &state, ENGLISH).is_none());

        std::fs::remove_file(root.join(DISPLAY_PATH)).unwrap();
        let old = read_mcp_owner_state(root).unwrap();
        assert!(localized_detail(root, &old, ENGLISH).is_none());
        assert_eq!(read_owner_state_for_display(root, ENGLISH), Some(old));
    }

    #[test]
    fn a_not_accepted_state_says_what_the_owner_can_do_and_never_blames_the_network() {
        let dir = tempdir().expect("tempdir");
        let journal_root = dir.path();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).expect("mkdir");
        let next_attempt = Timestamp::now() + jiff::SignedDuration::from_secs(300);
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
        let next_attempt = Timestamp::now() + jiff::SignedDuration::from_secs(300);

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
