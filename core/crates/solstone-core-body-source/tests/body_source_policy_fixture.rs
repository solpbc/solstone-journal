// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use solstone_core_body_source::{BodyRawRetention, BodySourceFamily, BodyString};

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

#[test]
fn fixture_source_policies_round_trip_and_order_like_wire_bytes() {
    let policies = support::native_bundle_source_policies();
    assert_eq!(policies.len(), 4);

    let source_types: BTreeSet<String> = policies
        .iter()
        .map(|(source_type, _)| source_type.clone())
        .collect();
    assert_eq!(
        source_types,
        BTreeSet::from(["apple_health".to_owned(), "oura_api".to_owned()])
    );
    let raw_retentions: BTreeSet<String> = policies
        .iter()
        .map(|(_, raw_retention)| raw_retention.clone())
        .collect();
    assert_eq!(
        raw_retentions,
        BTreeSet::from([
            "discard".to_owned(),
            "retain_complete".to_owned(),
            "retain_parsed".to_owned(),
        ])
    );

    let mut parsed_families = Vec::new();
    for source_type in &source_types {
        let from_bytes =
            BodySourceFamily::from_bytes(source_type.as_bytes()).expect("fixture family is valid");
        let wire_body_string = body_string(source_type);
        let from_body_string = BodySourceFamily::from_body_string(&wire_body_string)
            .expect("fixture body string is valid");
        assert_eq!(from_bytes, from_body_string);
        assert_eq!(
            BodySourceFamily::try_from(source_type.as_bytes()).unwrap(),
            from_bytes
        );
        assert_eq!(
            BodySourceFamily::try_from(&wire_body_string).unwrap(),
            from_body_string
        );
        assert_eq!(from_bytes.as_str(), source_type);
        assert_eq!(
            BodySourceFamily::from_body_string(&from_bytes.to_body_string())
                .expect("emitted body string is valid"),
            from_bytes
        );
        assert_eq!(hash_of(&from_bytes), hash_of(&from_body_string));
        parsed_families.push(from_bytes);
    }

    let mut parsed_retentions = Vec::new();
    for raw_retention in &raw_retentions {
        let from_bytes = BodyRawRetention::from_bytes(raw_retention.as_bytes())
            .expect("fixture retention is valid");
        let wire_body_string = body_string(raw_retention);
        let from_body_string = BodyRawRetention::from_body_string(&wire_body_string)
            .expect("fixture body string is valid");
        assert_eq!(from_bytes, from_body_string);
        assert_eq!(
            BodyRawRetention::try_from(raw_retention.as_bytes()).unwrap(),
            from_bytes
        );
        assert_eq!(
            BodyRawRetention::try_from(&wire_body_string).unwrap(),
            from_body_string
        );
        assert_eq!(from_bytes.as_str(), raw_retention);
        assert_eq!(
            BodyRawRetention::from_body_string(&from_bytes.to_body_string())
                .expect("emitted body string is valid"),
            from_bytes
        );
        assert_eq!(hash_of(&from_bytes), hash_of(&from_body_string));
        parsed_retentions.push(from_bytes);
    }

    parsed_families.sort();
    let ordered_families: Vec<&str> = parsed_families
        .iter()
        .map(BodySourceFamily::as_str)
        .collect();
    let mut raw_families: Vec<&str> = source_types.iter().map(String::as_str).collect();
    raw_families.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    assert_eq!(ordered_families, raw_families);

    parsed_retentions.sort();
    let ordered_retentions: Vec<&str> = parsed_retentions
        .iter()
        .map(BodyRawRetention::as_str)
        .collect();
    let mut raw_retentions: Vec<&str> = raw_retentions.iter().map(String::as_str).collect();
    raw_retentions.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    assert_eq!(ordered_retentions, raw_retentions);

    for (source_type, raw_retention) in policies {
        let family = BodySourceFamily::from_bytes(source_type.as_bytes()).unwrap();
        let retention = BodyRawRetention::from_bytes(raw_retention.as_bytes()).unwrap();
        assert!(retention.check_compatible(&family).is_ok());
    }
}
