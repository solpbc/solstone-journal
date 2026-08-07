// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::error::Error;

use solstone_core_body_source::{BodyString, BodyValue, CanonicalizeError, canonicalize};

fn key() -> BodyString {
    BodyString::from_code_points(vec![u32::from(b'a')]).unwrap()
}

fn nested_arrays(depth: usize) -> BodyValue {
    (0..depth).fold(BodyValue::Null, |value, _| BodyValue::Array(vec![value]))
}

fn nested_objects(depth: usize) -> BodyValue {
    (0..depth).fold(BodyValue::Null, |value, _| {
        BodyValue::Object(BTreeMap::from([(key(), value)]))
    })
}

fn alternating_containers(depth: usize) -> BodyValue {
    (0..depth).fold(BodyValue::Null, |value, index| {
        if index % 2 == 0 {
            BodyValue::Array(vec![value])
        } else {
            BodyValue::Object(BTreeMap::from([(key(), value)]))
        }
    })
}

#[test]
fn canonicalization_enforces_the_parser_nesting_limit() {
    for build in [
        nested_arrays as fn(usize) -> BodyValue,
        nested_objects,
        alternating_containers,
    ] {
        canonicalize(&build(128)).expect("128 containers should canonicalize");

        let too_deep = build(129);
        let error = canonicalize(&too_deep).expect_err("129 containers should fail");
        assert_eq!(error, CanonicalizeError::ValueTooDeep { depth: 129 });
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(display.len() <= 256 && debug.len() <= 256);
        assert!(!display.contains("null") && !debug.contains("null"));
        assert!(!display.contains(['[', ']', '{', '}']));
        assert!(!debug.contains(['[', ']']));
        assert!(Error::source(&error).is_none());
        drop(too_deep);
    }
}
