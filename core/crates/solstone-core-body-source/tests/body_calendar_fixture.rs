// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use solstone_core_body_source::{BodyDay, BodyMonth, BodyString};

use crate::support;

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
fn fixture_calendar_values_round_trip_and_order_like_wire_bytes() {
    let days = support::native_bundle_days_affected();
    let months = support::native_bundle_shard_months();
    assert_eq!(days.len(), 1);
    assert_eq!(months.len(), 1);

    let mut parsed_days = Vec::new();
    let mut day_hashes = HashSet::new();
    for day in &days {
        let from_bytes = BodyDay::from_bytes(day.as_bytes()).expect("fixture day is valid");
        let wire_body_string = body_string(day);
        let from_body_string =
            BodyDay::from_body_string(&wire_body_string).expect("fixture body string is valid");
        assert_eq!(from_bytes, from_body_string);
        assert_eq!(BodyDay::try_from(day.as_bytes()).unwrap(), from_bytes);
        assert_eq!(
            BodyDay::try_from(&wire_body_string).unwrap(),
            from_body_string
        );
        assert_eq!(from_bytes.as_str(), day);
        assert_eq!(
            BodyDay::from_body_string(&from_bytes.to_body_string())
                .expect("emitted body string is valid"),
            from_bytes
        );
        assert_eq!(
            from_bytes.month().as_str(),
            format!("{}-{}", &day[..4], &day[4..6])
        );
        assert_eq!(hash_of(&from_bytes), hash_of(&from_body_string));
        day_hashes.insert(from_bytes.clone());
        day_hashes.insert(from_body_string);
        parsed_days.push(from_bytes);
    }
    assert_eq!(day_hashes.len(), parsed_days.len());

    let mut parsed_months = Vec::new();
    let mut month_hashes = HashSet::new();
    for month in &months {
        let from_bytes = BodyMonth::from_bytes(month.as_bytes()).expect("fixture month is valid");
        let wire_body_string = body_string(month);
        let from_body_string =
            BodyMonth::from_body_string(&wire_body_string).expect("fixture body string is valid");
        assert_eq!(from_bytes, from_body_string);
        assert_eq!(BodyMonth::try_from(month.as_bytes()).unwrap(), from_bytes);
        assert_eq!(
            BodyMonth::try_from(&wire_body_string).unwrap(),
            from_body_string
        );
        assert_eq!(from_bytes.as_str(), month);
        assert_eq!(
            BodyMonth::from_body_string(&from_bytes.to_body_string())
                .expect("emitted body string is valid"),
            from_bytes
        );
        assert_eq!(hash_of(&from_bytes), hash_of(&from_body_string));
        month_hashes.insert(from_bytes.clone());
        month_hashes.insert(from_body_string);
        parsed_months.push(from_bytes);
    }
    assert_eq!(month_hashes.len(), parsed_months.len());

    parsed_days.sort();
    let ordered_days: Vec<&str> = parsed_days.iter().map(BodyDay::as_str).collect();
    let mut raw_days: Vec<&str> = days.iter().map(String::as_str).collect();
    raw_days.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    assert_eq!(ordered_days, raw_days);

    parsed_months.sort();
    let ordered_months: Vec<&str> = parsed_months.iter().map(BodyMonth::as_str).collect();
    let mut raw_months: Vec<&str> = months.iter().map(String::as_str).collect();
    raw_months.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    assert_eq!(ordered_months, raw_months);
}
