// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use solstone_core_body_source::{BodyDigest, BodyString};

use crate::support;

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect())
        .expect("ASCII text is a valid body string")
}

fn hash_of(value: &BodyDigest) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn fixture_digests_round_trip_and_order_like_wire_bytes() {
    let digests = support::native_bundle_digests();
    assert_eq!(digests.len(), 9);

    let mut parsed = Vec::new();
    for digest in &digests {
        let from_bytes =
            BodyDigest::from_bytes(digest.as_bytes()).expect("fixture digest is valid");
        let wire_body_string = body_string(digest);
        let from_body_string =
            BodyDigest::from_body_string(&wire_body_string).expect("fixture body string is valid");
        assert_eq!(from_bytes, from_body_string);
        assert_eq!(BodyDigest::try_from(digest.as_bytes()).unwrap(), from_bytes);
        assert_eq!(
            BodyDigest::try_from(&wire_body_string).unwrap(),
            from_body_string
        );
        assert_eq!(from_bytes.as_str(), digest);
        assert_eq!(
            BodyDigest::from_body_string(&from_bytes.to_body_string())
                .expect("emitted body string is valid"),
            from_bytes
        );
        assert_eq!(hash_of(&from_bytes), hash_of(&from_body_string));
        parsed.push(from_bytes);
    }

    parsed.sort();
    let ordered_digests: Vec<&str> = parsed.iter().map(BodyDigest::as_str).collect();
    let mut raw_digests: Vec<&str> = digests.iter().map(String::as_str).collect();
    raw_digests.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    assert_eq!(ordered_digests, raw_digests);
}
