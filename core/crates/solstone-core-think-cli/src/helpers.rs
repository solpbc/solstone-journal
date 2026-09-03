// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
pub(crate) struct ThinkStatus(Arc<Mutex<Map<String, Value>>>);

impl ThinkStatus {
    pub(crate) fn update(&self, fields: Map<String, Value>) {
        // Source-derived, not measured: thinking.py:770-773 updates the
        // shared status mapping in-place under one lock.
        self.0.lock().expect("think status lock").extend(fields);
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Map<String, Value> {
        self.0.lock().expect("think status lock").clone()
    }
}

/// Send a best-effort `think` tract event, matching `thinking.py:895-898`.
pub(crate) fn emit(journal: &Path, now_ms: i64, event: &str, fields: Map<String, Value>) -> bool {
    let envelope = CallosumEnvelope {
        tract: "think".to_owned(),
        event: event.to_owned(),
        ts: Some(now_ms),
        extra: fields,
    };
    let Ok(mut line) = serde_json::to_string(&envelope) else {
        return false;
    };
    line.push('\n');
    CallosumOneShotSender::new(journal.join("health/callosum.sock"), SOCKET_TIMEOUT)
        .send_line(&line)
        .is_ok()
}
