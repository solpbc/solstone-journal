// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The stats request graph is deliberately source-audited. The sole shell
//! spawn, speaker-resolve discovery_helper.rs, is outside this graph and is not
//! silently covered by this test.

const CORE_DISPATCH: &str = include_str!("../../../solstone-core/src/main.rs");
const SHELL_COMPOSITION: &str = include_str!("../../../solstone-core-convey-shell/src/lib.rs");
const SESSION_GATE: &str = include_str!("../../../solstone-core-convey-shell/src/session_gate.rs");
const STATS_WEB: &str = include_str!("../../../solstone-core-stats-web/src/lib.rs");
const STATS_TOKENS: &str = include_str!("../../../solstone-core-stats-web/src/tokens.rs");
const TALENT_CONFIG: &str = include_str!("../../../solstone-core-talent-config/src/lib.rs");
const JOURNAL_READER: &str = include_str!("../../../solstone-core-journal-io/src/readers.rs");
const JOURNAL_PATHS: &str = include_str!("../../../solstone-core-journal-io/src/paths.rs");

fn rejects_dispatch(source: &str) -> bool {
    [
        "Command::new",
        "std::process::Command",
        "tokio::process::Command",
        "/usr/bin/python",
        "/bin/python",
        "python.exe",
    ]
    .iter()
    .any(|needle| source.contains(needle))
}

#[test]
fn ac18_stats_request_graph_contains_no_process_dispatch() {
    for (seam, source) in [
        ("core convey dispatch", CORE_DISPATCH),
        ("shell composition", SHELL_COMPOSITION),
        ("session gate", SESSION_GATE),
        ("stats routes", STATS_WEB),
        ("stats token fold", STATS_TOKENS),
        ("talent config", TALENT_CONFIG),
        ("journal reader", JOURNAL_READER),
        ("journal paths", JOURNAL_PATHS),
    ] {
        assert!(
            !rejects_dispatch(source),
            "{seam} must not dispatch a process"
        );
    }
}

#[test]
fn ac18_rejects_forbidden_spawn_mutation_at_each_stats_graph_seam() {
    for seam in [
        "core",
        "shell",
        "session",
        "stats",
        "talent-config",
        "journal-io",
    ] {
        assert!(
            rejects_dispatch(&format!("{seam}; std::process::Command::new(\"python3\")")),
            "{seam}"
        );
        assert!(
            rejects_dispatch(&format!("{seam}; \"/usr/bin/python3\"")),
            "{seam}"
        );
    }
}
