// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Prove the bound stream is independent of process-local SOL_STREAM.

use serde_json::Map;
use solstone_core_talent_runtime::contract::GateDecision;
use solstone_core_talent_runtime::documents::gate;
use solstone_core_talent_runtime::{ExecutionContext, PreparedTalent};
use std::env;
use std::process::Command;

#[test]
fn criterion_11_uses_bound_stream_in_every_host_environment() {
    for bound in [None, Some("different.stream"), Some("import.document")] {
        for ambient in [None, Some("different.stream"), Some("import.document")] {
            let mut command = Command::new(env::current_exe().unwrap());
            command.args([
                "--exact",
                "criterion_11_child_uses_the_production_gate",
                "--nocapture",
            ]);
            command.env_remove("SOL_STREAM");
            if let Some(stream) = ambient {
                command.env("SOL_STREAM", stream);
            }
            command.env("DOCUMENTS_GATE_BOUND", bound.unwrap_or(""));
            command.env(
                "DOCUMENTS_GATE_EXPECTATION",
                if bound == Some("import.document") {
                    "proceed"
                } else {
                    "skip"
                },
            );
            assert!(command.status().unwrap().success());
        }
    }
}

#[test]
fn criterion_11_child_uses_the_production_gate() {
    let Ok(expectation) = env::var("DOCUMENTS_GATE_EXPECTATION") else {
        return;
    };
    let mut prepared = PreparedTalent::new(
        "documents".to_owned(),
        Map::from_iter([(
            "env".to_owned(),
            serde_json::json!({
                "SOL_STREAM":"import.document"
            }),
        )]),
    );
    let bound = env::var("DOCUMENTS_GATE_BOUND").unwrap();
    if !bound.is_empty() {
        prepared
            .config
            .insert("stream".into(), serde_json::Value::from(bound));
    }
    let context = ExecutionContext {
        journal: Default::default(),
    };
    let before = prepared.config.clone();
    // Native callable hosts do not mutate the host process environment.
    // Request.env is transport metadata; the bound stream drives the gate.
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
