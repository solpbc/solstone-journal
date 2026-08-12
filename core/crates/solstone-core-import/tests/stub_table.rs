// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use solstone_core_import::{ImportError, MODULE_STUBS};

#[test]
fn every_import_module_stub_is_unique_and_unimplemented() {
    assert_eq!(MODULE_STUBS.len(), 11);
    let names = MODULE_STUBS
        .iter()
        .map(|(name, _)| *name)
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), MODULE_STUBS.len());

    for (name, stub) in MODULE_STUBS {
        assert!(matches!(
            stub(),
            Err(ImportError::Unimplemented { module }) if module == *name
        ));
    }
}

#[test]
fn implemented_import_modules_have_no_unimplemented_seam() {
    for implemented in [
        "contract", "detect", "timestamp", "staging", "metadata", "dedupe", "publish", "events",
    ] {
        assert!(
            MODULE_STUBS
                .iter()
                .all(|(module, _)| *module != implemented)
        );
    }
}
