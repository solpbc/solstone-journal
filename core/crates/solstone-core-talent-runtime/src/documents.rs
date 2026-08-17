// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::contract::GateDecision;
use crate::{ExecutionContext, PreparedTalent, StageError};

pub fn gate(
    _prepared: &PreparedTalent,
    _context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    // Cortex projects request.env into this worker's process environment; the
    // hook deliberately reads that process environment, like the Python hook.
    Ok(gate_stream(std::env::var("SOL_STREAM").ok().as_deref()))
}

fn gate_stream(stream: Option<&str>) -> GateDecision {
    if stream == Some("import.document") {
        GateDecision::Proceed
    } else {
        GateDecision::Skip("not a document import segment".to_owned())
    }
}
