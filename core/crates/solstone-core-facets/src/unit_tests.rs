// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::json;

use crate::enrich_relationship_with_journal;

#[test]
fn enrichment_without_journal_identity_promotes_entity_id() {
    assert_eq!(
        enrich_relationship_with_journal(
            &json!({"entity_id":"fallback-id","description":"facet data"}),
            None,
        ),
        json!({"id":"fallback-id","description":"facet data"})
    );
}

#[test]
fn enrichment_overlays_present_journal_identity_fields() {
    assert_eq!(
        enrich_relationship_with_journal(
            &json!({
                "entity_id":"old-id",
                "description":"facet data",
                "aka":["relationship alias"],
                "is_principal":false,
                "blocked":false,
            }),
            Some(&json!({
                "id":"current-id",
                "name":"Current Name",
                "type":"person",
                "aka":["journal alias"],
                "is_principal":true,
                "blocked":true,
            })),
        ),
        json!({
            "id":"current-id",
            "name":"Current Name",
            "type":"person",
            "aka":["journal alias"],
            "is_principal":true,
            "blocked":true,
            "description":"facet data",
        })
    );
}

#[test]
fn well_formed_facet_id_validation() {
    use crate::is_well_formed_facet_id;

    assert!(is_well_formed_facet_id(
        "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"
    ));
    assert!(is_well_formed_facet_id(
        "00000000-0000-4000-8000-000000000000"
    ));
    assert!(is_well_formed_facet_id(
        "ffffffff-ffff-4fff-bfff-ffffffffffff"
    ));

    // Invalid length
    assert!(!is_well_formed_facet_id(
        "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5"
    ));
    assert!(!is_well_formed_facet_id(
        "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5de"
    ));

    // Wrong version (must be 4)
    assert!(!is_well_formed_facet_id(
        "a1b2c3d4-e5f6-1a7b-8c9d-0e1f2a3b4c5d"
    ));
    assert!(!is_well_formed_facet_id(
        "a1b2c3d4-e5f6-5a7b-8c9d-0e1f2a3b4c5d"
    ));

    // Wrong variant (must be 8, 9, a, b)
    assert!(!is_well_formed_facet_id(
        "a1b2c3d4-e5f6-4a7b-0c9d-0e1f2a3b4c5d"
    ));
    assert!(!is_well_formed_facet_id(
        "a1b2c3d4-e5f6-4a7b-7c9d-0e1f2a3b4c5d"
    ));
    assert!(!is_well_formed_facet_id(
        "a1b2c3d4-e5f6-4a7b-cc9d-0e1f2a3b4c5d"
    ));

    // Uppercase rejection (must be lowercase)
    assert!(!is_well_formed_facet_id(
        "A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D"
    ));

    // Non-hex characters
    assert!(!is_well_formed_facet_id(
        "g1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"
    ));
}

#[test]
fn strip_incoming_facet_id_removes_id_only() {
    use crate::strip_incoming_facet_id;

    let mut doc = json!({
        "id": "foreign-id-12345",
        "title": "Work",
        "color": "#667eea",
        "emoji": "💼"
    });
    strip_incoming_facet_id(&mut doc);
    assert_eq!(
        doc,
        json!({
            "title": "Work",
            "color": "#667eea",
            "emoji": "💼"
        })
    );

    let mut non_object = json!("string-value");
    strip_incoming_facet_id(&mut non_object);
    assert_eq!(non_object, json!("string-value"));
}
