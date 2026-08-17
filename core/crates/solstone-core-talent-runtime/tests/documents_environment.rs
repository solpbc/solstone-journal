// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-local SOL_STREAM cannot share the lib harness.

use serde_json::Map;
use solstone_core_talent_runtime::contract::GateDecision;
use solstone_core_talent_runtime::documents::gate;
use solstone_core_talent_runtime::{ExecutionContext, PreparedTalent};
use std::env;
use std::process::Command;

#[test]
fn criterion_11_reads_process_environment_not_request_env() {
    for (stream, expected) in [
        (Some("different.stream"), "skip"),
        (Some("import.document"), "proceed"),
        (None, "skip"),
    ] {
        let mut command = Command::new(env::current_exe().unwrap());
        command.args([
            "--exact",
            "criterion_11_child_uses_the_production_gate",
            "--nocapture",
        ]);
        command.env_remove("SOL_STREAM");
        if let Some(s) = stream {
            command.env("SOL_STREAM", s);
        }
        command.env("DOCUMENTS_GATE_EXPECTATION", expected);
        assert!(command.status().unwrap().success());
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
