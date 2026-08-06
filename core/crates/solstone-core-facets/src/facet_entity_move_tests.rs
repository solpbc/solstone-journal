// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use serde_json::json;

use crate::move_facet_entity;
use crate::store_tests::{
    TempDir, create_test_facet, relationship_value, write_facet_relationship,
};

#[test]
fn move_merge_reconciles_link_fields_and_accounts_for_extra_files() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "from");
    create_test_facet(temporary.path(), "to");
    write_facet_relationship(
        temporary.path(),
        "from",
        "subject",
        json!({"entity_id":"id","description":"source","attached_at":"2026-01","updated_at":"2026-03","last_seen":"2026-04","source_only":"yes"}),
    );
    write_facet_relationship(
        temporary.path(),
        "to",
        "subject",
        json!({"entity_id":"id","description":"destination","attached_at":"2026-02","updated_at":"2026-02","last_seen":"2026-02"}),
    );
    let extra = temporary
        .path()
        .join("facets/from/entities/subject/extra.bin");
    fs::write(&extra, b"bytes").unwrap();
    move_facet_entity(temporary.path(), "subject", "from", "to", true).unwrap();
    let relationship = relationship_value(temporary.path(), "to", "subject");
    assert_eq!(relationship["description"], "destination");
    assert_eq!(relationship["attached_at"], "2026-01");
    assert_eq!(relationship["updated_at"], "2026-03");
    assert_eq!(relationship["source_only"], "yes");
    assert_eq!(relationship["last_seen"], "2026-04");
    assert_eq!(
        fs::read(
            temporary
                .path()
                .join("facets/to/entities/subject/extra.bin")
        )
        .unwrap(),
        b"bytes"
    );
    assert!(
        !temporary
            .path()
            .join("facets/from/entities/subject")
            .exists()
    );
}
