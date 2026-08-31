// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use serde_json::json;

use crate::store_tests::{TempDir, create_test_facet, write_facet_relationship};
use crate::{
    FacetRelationshipRecord, load_all_facet_relationships,
    load_all_facet_relationships_across_facets, scan_facet_relationships,
};

#[test]
fn per_facet_scans_keep_raw_relationship_directories_without_an_identity_join() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_facet_relationship(
        temporary.path(),
        "work",
        "legacy-directory",
        json!({"entity_id":"unresolved-id","description":"relationship"}),
    );

    assert_eq!(
        scan_facet_relationships(temporary.path(), "work").unwrap(),
        vec!["legacy-directory"]
    );
    assert_eq!(
        load_all_facet_relationships(temporary.path(), "work").unwrap(),
        [(
            "legacy-directory".to_owned(),
            json!({"entity_id":"unresolved-id","description":"relationship"})
        )]
        .into()
    );
}

#[test]
fn across_facets_keys_shared_relationship_directories_by_resolved_journal_entity() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    create_test_facet(temporary.path(), "personal");
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "work-entity",
        &json!({"id":"work-entity","name":"Work Acme"}),
        None,
    )
    .unwrap();
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "personal-entity",
        &json!({"id":"personal-entity","name":"Personal Acme"}),
        None,
    )
    .unwrap();
    write_facet_relationship(
        temporary.path(),
        "work",
        "acme-corp",
        json!({"entity_id":"work-entity","description":"work relationship"}),
    );
    write_facet_relationship(
        temporary.path(),
        "personal",
        "acme-corp",
        json!({"entity_id":"personal-entity","description":"personal relationship"}),
    );

    assert_eq!(
        load_all_facet_relationships_across_facets(temporary.path()).unwrap(),
        [
            (
                "personal-entity".to_owned(),
                vec![FacetRelationshipRecord {
                    facet_dir: "personal".to_owned(),
                    relationship: json!({"entity_id":"personal-entity","description":"personal relationship"}),
                }],
            ),
            (
                "work-entity".to_owned(),
                vec![FacetRelationshipRecord {
                    facet_dir: "work".to_owned(),
                    relationship: json!({"entity_id":"work-entity","description":"work relationship"}),
                }],
            ),
        ]
        .into()
    );
}
