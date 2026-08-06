// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The staged name's invisibility is pinned to the committed cross-language
//! fixture, not to a comment.
//!
//! `core/fixtures/segment_name_oracle.json` records, for 15 directory names, what
//! each of the two implementations that classify a segment by its name decides —
//! both executed, and gated to agree. This test reads it and asserts that the name
//! the removal executor actually produces is one the fixture says is invisible, and
//! that the obvious alternative is one the fixture says is not.
//!
//! ⚠ Without this, the staging choice rests on a comment, and the wrong choice is
//! the one a reader would reach for: comparable systems decorate with a suffix, and
//! a suffix here leaves the directory still recognised as a segment under its
//! undecorated key.

use serde_json::Value;
use solstone_core_retention::staging::{original_name, staged_name};

const ORACLE: &str = include_str!("../../../fixtures/segment_name_oracle.json");

fn rows() -> Vec<(String, bool, Option<String>)> {
    let document: Value = serde_json::from_str(ORACLE).expect("the oracle is valid JSON");
    document
        .get("rows")
        .and_then(Value::as_array)
        .expect("the oracle has rows")
        .iter()
        .map(|row| {
            (
                row.get("name")
                    .and_then(Value::as_str)
                    .expect("a row names a directory")
                    .to_owned(),
                row.get("is_segment")
                    .and_then(Value::as_bool)
                    .expect("a row says whether it is a segment"),
                row.get("rust_key")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )
        })
        .collect()
}

fn is_segment(name: &str) -> Option<bool> {
    rows()
        .into_iter()
        .find(|(candidate, ..)| candidate == name)
        .map(|(_, is_segment, _)| is_segment)
}

/// The name the executor produces is one the fixture says is not a segment.
#[test]
fn the_staged_name_is_not_recognised_as_a_segment() {
    let staged = staged_name("070000_17");
    assert_eq!(
        is_segment(&staged),
        Some(false),
        "the fixture must cover `{staged}` and say it is not a segment"
    );
}

/// The obvious alternative is one the fixture says IS a segment, under the
/// undecorated key — which is the defect the prefix avoids.
#[test]
fn a_suffixed_name_would_still_be_a_segment_under_the_bare_key() {
    let (_, recognised, key) = rows()
        .into_iter()
        .find(|(name, ..)| name == "070000_17.removing")
        .expect("the fixture covers the suffix form");
    assert!(
        recognised,
        "a suffixed staged name is still recognised as a segment"
    );
    assert_eq!(
        key.as_deref(),
        Some("070000_17"),
        "and under the UNDECORATED key, so an iterator would return two entries \
         with one key and two paths"
    );
}

/// Every name the fixture calls a segment survives a staging round trip
/// unchanged — including the one whose key differs from its name.
#[test]
fn staging_round_trips_every_segment_name_in_the_fixture() {
    let mut checked = 0usize;
    for (name, recognised, key) in rows() {
        if !recognised {
            continue;
        }
        let staged = staged_name(&name);
        assert_eq!(
            original_name(&staged),
            Some(name.as_str()),
            "staging `{name}` must restore it exactly"
        );
        // The fixture covers 15 specific names, so most staged forms are not in
        // it. Where it does cover one, it must say the staged form is invisible.
        if let Some(covered) = is_segment(&staged) {
            assert!(!covered, "`{staged}` must not be recognised as a segment");
        }
        // The case that catches a staged name built from the key rather than the
        // directory name.
        if let Some(key) = key.filter(|key| *key != name) {
            assert_ne!(
                staged_name(&key),
                staged,
                "`{name}` has key `{key}`; staging on the key would restore the \
                 wrong directory"
            );
        }
        checked = checked.saturating_add(1);
    }
    assert!(
        checked > 0,
        "the fixture produced no segment names, so this asserted nothing"
    );
}
