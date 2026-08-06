// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::store_tests::{TempDir, create_test_facet};
use crate::{
    FacetEntityWriteError, delete_detected_entity, read_detected_entities, save_detected_entity,
    update_detected_entity,
};

#[test]
fn detected_save_is_fold_insensitive_but_update_and_delete_are_exact() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    save_detected_entity(
        temporary.path(),
        "scope",
        "20260101",
        "kind",
        "Straße",
        "one",
    )
    .unwrap();
    assert!(matches!(
        save_detected_entity(
            temporary.path(),
            "scope",
            "20260101",
            "kind",
            "STRASSE",
            "two"
        ),
        Err(FacetEntityWriteError::EntityExists { .. })
    ));
    assert!(matches!(
        update_detected_entity(temporary.path(), "scope", "20260101", "STRASSE", "two"),
        Err(FacetEntityWriteError::EntityNotFound { .. })
    ));
    update_detected_entity(temporary.path(), "scope", "20260101", "Straße", "two").unwrap();
    assert!(
        delete_detected_entity(temporary.path(), "scope", "20260101", "STRASSE")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        delete_detected_entity(temporary.path(), "scope", "20260101", "Straße")
            .unwrap()
            .len(),
        1
    );
    assert!(
        read_detected_entities(temporary.path(), "scope", "20260101")
            .unwrap()
            .is_empty()
    );
}
