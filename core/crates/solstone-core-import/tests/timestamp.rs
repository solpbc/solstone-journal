// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_import::{TimestampError, validate_timestamp};

#[test]
fn ac11_shape_and_calendar_refusals_remain_distinct() {
    for value in ["20260311_1200000", "2026031_120000"] {
        assert_eq!(validate_timestamp(value), Err(TimestampError::Shape));
    }
    for value in ["00000000_000000", "20260230_120000", "20261345_120000"] {
        assert!(matches!(
            validate_timestamp(value),
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
