// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-shape contract for the isolated native Thinking read handlers.

// This declared inventory is the complete new read-handler scope: adding a
// handler requires adding its filename here, making any omission visible in
// review just as settings-web's explicit source inventory does. Pre-existing
// shell files (thinking.rs, lib.rs, restart protocol, and nix signal handling)
// are deliberately out of scope because they legitimately manage processes;
// this contract bounds only the new read surface.
const READ_HANDLER_SOURCES: &[(&str, &str)] = &[(
    "thinking_sol_reads.rs",
    include_str!("thinking_sol_reads.rs"),
)];

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

#[test]
fn ac11_thinking_sol_read_handlers_contain_no_process_launch() {
    for (name, source) in READ_HANDLER_SOURCES {
        assert!(
            !contains_forbidden_spawn(source),
            "{name} launches a process"
        );
    }
}
