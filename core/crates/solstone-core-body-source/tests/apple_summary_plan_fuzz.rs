// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{AppleSummaryPlan, BodyDay, BundleId};

const BUNDLE: &str = "body-00000000000000000000000000";
const APPLE_SUMMARY_SCHEMA: &str = "solstone.body.apple_day_summaries.v1";

fn bundle() -> BundleId {
    BundleId::from_bytes(BUNDLE.as_bytes()).expect("test bundle is valid")
}

fn day(value: &[u8]) -> BodyDay {
    BodyDay::from_bytes(value).expect("boundary day is valid")
}

#[test]
fn apple_summary_plan_accepts_empty_and_calendar_boundary_day_sequences() {
    let cases = [
        vec![],
        vec![day(b"00010101")],
        vec![day(b"99991231")],
        vec![day(b"20240229")],
        vec![day(b"00010101"), day(b"20240229"), day(b"99991231")],
    ];

    for days in cases {
        let expected_days = days.clone();
        let plan = AppleSummaryPlan::new(&bundle(), days).expect("ordered days must bind");
        assert_eq!(plan.schema(), APPLE_SUMMARY_SCHEMA);
        assert_eq!(plan.days(), expected_days.as_slice());
    }
}
