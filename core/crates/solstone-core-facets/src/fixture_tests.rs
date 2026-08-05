// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

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
