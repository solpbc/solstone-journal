// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{
    AppleSummaryPlan, BodyDay, BundleId, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField,
};

const BUNDLE: &str = "body-00000000000000000000000000";

fn bundle() -> BundleId {
    BundleId::from_bytes(BUNDLE.as_bytes()).expect("test bundle is valid")
}

fn day(value: &[u8]) -> BodyDay {
    BodyDay::from_bytes(value).expect("test day is valid")
}

fn assert_error(error: &EnvelopeError, bundle: &BundleId) {
    assert_eq!(error.bundle(), Some(bundle));
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::SummaryDays);
    assert_eq!(error.index(), None);

    let display = error.to_string();
    assert!(display.contains(bundle.as_str()));
    assert!(display.contains(EnvelopeErrorField::SummaryDays.as_str()));
    assert!(display.len() <= 122);
    assert_eq!(format!("{error:?}"), display);
    assert!(Error::source(error).is_none());
}

#[test]
fn apple_summary_plan_rejects_reverse_days() {
    let bundle = bundle();
    let error = AppleSummaryPlan::new(&bundle, vec![day(b"20260103"), day(b"20260102")])
        .expect_err("reverse days must refuse");
    assert_error(&error, &bundle);
}

#[test]
fn apple_summary_plan_rejects_adjacent_duplicate_days() {
    let bundle = bundle();
    let error = AppleSummaryPlan::new(&bundle, vec![day(b"20260102"), day(b"20260102")])
        .expect_err("adjacent duplicate days must refuse");
    assert_error(&error, &bundle);
}

#[test]
fn apple_summary_plan_rejects_separated_duplicate_days() {
    let bundle = bundle();
    let error = AppleSummaryPlan::new(
        &bundle,
        vec![day(b"20260102"), day(b"20260103"), day(b"20260102")],
    )
    .expect_err("separated duplicate days must refuse");
    assert_error(&error, &bundle);
}

#[test]
fn apple_summary_plan_rejects_simultaneous_reverse_and_duplicate_days() {
    let bundle = bundle();
    let error = AppleSummaryPlan::new(
        &bundle,
        vec![day(b"20260103"), day(b"20260103"), day(b"20260102")],
    )
    .expect_err("unordered duplicate days must refuse");
    assert_error(&error, &bundle);
}
