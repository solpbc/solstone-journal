// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Persist device sync history from observe/observed and observe/transferred.

use std::path::PathBuf;

use serde_json::{Map, Value};
use solstone_core_callosum::CallosumSocketConnection;
use solstone_core_observer::{SyncEventKind, SyncPersistResult, persist_sync, system_now_ms};

pub(crate) async fn subscribe_sync_persist(journal_root: PathBuf) {
    let mut connection =
        CallosumSocketConnection::new(journal_root.join("health/callosum.sock"), Map::new());
    connection.start();
    while let Some(envelope) = connection.next_message().await {
        let kind = match (envelope.tract.as_str(), envelope.event.as_str()) {
            ("observe", "observed") => SyncEventKind::Observed,
            ("observe", "transferred") => SyncEventKind::Transferred,
            _ => continue,
        };
        let device_name = extra_str(&envelope.extra, "observer");
        let day = extra_str(&envelope.extra, "day");
        let segment = extra_str(&envelope.extra, "segment");
        match persist_sync(
            &journal_root,
            device_name,
            day,
            segment,
            kind,
            system_now_ms(),
        ) {
            SyncPersistResult::Skipped | SyncPersistResult::Applied => {}
            SyncPersistResult::TornHistory => {
                log::debug!("ignored torn device sync history for {device_name}/{day}")
            }
            SyncPersistResult::HistoryWriteFailed => {
                log::debug!("failed to append device sync history for {device_name}/{day}")
            }
            SyncPersistResult::HistoryWrittenStatsFailed => {
                log::debug!("device sync history wrote but stats save failed for {device_name}")
            }
        }
    }
}

fn extra_str<'a>(extra: &'a Map<String, Value>, key: &str) -> &'a str {
    extra.get(key).and_then(Value::as_str).unwrap_or("")
}
