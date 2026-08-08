// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::process::Command;

use solstone_core_generate_wire::classify_output_responsiveness;

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

fn python_verdict(output: &str) -> (bool, Option<String>, bool) {
    let script = concat!(
        "import json, os, sys\n",
        "sys.path.insert(0, os.environ['SOLSTONE_REPO_ROOT'])\n",
        "from solstone.think.responsiveness import classify_output_responsiveness\n",
        "verdict = classify_output_responsiveness(sys.argv[1])\n",
        "print(json.dumps([verdict.non_responsive, verdict.matched_signal, verdict.empty_corpus]))\n",
    );
    let output = Command::new(python())
        .args(["-c", script, output])
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .output()
        .expect("run Python responsiveness classifier");
    assert!(
        output.status.success(),
        "Python stderr: {:?}",
        output.stderr
    );
    serde_json::from_slice(&output.stdout).expect("Python verdict JSON")
}

fn rust_verdict(output: &str) -> (bool, Option<String>, bool) {
    let verdict = classify_output_responsiveness(output);
    (
        verdict.non_responsive,
        verdict
            .matched_signal
            .map(|signal| signal.as_log_value().to_owned()),
        verdict.empty_corpus,
    )
}

#[test]
fn responsiveness_matches_python() {
    for output in [
        "I cannot complete that request.",
        "I can't complete that request.",
        "I am not able to complete that request.",
        "I'm not able to complete that request.",
        "I am unable to complete that request.",
        "I'm unable to complete that request.",
        "I do not have access to that resource.",
        "I don't have access to that resource.",
        "I do not have the ability to complete that request.",
        "I don't have the ability to complete that request.",
        "As an AI, I cannot complete that request.",
        "Sorry, I cannot complete that request.",
        "I cannot inspect the original file, so here is a description of the image.",
        "I can't access the source, but the screenshot shows a blue menu.",
        "I cannot browse, though the supplied text says the answer is seven.",
        "A useful answer that directly addresses the request.",
        r#"{"answer":"Useful answer.","note":"I cannot complete that request."}"#,
        r#"{"at":"12:30"}"#,
    ] {
        assert_eq!(
            rust_verdict(output),
            python_verdict(output),
            "output={output:?}"
        );
    }
}
