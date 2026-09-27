// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Small, non-secret service posture projection for the owner UI.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{JsonWriteOptions, write_json};

use crate::bridge_carrier::RegistrationHold;

const STATE_PATH: &str = "mcp-endpoint/owner-state.json";

const fn is_zero_u64(val: &u64) -> bool {
    *val == 0
}

/// Cumulative counters for diagnostic door cuts and refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoorCutCounters {
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub tool_withheld: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub tool_cut: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub token_withheld: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub token_cut: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub register_withheld: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub register_cut: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub authorize_withheld: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub authorize_cut: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub other_withheld: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub other_cut: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub cloudflare_refused: u64,
}

impl DoorCutCounters {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tool_withheld == 0
            && self.tool_cut == 0
            && self.token_withheld == 0
            && self.token_cut == 0
            && self.register_withheld == 0
            && self.register_cut == 0
            && self.authorize_withheld == 0
            && self.authorize_cut == 0
            && self.other_withheld == 0
            && self.other_cut == 0
            && self.cloudflare_refused == 0
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_refusal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_reason: Option<String>,
    #[serde(default, skip_serializing_if = "DoorCutCounters::is_empty")]
    pub cuts: DoorCutCounters,
}

pub fn read_mcp_owner_state(journal_root: &Path) -> Option<McpOwnerState> {
    let state: McpOwnerState =
        serde_json::from_slice(&std::fs::read(journal_root.join(STATE_PATH)).ok()?).ok()?;
    (state.schema == 1).then_some(state)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticCut {
    ToolWithheld,
    ToolCut,
    TokenWithheld,
    TokenCut,
    RegisterWithheld,
    RegisterCut,
    AuthorizeWithheld,
    AuthorizeCut,
    OtherWithheld,
    OtherCut,
    CloudflareRefused,
}

pub(crate) fn record_diagnostic_cut(journal_root: &Path, cut: DiagnosticCut) {
    match cut {
        DiagnosticCut::ToolWithheld => {
            log::warn!("mcp door withheld a tool call before any response byte");
        }
        DiagnosticCut::ToolCut => {
            log::warn!("mcp door cut a tool call while writing");
        }
        DiagnosticCut::TokenWithheld => {
            log::warn!("mcp door withheld a token response before any response byte");
        }
        DiagnosticCut::TokenCut => {
            log::warn!("mcp door cut a token response while writing");
        }
        DiagnosticCut::RegisterWithheld => {
            log::warn!("mcp door withheld a register response before any response byte");
        }
        DiagnosticCut::RegisterCut => {
            log::warn!("mcp door cut a register response while writing");
        }
        DiagnosticCut::AuthorizeWithheld => {
            log::warn!("mcp door withheld an authorize response before any response byte");
        }
        DiagnosticCut::AuthorizeCut => {
            log::warn!("mcp door cut an authorize response while writing");
        }
        DiagnosticCut::OtherWithheld => {
            log::warn!("mcp door withheld a response before any response byte");
        }
        DiagnosticCut::OtherCut => {
            log::warn!("mcp door cut a response while writing");
        }
        DiagnosticCut::CloudflareRefused => {
            log::warn!("mcp door refused a cloudflare proxied preface source");
        }
    }

    let existing = read_mcp_owner_state(journal_root);
    let mut state = existing.unwrap_or_else(|| McpOwnerState {
        schema: 1,
        status: "turning_on".to_owned(),
        address: None,
        address_leg: "waiting".to_owned(),
        certificate_leg: "waiting".to_owned(),
        relay_leg: "waiting".to_owned(),
        detail: None,
        next_attempt_at: None,
        observed_at: Utc::now(),
        open_refusal: None,
        closed_reason: None,
        cuts: DoorCutCounters::default(),
    });

    match cut {
        DiagnosticCut::ToolWithheld => {
            state.cuts.tool_withheld = state.cuts.tool_withheld.saturating_add(1)
        }
        DiagnosticCut::ToolCut => state.cuts.tool_cut = state.cuts.tool_cut.saturating_add(1),
        DiagnosticCut::TokenWithheld => {
            state.cuts.token_withheld = state.cuts.token_withheld.saturating_add(1)
        }
        DiagnosticCut::TokenCut => state.cuts.token_cut = state.cuts.token_cut.saturating_add(1),
        DiagnosticCut::RegisterWithheld => {
            state.cuts.register_withheld = state.cuts.register_withheld.saturating_add(1)
        }
        DiagnosticCut::RegisterCut => {
            state.cuts.register_cut = state.cuts.register_cut.saturating_add(1)
        }
        DiagnosticCut::AuthorizeWithheld => {
            state.cuts.authorize_withheld = state.cuts.authorize_withheld.saturating_add(1)
        }
        DiagnosticCut::AuthorizeCut => {
            state.cuts.authorize_cut = state.cuts.authorize_cut.saturating_add(1)
        }
        DiagnosticCut::OtherWithheld => {
            state.cuts.other_withheld = state.cuts.other_withheld.saturating_add(1)
        }
        DiagnosticCut::OtherCut => state.cuts.other_cut = state.cuts.other_cut.saturating_add(1),
        DiagnosticCut::CloudflareRefused => {
            state.cuts.cloudflare_refused = state.cuts.cloudflare_refused.saturating_add(1)
        }
    }
    state.observed_at = Utc::now();

    let _ = write_json(
        journal_root.join(STATE_PATH),
        &state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    );
}

pub(crate) fn publish_closed_status(
    journal_root: &Path,
    reason: Option<&str>,
    open_refusal: Option<&str>,
) {
    let existing = read_mcp_owner_state(journal_root);
    let (existing_refusal, cuts, address, legs) = existing
        .as_ref()
        .map(|s| {
            (
                s.open_refusal.clone(),
                s.cuts,
                s.address.clone(),
                (
                    s.address_leg.clone(),
                    s.certificate_leg.clone(),
                    s.relay_leg.clone(),
                ),
            )
        })
        .unwrap_or_else(|| {
            (
                None,
                DoorCutCounters::default(),
                None,
                (
                    "waiting".to_owned(),
                    "waiting".to_owned(),
                    "waiting".to_owned(),
                ),
            )
        });

    let final_refusal = open_refusal.map(str::to_owned).or(existing_refusal);

    let state = McpOwnerState {
        schema: 1,
        status: "closed".to_owned(),
        address,
        address_leg: legs.0,
        certificate_leg: legs.1,
        relay_leg: legs.2,
        detail: None,
        next_attempt_at: None,
        observed_at: Utc::now(),
        open_refusal: final_refusal,
        closed_reason: reason.map(str::to_owned),
        cuts,
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

pub(crate) fn write_mcp_owner_state(
    journal_root: &Path,
    status: &str,
    address: Option<&str>,
    legs: (&str, &str, &str),
    detail: Option<&str>,
) {
    let existing = read_mcp_owner_state(journal_root);
    let (open_refusal, cuts, closed_reason) = existing
        .as_ref()
        .map(|s| (s.open_refusal.clone(), s.cuts, s.closed_reason.clone()))
        .unwrap_or_default();

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
        open_refusal,
        closed_reason: if status == "closed" {
            closed_reason
        } else {
            None
        },
        cuts,
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
    let existing = read_mcp_owner_state(journal_root);
    let (open_refusal, cuts) = existing
        .as_ref()
        .map(|s| (s.open_refusal.clone(), s.cuts))
        .unwrap_or_default();

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
        open_refusal,
        closed_reason: None,
        cuts,
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
    let existing = read_mcp_owner_state(journal_root);
    let (open_refusal, cuts) = existing
        .as_ref()
        .map(|s| (s.open_refusal.clone(), s.cuts))
        .unwrap_or_default();

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
        open_refusal,
        closed_reason: None,
        cuts,
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

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    fn scratch_journal(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix(name)
            .tempdir_in(crate::test_scratch())
            .expect("scratch temp dir");
        let path = dir.path().to_path_buf();
        std::fs::create_dir_all(path.join("mcp-endpoint")).expect("mkdir");
        (dir, path)
    }

    #[test]
    fn old_owner_state_without_new_fields_parses_as_schema_one() {
        let (_dir, root) = scratch_journal("mcp-old-state-");
        let old_json = r#"{
            "schema": 1,
            "status": "on",
            "address": "test.solstone.me",
            "address_leg": "done",
            "certificate_leg": "done",
            "relay_leg": "done",
            "detail": null,
            "observed_at": "2026-09-26T12:00:00Z"
        }"#;
        std::fs::write(root.join(STATE_PATH), old_json).unwrap();
        let read = read_mcp_owner_state(&root).expect("parses old state");
        assert_eq!(read.schema, 1);
        assert_eq!(read.status, "on");
        assert!(read.open_refusal.is_none());
        assert!(read.closed_reason.is_none());
        assert!(read.cuts.is_empty());
    }

    #[test]
    fn publisher_with_no_epoch_and_reason_writes_closed_and_preserves_counters() {
        let (_dir, root) = scratch_journal("mcp-closed-publish-");
        record_diagnostic_cut(&root, DiagnosticCut::ToolWithheld);
        record_diagnostic_cut(&root, DiagnosticCut::CloudflareRefused);

        publish_closed_status(&root, Some("door_closed"), None);
        let read = read_mcp_owner_state(&root).expect("reads state");
        assert_eq!(read.status, "closed");
        assert_eq!(read.closed_reason.as_deref(), Some("door_closed"));
        assert_eq!(read.cuts.tool_withheld, 1);
        assert_eq!(read.cuts.cloudflare_refused, 1);
    }

    #[test]
    fn open_refusal_bind_failed_is_preserved_across_status_writes() {
        let (_dir, root) = scratch_journal("mcp-refusal-preserved-");
        publish_closed_status(&root, Some("bind_failed"), Some("bind_failed"));

        write_mcp_owner_state(
            &root,
            "offline",
            Some("addr.solstone.me"),
            ("done", "done", "waiting"),
            None,
        );
        let read = read_mcp_owner_state(&root).expect("reads state");
        assert_eq!(read.status, "offline");
        assert_eq!(read.open_refusal.as_deref(), Some("bind_failed"));
    }
}
