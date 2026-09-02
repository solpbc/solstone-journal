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
