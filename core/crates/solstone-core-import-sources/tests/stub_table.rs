// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use solstone_core_import_sources::{ImportSourcesError, MODULE_STUBS};

const IMPLEMENTED_SOURCE_MODULES: &[&str] = &[
    "chatgpt", "claude", "document", "gemini", "ics", "image", "kindle", "obsidian",
];

#[test]
fn every_source_module_stub_is_unique_and_unimplemented() {
    assert_eq!(MODULE_STUBS.len(), 3);
    let names = MODULE_STUBS
        .iter()
        .map(|(name, _)| *name)
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), MODULE_STUBS.len());
    assert!(!names.contains("archive"));
    assert!(!names.contains("journal_archive"));

    for (name, stub) in MODULE_STUBS {
        assert_eq!(
            stub(),
            Err(ImportSourcesError::Unimplemented { module: name }),
            "stub {name} must not report success"
        );
    }
}

#[test]
fn implemented_source_modules_have_no_unimplemented_seam() {
    for implemented in IMPLEMENTED_SOURCE_MODULES {
        assert!(
            MODULE_STUBS
                .iter()
                .all(|(module, _)| *module != *implemented),
            "implemented source module {implemented} must not retain a stub"
        );
    }
}
