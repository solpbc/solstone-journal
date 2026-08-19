// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Named partition decision table. The `solstone-core-journal` basename branch
//! is owned by `src/partition.rs::resolves_service_partition_for_sibling_journal_binary`
//! and is deliberately absent here.

use solstone_core_system::partition::partition_for;

fn decision_table() -> Vec<(&'static str, Vec<&'static str>, &'static str)> {
    vec![
        // the two accepted heads, exact
        (
            "journal_head",
            vec!["journal", "indexer", "--rescan"],
            "indexer",
        ),
        ("sol_head", vec!["solstone", "heartbeat"], "heartbeat"),
        // the `think` flag ladder, in its declared first-hit order
        ("think_bare_is_daily", vec!["journal", "think"], "daily"),
        (
            "think_activity",
            vec!["journal", "think", "--activity", "a"],
            "activity",
        ),
        ("think_flush", vec!["journal", "think", "--flush"], "flush"),
        (
            "think_segments",
            vec!["journal", "think", "--segments"],
            "segment",
        ),
        (
            "think_weekly",
            vec!["journal", "think", "--weekly"],
            "weekly",
        ),
        (
            "think_cadence",
            vec!["journal", "think", "--cadence"],
            "cadence",
        ),
        (
            "think_segment",
            vec!["journal", "think", "--segment", "x"],
            "segment",
        ),
        // first hit wins: a production argv carries BOTH, and --flush precedes
        // --segment in the ladder. A set-membership port routes this elsewhere.
        (
            "think_flush_and_segment_first_hit_wins",
            vec!["journal", "think", "--segment", "x", "--flush"],
            "flush",
        ),
        // maintenance sub-partitions only at the right arity and shape
        (
            "maintenance_run_subpartition",
            vec!["journal", "maintenance", "run", "backup:run"],
            "maintenance:backup:run",
        ),
        (
            "maintenance_short_argv",
            vec!["journal", "maintenance", "run"],
            "maintenance",
        ),
        (
            "maintenance_not_run_verb",
            vec!["journal", "maintenance", "status", "backup:run"],
            "maintenance",
        ),
        // path-form fallback: basename of argv[0], NOT a service command
        (
            "path_form_journal_is_basename",
            vec!["/opt/tools/journal", "backup"],
            "journal",
        ),
        (
            "path_form_sol_is_basename",
            vec!["/usr/local/bin/solstone", "backup"],
            "solstone",
        ),
        ("unrelated_binary", vec!["/usr/bin/rsync", "-av"], "rsync"),
        // degenerate shapes
        ("head_only_no_subcommand", vec!["journal"], "journal"),
        ("empty_argv", vec![], "unknown"),
    ]
}

#[test]
fn partition_for_matches_named_decision_table() {
    for (name, argv, expected) in decision_table() {
        let argv = argv.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(partition_for(&argv).as_str(), expected, "{name}");
    }
}
