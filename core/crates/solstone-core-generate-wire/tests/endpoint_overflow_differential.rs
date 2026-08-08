// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::process::Command;

use solstone_core_generate_wire::{OverflowDecision, endpoint_overflow_decision};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let venv = repository_root().join(".venv/bin/python3");
    assert!(venv.is_file(), "differential requires make install");
    venv
}

fn python_decision(body: &str, served_window: Option<u32>, attempt: u32) -> (String, Option<u32>) {
    let script = concat!(
        "import json, os, sys\n",
        "sys.path.insert(0, os.environ['SOLSTONE_REPO_ROOT'])\n",
        "from solstone.think.providers.local import _endpoint_overflow_decision\n",
        "window = None if sys.argv[2] == 'none' else int(sys.argv[2])\n",
        "decision = _endpoint_overflow_decision(sys.argv[1], window, int(sys.argv[3]))\n",
        "print(json.dumps([decision.kind, decision.max_tokens]))\n",
    );
    let output = Command::new(python())
        .args([
            "-c",
            script,
            body,
            &served_window.map_or_else(|| "none".into(), |value| value.to_string()),
            &attempt.to_string(),
        ])
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .output()
        .expect("run Python endpoint overflow decision");
    assert!(
        output.status.success(),
        "Python stderr: {:?}",
        output.stderr
    );
    serde_json::from_slice(&output.stdout).expect("Python decision JSON")
}

fn rust_decision(body: &str, served_window: Option<u32>, attempt: u32) -> (String, Option<u32>) {
    match endpoint_overflow_decision(body, served_window, attempt) {
        OverflowDecision::Retry(max_tokens) => ("retry".into(), Some(max_tokens)),
        OverflowDecision::Budget => ("budget".into(), None),
        OverflowDecision::Context => ("context".into(), None),
        OverflowDecision::Contract => ("contract".into(), None),
    }
}

#[test]
fn overflow_decisions_match_python() {
    for (body, served_window, attempt) in [
        (
            "maximum context length of 1000 tokens: 600 tokens from the input messages and 400 tokens for the completion",
            None,
            0,
        ),
        (
            "maximum context length of 1000 tokens: 600 tokens from the input messages and 400 tokens for the completion",
            None,
            1,
        ),
        (
            "maximum context length of 1000 tokens: 800 tokens from the input messages and 400 tokens for the completion",
            None,
            0,
        ),
        (
            "600 tokens from the input messages and 400 tokens for the completion",
            Some(1000),
            0,
        ),
        ("request exceeds the context window", None, 0),
        ("unexpected endpoint response", None, 0),
    ] {
        assert_eq!(
            rust_decision(body, served_window, attempt),
            python_decision(body, served_window, attempt),
            "body={body:?}, served_window={served_window:?}, attempt={attempt}"
        );
    }
}
