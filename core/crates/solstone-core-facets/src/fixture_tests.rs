// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use serde::Deserialize;
use serde_json::json;

use crate::store_tests::{
    TempDir, create_test_facet, relationship_value, write_facet_relationship, write_journal_entity,
};
use crate::{FacetEntityLinkRepairBranch, repair_facet_entity_links};

const DIVERGENCES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_slug_native_divergences.json"
));
const LIFECYCLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_lifecycle.json"
));

#[derive(Deserialize)]
struct DivergenceFixture {
    entries: Vec<DivergenceEntry>,
}

#[derive(Deserialize)]
struct DivergenceEntry {
    category: String,
    native_slug: String,
    reference_slug: String,
}

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

#[test]
fn facet_link_repair_matches_every_assigned_slug_divergence_by_literal_directory_name() {
    let fixture: DivergenceFixture = serde_json::from_str(DIVERGENCES).unwrap();
    let entries = fixture
        .entries
        .iter()
        .filter(|entry| entry.category != "Cn")
        .collect::<Vec<_>>();
    assert!(
        entries
            .iter()
            .all(|entry| entry.native_slug != entry.reference_slug)
    );
    assert_eq!(entries.len(), 62);

    let mut processed = 0;
    for entry in entries {
        let temporary = TempDir::new();
        create_test_facet(temporary.path(), "work");
        write_journal_entity(temporary.path(), &entry.reference_slug, None);
        write_facet_relationship(
            temporary.path(),
            "work",
            &entry.reference_slug,
            json!({"lane": "fixture"}),
        );

        let report = repair_facet_entity_links(temporary.path(), "work").unwrap();

        assert!(report.branches.iter().any(|branch| matches!(
            branch,
            FacetEntityLinkRepairBranch::Linked { facet_entity_dir, journal_entity_id }
                if facet_entity_dir == &entry.reference_slug
                    && journal_entity_id == &entry.reference_slug
        )));
        assert_eq!(
            relationship_value(temporary.path(), "work", &entry.reference_slug)["entity_id"],
            entry.reference_slug
        );
        processed += 1;
    }
    assert_eq!(processed, 62);
}
