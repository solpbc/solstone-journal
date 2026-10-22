// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::contract::GateDecision;
use crate::{ExecutionContext, PreparedTalent, StageError};

pub fn gate(
    prepared: &PreparedTalent,
    _context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    // Callable hosts do not project request.env into the host environment.
    // The prepared request carries the same bound stream in every host.
    Ok(gate_stream(
        prepared
            .config
            .get("stream")
            .and_then(serde_json::Value::as_str),
    ))
}

fn gate_stream(stream: Option<&str>) -> GateDecision {
    if stream == Some("import.document") {
        GateDecision::Proceed
    } else {
        GateDecision::Skip("not a document import segment".to_owned())
    }
}
