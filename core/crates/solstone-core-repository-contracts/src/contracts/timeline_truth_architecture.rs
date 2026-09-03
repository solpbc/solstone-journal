// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-derived ownership rules for the versioned timeline artifact pipeline.

use std::fs;
use std::path::Path;

use super::talent_config_reader_architecture::{mask_literals_and_comments, production_scopes};

const TIMELINE_MOD: &str = include_str!("../../../solstone-core-timeline/src/lib.rs");
const TIMELINE_SOURCES: &[(&str, &str)] = &[
    (
        "binding",
        include_str!("../../../solstone-core-timeline/src/binding.rs"),
    ),
    (
        "currentness",
        include_str!("../../../solstone-core-timeline/src/currentness.rs"),
    ),
    (
        "error",
        include_str!("../../../solstone-core-timeline/src/error.rs"),
    ),
    (
        "fingerprint",
        include_str!("../../../solstone-core-timeline/src/fingerprint.rs"),
    ),
    (
        "locks",
        include_str!("../../../solstone-core-timeline/src/locks.rs"),
    ),
    (
        "schema",
        include_str!("../../../solstone-core-timeline/src/schema.rs"),
    ),
    (
        "state",
        include_str!("../../../solstone-core-timeline/src/state.rs"),
    ),
    (
        "store",
        include_str!("../../../solstone-core-timeline/src/store.rs"),
    ),
];

const MAINTENANCE_TIMELINE: &str =
    include_str!("../../../solstone-core-maintenance/src/bodies/timeline.rs");
const FACETS_TIMELINE_PROJECTION: &str =
    include_str!("../../../solstone-core-facets-web/src/timeline/projection.rs");
const SYSTEM_HEALTH_TIMELINE_DIVERGENCE: &str =
    include_str!("../../../solstone-core-system-health/src/timeline_divergence.rs");
const DOCTOR_TIMELINE_DIVERGENCE: &str =
    include_str!("../../../solstone-core-doctor/src/checks/timeline_divergence.rs");
const MAINTENANCE_REGISTRY: &str =
    include_str!("../../../solstone-core-maintenance/src/registry.rs");
const MAINTENANCE_SCHEDULE_SYNC: &str =
    include_str!("../../../solstone-core-maintenance/src/schedule_sync.rs");

fn is_test_source(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "tests")
        || path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name == "test_support.rs" || name.ends_with("_tests.rs")
        })
}

fn has_direct_timeline_publication(scope: &str) -> bool {
    let masked = mask_literals_and_comments(scope);
    let code = String::from_utf8(masked).expect("Rust source remains UTF-8 after masking");
    let targets_timeline = scope.contains("timeline.json");
    let publishes = [
        "atomic_replace(",
        "atomic_replace_detailed(",
        "write_json(",
        "fs::write(",
        "std::fs::write(",
    ]
    .iter()
    .any(|surface| code.contains(surface));
    let constructs_v1_shape = code.contains("json!")
        && scope.contains("\"schema_version\"")
        && scope.contains("\"kind\"");
    targets_timeline && (publishes || constructs_v1_shape)
}

fn has_raw_timeline_value_reader(scope: &str) -> bool {
    let masked = mask_literals_and_comments(scope);
    let code = String::from_utf8(masked).expect("Rust source remains UTF-8 after masking");
    let raw_value_read = [
        "serde_json::from_str::<Value>",
        "serde_json::from_slice::<Value>",
        "serde_json::from_reader::<Value>",
    ]
    .iter()
    .any(|surface| code.contains(surface));
    scope.contains("timeline.json")
        && raw_value_read
        && !is_maintenance_shape_classifier(scope, &code)
}

fn is_maintenance_shape_classifier(scope: &str, code: &str) -> bool {
    let segment_classifier = code.contains("serde_json::from_slice::<Value>")
        && code.contains("serde_json::from_value::<SegmentTimelineV1>")
        && scope.contains("discover_day_segment_bindings")
        && scope.contains("malformed_json");
    let day_classifier = code.contains("serde_json::from_slice::<Value>")
        && code.contains("serde_json::from_slice::<DayTimelineV1>")
        && scope.contains("master_scan_failure")
        && scope.contains("malformed_json");
    segment_classifier || day_classifier
}

fn visit_production_sources(
    root: &Path,
    violations: &mut Vec<String>,
    predicate: fn(&str) -> bool,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !is_test_source(&path) {
                visit_production_sources(&path, violations, predicate);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && !is_test_source(&path)
        {
            let source = fs::read_to_string(&path).expect("repository source is readable");
            if production_scopes(&source)
                .iter()
                .any(|scope| predicate(scope))
            {
                violations.push(path.display().to_string());
            }
        }
    }
}

fn scan_workspace(predicate: fn(&str) -> bool) -> Vec<String> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory");
    let mut violations = Vec::new();
    for entry in fs::read_dir(crates)
        .expect("crates directory is readable")
        .flatten()
    {
        let name = entry.file_name();
        if name == "solstone-core-timeline" || name == "solstone-core-repository-contracts" {
            continue;
        }
        let source_root = entry.path().join("src");
        visit_production_sources(&source_root, &mut violations, predicate);
    }
    violations
}

fn descriptor_blocks(source: &str) -> Vec<&str> {
    source
        .split("RoutineDescriptor {")
        .skip(1)
        .filter(|block| block.contains("id: \"timeline:"))
        .collect()
}

fn descriptor_id(block: &str) -> &str {
    let after = block
        .split_once("id: \"")
        .expect("timeline descriptor has an id")
        .1;
    after.split_once('"').expect("timeline id closes").0
}

#[test]
fn scan_covers_every_declared_timeline_module() {
    let declared = TIMELINE_MOD
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix("mod ")
                .and_then(|rest| rest.strip_suffix(';'))
        })
        .collect::<Vec<_>>();
    assert_eq!(declared.len(), TIMELINE_SOURCES.len());
    for module in declared {
        assert!(
            TIMELINE_SOURCES.iter().any(|(name, _)| *name == module),
            "unscanned timeline module {module}"
        );
    }
}

#[test]
fn timeline_artifact_publication_has_one_owner() {
    let violations = scan_workspace(has_direct_timeline_publication);
    assert!(
        violations.is_empty(),
        "direct timeline artifact publication outside solstone-core-timeline: {violations:?}"
    );
}

#[test]
fn timeline_readers_are_typed_and_validated() {
    for (name, source, required) in [
        (
            "maintenance",
            MAINTENANCE_TIMELINE,
            [
                "SegmentTimelineV1",
                "DayTimelineV1",
                "MasterTimelineV1",
                "validate_segment_timeline",
                "validate_day_timeline",
                "validate_master_timeline",
            ],
        ),
        (
            "facets-web",
            FACETS_TIMELINE_PROJECTION,
            [
                "SegmentTimelineV1",
                "DayTimelineV1",
                "MasterTimelineV1",
                "validate_segment_timeline",
                "validate_day_timeline",
                "validate_master_timeline",
            ],
        ),
        (
            "system-health",
            SYSTEM_HEALTH_TIMELINE_DIVERGENCE,
            [
                "SegmentTimelineV1",
                "DayTimelineV1",
                "MasterTimelineV1",
                "validate_segment_timeline",
                "validate_day_timeline",
                "validate_master_timeline",
            ],
        ),
    ] {
        for required in required {
            assert!(
                source.contains(required),
                "{name} lacks typed reader {required}"
            );
        }
        if name != "maintenance" {
            assert!(
                ![
                    "serde_json::from_str::<Value>",
                    "serde_json::from_slice::<Value>",
                    "serde_json::from_reader::<Value>",
                ]
                .iter()
                .any(|surface| source.contains(surface)),
                "{name} parses a timeline artifact as an untyped Value"
            );
        }
    }

    // Maintenance parses a Value only to report malformed JSON separately from a wrong V1 shape,
    // then immediately deserializes to and validates the typed artifact before accepting it.
    assert!(MAINTENANCE_TIMELINE.contains("serde_json::from_value::<SegmentTimelineV1>"));
    assert!(MAINTENANCE_TIMELINE.contains("serde_json::from_slice::<DayTimelineV1>"));
    assert!(
        MAINTENANCE_TIMELINE.contains("serde_json::from_slice::<Value>"),
        "maintenance must keep its narrow malformed-versus-wrong-shape classifier"
    );
    assert!(
        DOCTOR_TIMELINE_DIVERGENCE.contains("diagnose_timeline_divergence"),
        "Doctor must delegate its timeline reads to the independent system-health diagnosis"
    );

    let violations = scan_workspace(has_raw_timeline_value_reader);
    assert!(
        violations.is_empty(),
        "raw Value timeline readers outside the typed artifact boundary: {violations:?}"
    );
}

#[test]
fn only_orchestrated_timeline_rollup_is_scheduled() {
    for retired in [
        "maintenance:timeline:rollup-day",
        "maintenance:timeline:rollup-master",
    ] {
        assert!(
            MAINTENANCE_SCHEDULE_SYNC.contains(retired),
            "retired timeline schedule {retired} is not recorded"
        );
    }
    assert!(MAINTENANCE_SCHEDULE_SYNC.contains("const RETIRED_ENTRIES: &[&str]"));

    let descriptors = descriptor_blocks(MAINTENANCE_REGISTRY);
    assert_eq!(
        descriptors.len(),
        3,
        "the complete timeline routine census must be reviewed here"
    );
    for descriptor in descriptors {
        match descriptor_id(descriptor) {
            "timeline:rollup" => assert!(
                descriptor.contains("args: &[\"--commit\"]"),
                "orchestrated timeline rollup must be the committing scheduled entry"
            ),
            "timeline:rollup-day" | "timeline:rollup-master" => assert!(
                descriptor.contains("args: &[]"),
                "retired per-stage timeline rollup must not retain scheduled arguments"
            ),
            unexpected => panic!("unreviewed timeline routine {unexpected}"),
        }
    }
}

#[test]
fn publication_predicate_catches_the_legacy_continuation_writer() {
    let legacy_writer = r#"
        fn write_continuation(segment: &Path) {
            let path = segment.join("timeline.json");
            let value = serde_json::json!({"title": "Continued"});
            atomic_replace(&path, &serde_json::to_vec(&value).unwrap(), options).unwrap();
        }
    "#;
    assert!(
        production_scopes(legacy_writer)
            .iter()
            .any(|scope| has_direct_timeline_publication(scope)),
        "the pre-V1 continuation writer must be rejected"
    );

    let test_only_writer = format!("#[cfg(test)]\nmod tests {{ {legacy_writer} }}");
    assert!(
        !production_scopes(&test_only_writer)
            .iter()
            .any(|scope| has_direct_timeline_publication(scope)),
        "test fixtures are not production artifact writers"
    );

    let comment_only_writer = format!("/* {legacy_writer} */\nfn unrelated() {{}}");
    assert!(
        !production_scopes(&comment_only_writer)
            .iter()
            .any(|scope| has_direct_timeline_publication(scope)),
        "comments are not production artifact writers"
    );
}

#[test]
fn raw_reader_predicate_catches_slice_and_reader_variants() {
    for reader in [
        "serde_json::from_slice::<Value>(bytes)",
        "serde_json::from_reader::<Value>(reader)",
    ] {
        let source =
            format!("fn read() {{ let path = root.join(\"timeline.json\"); let _ = {reader}; }}");
        assert!(
            production_scopes(&source)
                .iter()
                .any(|scope| has_raw_timeline_value_reader(scope)),
            "raw reader {reader} must be rejected"
        );
    }
}
