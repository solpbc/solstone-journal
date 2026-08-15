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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionContext, PreparedTalent};
    use serde_json::Map;
    use std::env;

    #[test]
    fn criterion_11_reads_process_environment_not_request_env() {
        for (stream, expected) in [("different.stream", "skip"), ("import.document", "proceed")] {
            let status = std::process::Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "documents::tests::criterion_11_child_uses_the_production_gate",
                    "--nocapture",
                ])
                .env("SOL_STREAM", stream)
                .env("DOCUMENTS_GATE_EXPECTATION", expected)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    #[test]
    fn criterion_11_child_uses_the_production_gate() {
        let Ok(expectation) = env::var("DOCUMENTS_GATE_EXPECTATION") else {
            return;
        };
        let prepared = PreparedTalent {
            name: "documents".to_owned(),
            config: Map::from_iter([(
                "env".to_owned(),
                serde_json::json!({
                    "SOL_STREAM":"import.document"
                }),
            )]),
        };
        let context = ExecutionContext {
            journal: Default::default(),
        };
        let before = prepared.config.clone();
        // Cortex applies request.env to the child process at process.rs:118-127,
        // while the request JSON carries the same map. Reading that map here
        // would always skip in production, so the production gate reads env.
        let decision = gate(&prepared, &context).unwrap();
        match expectation.as_str() {
            "skip" => assert_eq!(
                decision,
                GateDecision::Skip("not a document import segment".to_owned())
            ),
            "proceed" => assert_eq!(decision, GateDecision::Proceed),
            other => panic!("unknown child expectation: {other}"),
        }
        assert_eq!(prepared.config, before);
    }
}
