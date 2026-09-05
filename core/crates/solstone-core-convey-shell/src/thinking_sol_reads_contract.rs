// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-shape contract for the isolated native Thinking read handlers.

fn contains_forbidden_spawn(source: &str) -> bool {
    ["Command::new", ".spawn(", ".output(", "tokio::process"]
        .into_iter()
        .any(|token| source.contains(token))
}

#[test]
fn ac11_synthetic_process_launch_is_rejected() {
    assert!(contains_forbidden_spawn(
        "std::process::Command::new(\"python\").spawn()",
    ));
}
