// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-shape contract for isolated native Thinking write handlers.

// This declared inventory is the complete new write-handler scope. Writes are
// delegated to their domain owners; handlers only bind HTTP requests to those
// owners and must not acquire locks or write journal files themselves.
const WRITE_HANDLER_SOURCES: &[(&str, &str)] = &[(
    "thinking_sol_writes.rs",
    include_str!("thinking_sol_writes.rs"),
)];

fn contains_forbidden_write(source: &str) -> bool {
    [
        "std::fs::",
        "File::",
        "hold_lock",
        "atomic_replace",
        "write_json",
        "commit_journal_config",
    ]
    .into_iter()
    .any(|token| source.contains(token))
}

fn contains_forbidden_spawn(source: &str) -> bool {
    ["Command::new", ".spawn(", ".output(", "tokio::process"]
        .into_iter()
        .any(|token| source.contains(token))
}

#[test]
fn thinking_sol_write_handlers_contain_no_process_launch_or_direct_write_owner() {
    for (name, source) in WRITE_HANDLER_SOURCES {
        assert!(
            !contains_forbidden_spawn(source),
            "{name} launches a process"
        );
        assert!(
            !contains_forbidden_write(source),
            "{name} bypasses a config or identity write owner"
        );
    }
}
