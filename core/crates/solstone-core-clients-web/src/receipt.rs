// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use solstone_core_retention::receipt::NotRemoved;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Issue {
    pub what: String,
    pub plain_reason: String,
}

#[derive(Serialize)]
pub(crate) struct ReceiptTarget {
    pub journal: String,
    pub stream: String,
}

#[derive(Serialize)]
pub(crate) struct Removed {
    pub days: u64,
    pub index_chunks: u64,
    pub mixed_segments: u64,
    pub originals: u64,
    pub segments: u64,
    pub stream_identity: u64,
    pub tombstones: u64,
}

#[derive(Serialize)]
pub(crate) struct Receipt {
    pub target: ReceiptTarget,
    pub removed: Removed,
    pub not_confirmed: Vec<Issue>,
    pub not_removed: Vec<Issue>,
    pub backup_hosted: &'static str,
}

pub(crate) fn owner_issue(entry: &NotRemoved) -> Issue {
    if entry.staged.is_some()
        || entry
            .reason
            .contains("previous removal of this segment did not finish")
    {
        return Issue {
            what: entry.staged.clone().unwrap_or_else(|| entry.entry.clone()),
            plain_reason: "a previous removal of this segment did not finish; \
                           it needs looking at before this can be retried"
                .to_owned(),
        };
    }
    if entry.reason.contains("already been removed") {
        return Issue {
            what: entry.entry.clone(),
            plain_reason: "this segment has already been removed".to_owned(),
        };
    }
    if entry
        .reason
        .starts_with("another process is working on this segment")
    {
        return Issue {
            what: entry.entry.clone(),
            plain_reason: "another process is working on this segment".to_owned(),
        };
    }
    if entry.reason.contains("no such segment in your journal") {
        return Issue {
            what: entry.entry.clone(),
            plain_reason: "there is no such segment in your journal".to_owned(),
        };
    }
    Issue {
        what: entry.entry.clone(),
        plain_reason: entry.reason.clone(),
    }
}
