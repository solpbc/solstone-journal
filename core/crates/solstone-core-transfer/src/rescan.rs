// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Best-effort synchronous indexer-rescan notification.

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};

/// Result of asking the supervisor to rescan newly landed content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RescanOutcome {
    /// No content landed, so no notification was necessary.
    NotNeeded,
    /// The message was accepted by the local socket.
    Queued,
    /// The local socket was unavailable; import remains successful.
    Unavailable,
}

impl RescanOutcome {
    /// Stable report label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotNeeded => "not-needed",
            Self::Queued => "queued",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Send the existing supervisor request shape over the one-shot client.
pub fn send_indexer_rescan(journal: &Path) -> RescanOutcome {
    let mut extra = Map::new();
    extra.insert("cmd".to_owned(), json!(["journal", "indexer", "--rescan"]));
    let envelope = CallosumEnvelope {
        tract: "supervisor".to_owned(),
        event: "request".to_owned(),
        ts: None,
        extra,
    };
    let Ok(mut line) = serde_json::to_string(&envelope) else {
        return RescanOutcome::Unavailable;
    };
    line.push('\n');
    let sender = CallosumOneShotSender::new(
        journal.join("health").join("callosum.sock"),
        Duration::from_secs(1),
    );
    if sender.send_line(&line).is_ok() {
        RescanOutcome::Queued
    } else {
        log::warn!("indexer rescan was not queued: Callosum socket unavailable");
        RescanOutcome::Unavailable
    }
}
