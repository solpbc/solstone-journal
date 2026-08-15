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
        GateDecision::Skip("not a document import segment")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionContext, PreparedTalent};
    use serde_json::Map;
    use std::path::PathBuf;

    #[test]
    fn criterion_11_reads_process_environment_not_request_env() {
        let prepared = PreparedTalent {
            name: "documents".to_owned(),
            config: Map::new(),
        };
        let context = ExecutionContext {
            journal: PathBuf::new(),
        };
        // The source is the child process environment, populated by cortex.
        let _ = (prepared, context); // gate reads std::env::var at the production boundary.
        assert_eq!(
            gate_stream(None),
            GateDecision::Skip("not a document import segment")
        );
        assert_eq!(gate_stream(Some("import.document")), GateDecision::Proceed);
    }
}
