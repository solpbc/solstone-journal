// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{BodyDay, BodySourceFamily, BodySourceHash};

const BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn day(value: &str) -> BodyDay {
    BodyDay::from_bytes(value.as_bytes()).expect("test day is valid")
}

fn source_hash(family: BodySourceFamily, suffix: &str) -> BodySourceHash {
    BodySourceHash::from_bytes_for_family(format!("{BASE}{suffix}").as_bytes(), &family)
        .expect("test source hash is valid")
}

#[test]
fn plain_source_hashes_have_no_day_bound() {
    for family in [BodySourceFamily::AppleHealth, BodySourceFamily::OuraApi] {
        let hash = source_hash(family, "");
        assert!(hash.includes_day(&day("00010101")));
        assert!(hash.includes_day(&day("99991231")));
    }
}

#[test]
fn closed_apple_window_includes_both_boundaries_and_interior_only() {
    let hash = source_hash(
        BodySourceFamily::AppleHealth,
        "#window:20260102:20260104",
    );

    assert!(!hash.includes_day(&day("20260101")));
    assert!(hash.includes_day(&day("20260102")));
    assert!(hash.includes_day(&day("20260103")));
    assert!(hash.includes_day(&day("20260104")));
    assert!(!hash.includes_day(&day("20260105")));
}

#[test]
fn left_open_apple_window_has_only_an_inclusive_upper_bound() {
    let hash = source_hash(
        BodySourceFamily::AppleHealth,
        "#window:open:20260102",
    );

    assert!(hash.includes_day(&day("00010101")));
    assert!(hash.includes_day(&day("20260102")));
    assert!(!hash.includes_day(&day("20260103")));
}

#[test]
fn right_open_apple_window_has_only_an_inclusive_lower_bound() {
    let hash = source_hash(
        BodySourceFamily::AppleHealth,
        "#window:20260102:open",
    );

    assert!(!hash.includes_day(&day("20260101")));
    assert!(hash.includes_day(&day("20260102")));
    assert!(hash.includes_day(&day("99991231")));
}

#[test]
fn single_day_apple_window_includes_exactly_that_day() {
    let hash = source_hash(
        BodySourceFamily::AppleHealth,
        "#window:20260102:20260102",
    );

    assert!(!hash.includes_day(&day("20260101")));
    assert!(hash.includes_day(&day("20260102")));
    assert!(!hash.includes_day(&day("20260103")));
}
