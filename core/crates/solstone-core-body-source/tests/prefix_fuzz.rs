// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};

use solstone_core_body_source::parse;

use crate::support;

use support::{codec_rows, vectors};

fn assert_prefix_is_safe(input: &[u8]) {
    for length in 1..input.len() {
        let result = catch_unwind(AssertUnwindSafe(|| parse(&input[..length])))
            .unwrap_or_else(|_| panic!("parser panicked for prefix length {length}"));
        if let Err(error) = result {
            let display = error.to_string();
            let debug = format!("{error:?}");
            assert!(display.len() <= 256 && debug.len() <= 256);
            assert!(!display.contains(['[', ']', '{', '}', '"']));
            assert!(!debug.contains("null"));
            assert!(Error::source(&error).is_none());
        }
    }
}

#[test]
fn every_direct_json_prefix_is_panic_free_and_bounded() {
    let fixture = vectors();
    for section in [
        "canonical_cases",
        "string_decode_cases",
        "float_cases",
        "malformed_cases",
    ] {
        for case in fixture[section].as_array().expect("fixture section") {
            assert_prefix_is_safe(case["raw_json"].as_str().expect("raw JSON").as_bytes());
        }
    }
    for row in codec_rows()["rows"].as_array().expect("codec rows") {
        let json = serde_json::to_string(&row["row"]).expect("row should serialize");
        assert_prefix_is_safe(json.as_bytes());
    }
}
