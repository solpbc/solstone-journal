// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const SCHEDULE_MOD: &str = include_str!("../../solstone-core-system/src/schedule/mod.rs");
const SCHEDULE_SOURCES: &[(&str, &str)] = &[
    (
        "caps",
        include_str!("../../solstone-core-system/src/schedule/caps.rs"),
    ),
    (
        "completion",
        include_str!("../../solstone-core-system/src/schedule/completion.rs"),
    ),
    (
        "config",
        include_str!("../../solstone-core-system/src/schedule/config.rs"),
    ),
    (
        "due",
        include_str!("../../solstone-core-system/src/schedule/due.rs"),
    ),
    (
        "engine",
        include_str!("../../solstone-core-system/src/schedule/engine.rs"),
    ),
    (
        "report",
        include_str!("../../solstone-core-system/src/schedule/report.rs"),
    ),
    (
        "status",
        include_str!("../../solstone-core-system/src/schedule/status.rs"),
    ),
    (
        "submission",
        include_str!("../../solstone-core-system/src/schedule/submission.rs"),
    ),
];
const CLI: &str = include_str!("../../solstone-core-cli/src/lib.rs");
const MAIN: &str = include_str!("../src/main.rs");
const JOURNAL_IO: &str = include_str!("../../solstone-core-journal-io/src/lib.rs");
const JOURNAL_IO_READERS: &str = include_str!("../../solstone-core-journal-io/src/readers.rs");

fn function_source<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source.find(name).expect("function exists");
    let after = &source[start + name.len()..];
    &source[start..start + name.len() + after.find("\nfn ").unwrap_or(after.len())]
}

#[test]
fn scan_covers_every_declared_schedule_module() {
    let declared = SCHEDULE_MOD
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix("mod ")
                .and_then(|rest| rest.strip_suffix(';'))
        })
        .collect::<Vec<_>>();
    assert_eq!(declared.len(), SCHEDULE_SOURCES.len());
    for module in declared {
        assert!(
            SCHEDULE_SOURCES.iter().any(|(name, _)| *name == module),
            "unscanned module {module}"
        );
    }
}

#[test]
fn schedule_read_path_has_no_spawn_or_ambient_reach() {
    for (name, source) in SCHEDULE_SOURCES
        .iter()
        .copied()
        .chain([
            ("journal-io", JOURNAL_IO),
            ("journal-io-readers", JOURNAL_IO_READERS),
        ])
        .chain([
            ("schedule-parser", function_source(CLI, "fn parse_schedule")),
            ("schedule-handler", function_source(MAIN, "fn run_schedule")),
        ])
    {
        for forbidden in [
            "Command::new",
            "std::process",
            "std::env",
            "python",
            "python3",
        ] {
            assert!(
                !source.contains(forbidden),
                "{name} reaches forbidden surface {forbidden}"
            );
        }
    }
    assert!(CLI.contains("Schedule(ScheduleOptions)"));
    assert!(CLI.contains("command == OsStr::new(\"schedule\")"));
}

#[test]
fn report_is_a_read_only_projection() {
    let report = SCHEDULE_SOURCES
        .iter()
        .find_map(|(name, source)| (*name == "report").then_some(*source))
        .expect("report registered");
    for forbidden in [
        "ScheduleEngine",
        ".check(",
        ".catch_up(",
        ".record_completion(",
        "atomic_replace",
        "fs::write",
        "write(",
    ] {
        assert!(
            !report.contains(forbidden),
            "report reaches write/runtime surface {forbidden}"
        );
    }
}
