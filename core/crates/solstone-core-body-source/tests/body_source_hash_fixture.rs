// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use solstone_core_body_source::{BodySourceFamily, BodySourceHash, BodyString};

mod support;

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect())
        .expect("ASCII text is a valid body string")
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn family_from_source_type(source_type: &str) -> BodySourceFamily {
    BodySourceFamily::from_bytes(source_type.as_bytes()).expect("fixture source type is valid")
}

fn order_keys(values: &[BodySourceHash]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|value| {
            (
                value.family().as_str().to_owned(),
                value.as_str().to_owned(),
            )
        })
        .collect()
}

#[test]
fn fixture_source_hashes_round_trip_are_family_bound_and_order_like_wire_bytes() {
    let source_hashes = support::native_bundle_source_hashes();
    assert_eq!(source_hashes.len(), 4);

    let mut parsed = Vec::new();
    let mut hashes = HashSet::new();
    for (source_type, source_hash) in &source_hashes {
        let family = family_from_source_type(source_type);
        let from_bytes = BodySourceHash::from_bytes_for_family(source_hash.as_bytes(), &family)
            .expect("fixture source hash is valid");
        let wire_body_string = body_string(source_hash);
        let from_body_string =
            BodySourceHash::from_body_string_for_family(&wire_body_string, &family)
                .expect("fixture body string is valid");
        assert_eq!(from_bytes, from_body_string);
        assert_eq!(from_bytes.family(), family);
        assert_eq!(from_bytes.as_str(), source_hash);
        assert_eq!(
            BodySourceHash::from_body_string_for_family(&from_bytes.to_body_string(), &family)
                .expect("emitted body string is valid"),
            from_bytes
        );
        assert_eq!(hash_of(&from_bytes), hash_of(&from_body_string));
        hashes.insert(from_bytes.clone());
        hashes.insert(from_body_string.clone());
        parsed.push(from_bytes);
        parsed.push(from_body_string);
    }
    assert_eq!(hashes.len(), source_hashes.len());

    let mut expected = order_keys(&parsed);
    expected.sort_by(|left, right| {
        left.0
            .as_bytes()
            .cmp(right.0.as_bytes())
            .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
    });
    parsed.sort();
    assert_eq!(order_keys(&parsed), expected);
}

#[test]
fn apple_window_spellings_remain_distinct_and_order_by_exact_suffix_bytes() {
    let family = BodySourceFamily::AppleHealth;
    let base = "e".repeat(64);
    let spellings = [
        base.clone(),
        format!("{base}#window:open:20260102"),
        format!("{base}#window:20260101:open"),
        format!("{base}#window:20260101:20260102"),
        format!("{base}#window:20260102:20260102"),
    ];
    let mut values: Vec<BodySourceHash> = spellings
        .iter()
        .map(|spelling| {
            BodySourceHash::from_bytes_for_family(spelling.as_bytes(), &family)
                .expect("hand-authored Apple spelling is valid")
        })
        .collect();

    for (index, value) in values.iter().enumerate() {
        for other in &values[index + 1..] {
            assert_ne!(value, other);
        }
    }
    assert_eq!(
        values.iter().cloned().collect::<HashSet<_>>().len(),
        values.len()
    );

    let mut expected = order_keys(&values);
    expected.sort_by(|left, right| {
        left.0
            .as_bytes()
            .cmp(right.0.as_bytes())
            .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
    });
    values.sort();
    assert_eq!(order_keys(&values), expected);
}

#[test]
fn identical_plain_spellings_are_distinct_between_source_families() {
    let spelling = "f".repeat(64);
    let apple =
        BodySourceHash::from_bytes_for_family(spelling.as_bytes(), &BodySourceFamily::AppleHealth)
            .expect("Apple plain spelling is valid");
    let oura =
        BodySourceHash::from_bytes_for_family(spelling.as_bytes(), &BodySourceFamily::OuraApi)
            .expect("Oura plain spelling is valid");

    assert_ne!(apple, oura);
    assert_eq!(
        HashSet::from([apple.clone(), oura.clone()]).len(),
        2,
        "family binding participates in equality and hashing"
    );
    assert_eq!(apple.as_str(), oura.as_str());
    assert_ne!(apple.family(), oura.family());
}
