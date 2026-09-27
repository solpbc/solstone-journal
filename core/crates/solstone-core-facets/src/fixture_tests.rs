// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use serde::Deserialize;

use crate::store_tests::TempDir;

const LIFECYCLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_lifecycle.json"
));

#[derive(Deserialize)]
struct LifecycleFixture {
    target_entity_id: String,
    journal_files: std::collections::BTreeMap<String, String>,
    expected_counts: LifecycleCounts,
}

#[derive(Deserialize)]
struct LifecycleCounts {
    unrecognized_file: usize,
    facet_relationship: usize,
    observation: usize,
    activity: usize,
    segment_label: usize,
    segment_correction: usize,
    aka_crossref: usize,
    speaker_candidate: usize,
    keep_separate: usize,
    identify_operation: usize,
    ambiguity: usize,
    entity_review_candidate: usize,
    speaker_review_candidate: usize,
    candidate_pair: usize,
    dismissal: usize,
    unreadable: usize,
}

#[test]
fn lifecycle_fixture_scans_python_writer_backed_inputs() {
    let fixture: LifecycleFixture = serde_json::from_str(LIFECYCLE).unwrap();
    let temporary = TempDir::new();
    for (relative, contents) in fixture.journal_files {
        let path = temporary.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    let counts = crate::store::reference_scan::scan_entity_references(
        temporary.path(),
        &fixture.target_entity_id,
        &fixture.target_entity_id,
        None,
    )
    .unwrap();
    assert_eq!(
        counts.unrecognized_file,
        fixture.expected_counts.unrecognized_file
    );
    assert_eq!(
        counts.facet_relationship,
        fixture.expected_counts.facet_relationship
    );
    assert_eq!(counts.observation, fixture.expected_counts.observation);
    assert_eq!(counts.activity, fixture.expected_counts.activity);
    assert_eq!(counts.segment_label, fixture.expected_counts.segment_label);
    assert_eq!(
        counts.segment_correction,
        fixture.expected_counts.segment_correction
    );
    assert_eq!(counts.aka_crossref, fixture.expected_counts.aka_crossref);
    assert_eq!(
        counts.speaker_candidate,
        fixture.expected_counts.speaker_candidate
    );
    assert_eq!(counts.keep_separate, fixture.expected_counts.keep_separate);
    assert_eq!(
        counts.identify_operation,
        fixture.expected_counts.identify_operation
    );
    assert_eq!(counts.ambiguity, fixture.expected_counts.ambiguity);
    assert_eq!(
        counts.entity_review_candidate,
        fixture.expected_counts.entity_review_candidate
    );
    assert_eq!(
        counts.speaker_review_candidate,
        fixture.expected_counts.speaker_review_candidate
    );
    assert_eq!(
        counts.candidate_pair,
        fixture.expected_counts.candidate_pair
    );
    assert_eq!(counts.dismissal, fixture.expected_counts.dismissal);
    assert_eq!(counts.unreadable, fixture.expected_counts.unreadable);
}
