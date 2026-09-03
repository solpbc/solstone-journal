// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The per-run oplog migration must not grow a new fixed-path writer.

const FORMER_WRITER_SOURCES: &[(&str, &str)] = &[
    (
        "think/activity",
        include_str!("../../../solstone-core-think-cli/src/activity.rs"),
    ),
    (
        "think/cadence",
        include_str!("../../../solstone-core-think-cli/src/cadence.rs"),
    ),
    (
        "think/daily",
        include_str!("../../../solstone-core-think-cli/src/daily.rs"),
    ),
    (
        "think/daily_lifecycle",
        include_str!("../../../solstone-core-think-cli/src/daily_lifecycle.rs"),
    ),
    (
        "think/dispatch",
        include_str!("../../../solstone-core-think-cli/src/dispatch.rs"),
    ),
    (
        "think/flush",
        include_str!("../../../solstone-core-think-cli/src/flush.rs"),
    ),
    (
        "think/helpers",
        include_str!("../../../solstone-core-think-cli/src/helpers.rs"),
    ),
    (
        "think/lib",
        include_str!("../../../solstone-core-think-cli/src/lib.rs"),
    ),
    (
        "think/run_log",
        include_str!("../../../solstone-core-think-cli/src/run_log.rs"),
    ),
    (
        "think/segment",
        include_str!("../../../solstone-core-think-cli/src/segment.rs"),
    ),
    (
        "think/weekly",
        include_str!("../../../solstone-core-think-cli/src/weekly.rs"),
    ),
    (
        "heartbeat",
        include_str!("../../../solstone-core/src/heartbeat.rs"),
    ),
    (
        "steward/pre_hook",
        include_str!("../../../solstone-core-talent-runtime/src/steward_log.rs"),
    ),
    (
        "offload/pruning_audit",
        include_str!("../../../solstone-core-offload/src/pruning_audit.rs"),
    ),
    (
        "offload/run",
        include_str!("../../../solstone-core-offload/src/run.rs"),
    ),
    (
        "service_unit/systemd",
        include_str!("../../../solstone-core-service-unit/src/systemd.rs"),
    ),
    (
        "service_unit/plist",
        include_str!("../../../solstone-core-service-unit/src/plist.rs"),
    ),
    (
        "service_capture",
        include_str!("../../../solstone-core/src/service_capture.rs"),
    ),
];

const RETIRED_OUTPUT_NAMES: &[&str] = &[
    "task_log.txt",
    "heartbeat.log",
    "steward.log",
    "retention.log",
    "pruning-runs",
    "service.log",
    "StandardOutput",
    "StandardError",
    "StandardOutPath",
    "StandardErrorPath",
];

fn production_source(source: &str) -> &str {
    ["\n#[cfg(test)]", "\nmod tests"]
        .into_iter()
        .filter_map(|boundary| source.find(boundary))
        .min()
        .map_or(source, |boundary| &source[..boundary])
}

fn retired_output_name(source: &str) -> Option<&'static str> {
    RETIRED_OUTPUT_NAMES
        .iter()
        .copied()
        .find(|name| source.contains(name))
}

#[test]
fn former_diagnostic_writer_sources_do_not_name_retired_fixed_outputs() {
    for (owner, source) in FORMER_WRITER_SOURCES {
        let source = production_source(source);
        assert!(
            retired_output_name(source).is_none(),
            "{owner} names retired fixed diagnostic output {}",
            retired_output_name(source).expect("rejected source identifies the retired output"),
        );
    }
}

#[test]
fn retired_output_rejection_is_load_bearing() {
    assert_eq!(
        retired_output_name("open(\"task_log.txt\")"),
        Some("task_log.txt")
    );
    assert_eq!(
        retired_output_name("StandardOutput=append:/journal/health/service.log"),
        Some("service.log")
    );
}
