// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{path::Path, time::Duration};

use serde_json::{Value, json};
use solstone_core_callosum::{CallosumOneShotError, CallosumOneShotSender};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BusError {
    Unavailable,
}

fn send(root: &Path, value: Value) -> Result<(), BusError> {
    let line = format!(
        "{}\n",
        serde_json::to_string(&value).map_err(|_| BusError::Unavailable)?
    );
    CallosumOneShotSender::new(root.join("health/callosum.sock"), Duration::from_secs(2))
        .send_line(&line)
        .map_err(|error| match error {
            CallosumOneShotError::Unavailable => BusError::Unavailable,
        })
}

pub(crate) fn request_required(root: &Path, task_id: &str, cmd: &[String]) -> Result<(), BusError> {
    send(
        root,
        json!({
            "tract": "supervisor",
            "event": "request",
            "ref": task_id,
            "cmd": cmd,
            "queue_if_active_cmd_differs": true,
        }),
    )
}

/// Deliver a segment-ingest notification without changing ingest persistence on failure.
#[allow(dead_code)] // Phase C's segment route is the first caller.
pub(crate) fn emit_best_effort(root: &Path, value: Value) {
    let _ = send(root, value);
}
