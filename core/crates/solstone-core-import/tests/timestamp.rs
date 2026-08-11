// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_import::{TimestampError, validate_timestamp};

const CORPUS: &str = include_str!("../../../fixtures/import_resolver_corpus.json");

fn corpus_timestamp(row: &str) -> String {
    row.split_once("::")
        .unwrap()
        .0
        .trim_start_matches("timestamp=")
        .to_owned()
}

fn corpus_rows() -> Value {
    serde_json::from_str::<Value>(CORPUS).unwrap()["passes"]["native_detector_answers_no"].clone()
}

#[test]
fn ac11_shape_and_calendar_refusals_remain_distinct() {
    let rows = corpus_rows();
    for row in [
        "timestamp=20260311_1200000::audio.m4a",
        "timestamp=2026031_120000::audio.m4a",
    ] {
        assert_eq!(
            validate_timestamp(&corpus_timestamp(row)),
            Err(TimestampError::Shape)
        );
    }
    for row in [
        "timestamp=00000000_000000::audio.m4a",
        "timestamp=20260230_120000::audio.m4a",
        "timestamp=20261345_996060::audio.m4a",
    ] {
        assert!(rows[row]["raised"]["message"].is_string());
        assert!(matches!(
            validate_timestamp(&corpus_timestamp(row)),
            Err(TimestampError::Calendar { .. })
        ));
    }
}

#[test]
fn ac11b_unicode_digits_reach_calendar_validation() {
    // constructed Unicode-digit timestamp; Python `\d` shape class is Unicode-aware.
    let error = validate_timestamp("٢٠٢٦٠٣١١_١٢٠٠٠٠").unwrap_err();
    assert!(matches!(error, TimestampError::Calendar { .. }));
    assert!(error.to_string().starts_with("time data '"));
}
