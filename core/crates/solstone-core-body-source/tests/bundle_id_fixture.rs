// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use solstone_core_body_source::{BodyString, BundleId};

mod support;

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect())
        .expect("ASCII text is a valid body string")
}

fn hash_of(value: &BundleId) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn fixture_bundle_ids_round_trip_and_order_like_wire_bytes() {
    let ids = support::native_bundle_import_ids();
    assert_eq!(ids.len(), 4);

    let mut parsed = Vec::new();
    for id in &ids {
        let from_bytes = BundleId::from_bytes(id.as_bytes()).expect("fixture ID is valid");
        let wire_body_string = body_string(id);
        let from_body_string =
            BundleId::from_body_string(&wire_body_string).expect("fixture body string is valid");
        assert_eq!(from_bytes, from_body_string);
        assert_eq!(BundleId::try_from(id.as_bytes()).unwrap(), from_bytes);
        assert_eq!(
            BundleId::try_from(&wire_body_string).unwrap(),
            from_body_string
        );
        assert_eq!(from_bytes.as_str(), id);
        assert_eq!(
            BundleId::from_body_string(&from_bytes.to_body_string())
                .expect("emitted body string is valid"),
            from_bytes
        );
        assert_eq!(hash_of(&from_bytes), hash_of(&from_body_string));
        parsed.push(from_bytes);
    }

    parsed.sort();
    let ordered_ids: Vec<&str> = parsed.iter().map(BundleId::as_str).collect();
    let mut raw_ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    raw_ids.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    assert_eq!(ordered_ids, raw_ids);
}
