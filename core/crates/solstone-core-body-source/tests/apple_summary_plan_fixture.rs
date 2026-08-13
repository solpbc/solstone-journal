// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{AppleSummaryPlan, BodyDay, BundleId};

use crate::support;

use support::{envelope_multimonth_fixture, native_bundle_fixture};

fn bundle_from_case(case: &Value) -> BundleId {
    BundleId::from_bytes(
        case["directory"]
            .as_str()
            .expect("case directory")
            .as_bytes(),
    )
    .expect("fixture directory is a valid bundle ID")
}

fn days_from_fixture(days: &Value) -> Vec<BodyDay> {
    days.as_array()
        .expect("summary plan days")
        .iter()
        .map(|day| {
            BodyDay::from_bytes(day.as_str().expect("summary plan day").as_bytes())
                .expect("fixture summary plan day is valid")
        })
        .collect()
}

fn assert_plan_matches_fixture(plan: &AppleSummaryPlan, expected: &Value) {
    assert_eq!(
        plan.schema(),
        expected["schema"].as_str().expect("plan schema")
    );
    let expected_days = expected["days"].as_array().expect("summary plan days");
    assert_eq!(plan.days().len(), expected_days.len());
    for (actual, expected) in plan.days().iter().zip(expected_days) {
        assert_eq!(
            actual.as_str(),
            expected.as_str().expect("summary plan day")
        );
    }
}

#[test]
fn apple_summary_plan_fixture_matches_native_bundle_summary_plans() {
    let fixture = native_bundle_fixture();
    let mut plans = 0;

    for case in fixture["cases"].as_array().expect("fixture cases") {
        let name = case["name"].as_str().expect("case name");
        let envelope: Value = serde_json::from_str(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("expected envelope JSONL"),
        )
        .expect("expected envelope JSONL parses");
        let expected = &envelope["summary_plan"];

        match name {
            "apple_retain_complete_one_row" | "apple_discard_zero_rows" => {
                let plan = AppleSummaryPlan::new(
                    &bundle_from_case(case),
                    days_from_fixture(&expected["days"]),
                )
                .unwrap_or_else(|error| panic!("{name} summary plan should bind: {error}"));
                assert_plan_matches_fixture(&plan, expected);
                plans += 1;
            }
            "oura_retain_parsed_one_row" | "oura_discard_zero_rows" => {
                assert!(expected.is_null(), "{name} has no summary plan");
            }
            _ => panic!("unexpected fixture case {name}"),
        }
    }

    assert_eq!(plans, 2);
}

#[test]
fn apple_summary_plan_fixture_matches_multimonth_summary_plan() {
    let fixture = envelope_multimonth_fixture();
    let case = &fixture["cases"][0];
    let expected = &case["expected_envelope"]["summary_plan"];
    let plan = AppleSummaryPlan::new(
        &bundle_from_case(case),
        days_from_fixture(&expected["days"]),
    )
    .expect("multimonth summary plan should bind");

    assert_eq!(
        plan.days().iter().map(BodyDay::as_str).collect::<Vec<_>>(),
        ["20260102", "20260103", "20260201"]
    );
    assert_plan_matches_fixture(&plan, expected);
}
