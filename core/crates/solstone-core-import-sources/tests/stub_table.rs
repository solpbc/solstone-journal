// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use solstone_core_import_sources::{ImportSourcesError, MODULE_STUBS};

// One name per line, deliberately. Waves retire stubs from this crate concurrently, so a wave
// that retires a different module than its sibling edits a different line here and the two
// merge cleanly. A count cannot do that: a bare `MODULE_STUBS.len()` literal is one line both
// siblings must change, so git takes one side's number silently, with no conflict marker, and
// the result compiles and passes while being wrong.
const EXPECTED_STUBS: &[&str] = &[
    "apple_health", //
    "oura",         //
    "registry",     //
];

// Same reasoning, same one-per-line shape.
const IMPLEMENTED_SOURCE_MODULES: &[&str] = &[
    "archive",  //
    "chatgpt",  //
    "claude",   //
    "document", //
    "gemini",   //
    "ics",      //
    "image",    //
    "kindle",   //
    "obsidian", //
];

// The retired name of the archive source. It must not reappear under either spelling.
const RETIRED_MODULE_NAMES: &[&str] = &[
    "journal_archive", //
];

fn stub_names() -> BTreeSet<&'static str> {
    MODULE_STUBS.iter().map(|(name, _)| *name).collect()
}

#[test]
fn every_source_module_stub_is_unique_and_unimplemented() {
    let names = stub_names();

    // Compare the set of names, never a count. This is what makes a wrong merge fail loudly: a
    // stale entry names the module it is stale about, rather than reporting a number that
    // happens not to match.
    let expected = EXPECTED_STUBS.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        names, expected,
        "stub table drifted from the expected set; retire the entry here in the same change that \
         retires the module"
    );

    // Uniqueness is a property of the table itself and still worth asserting directly: the set
    // collapses duplicates the slice would happily carry.
    assert_eq!(
        names.len(),
        MODULE_STUBS.len(),
        "stub table contains a duplicate module name"
    );

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
    let names = stub_names();
    for implemented in IMPLEMENTED_SOURCE_MODULES {
        assert!(
            !names.contains(*implemented),
            "implemented source module {implemented} must not retain a stub"
        );
    }
}

#[test]
fn retired_module_names_do_not_reappear() {
    let names = stub_names();
    for retired in RETIRED_MODULE_NAMES {
        assert!(
            !names.contains(*retired),
            "retired module name {retired} must not reappear in the stub table"
        );
    }
}

#[test]
fn the_two_module_lists_are_disjoint() {
    // Without this, a module present in both lists would satisfy each test separately: the stub
    // set would match, and the implemented check would fail only if the stub were still there.
    let stubs = stub_names();
    let implemented = IMPLEMENTED_SOURCE_MODULES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();

    let overlap = stubs
        .intersection(&implemented)
        .copied()
        .collect::<Vec<_>>();
    assert!(
        overlap.is_empty(),
        "modules listed as both stubbed and implemented: {overlap:?}"
    );
}
