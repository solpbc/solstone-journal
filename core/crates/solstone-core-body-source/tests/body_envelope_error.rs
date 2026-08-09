// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    AppleSummaryPlan, BodyDay, BodyDigest, BodyEnvelope, BodyMonth, BodyRawRetention,
    BodySourceFamily, BodySourceHash, BundleId, EnvelopeError, EnvelopeErrorCode,
    EnvelopeErrorField, EnvelopeLedger, EnvelopeShard,
};

const BUNDLE: &str = "body-00000000000000000000000000";
const NONEMPTY_SHA256: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EMPTY_CONTENT_SHA256: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Inputs {
    bundle_id: BundleId,
    source_family: BodySourceFamily,
    source_hash: BodySourceHash,
    raw_retention: BodyRawRetention,
    row_count: u64,
    days: Vec<BodyDay>,
    shards: Vec<EnvelopeShard>,
    ledger: EnvelopeLedger,
    summary_plan: Option<AppleSummaryPlan>,
}

fn bundle() -> BundleId {
    BundleId::from_bytes(BUNDLE.as_bytes()).expect("test bundle is valid")
}

fn day(value: &[u8]) -> BodyDay {
    BodyDay::from_bytes(value).expect("test day is valid")
}

fn month(value: &[u8]) -> BodyMonth {
    BodyMonth::from_bytes(value).expect("test month is valid")
}

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("test digest is valid")
}

fn hash(family: BodySourceFamily, suffix: &str) -> BodySourceHash {
    BodySourceHash::from_bytes_for_family(format!("{HASH}{suffix}").as_bytes(), &family)
        .expect("test hash is valid")
}

fn shard(bundle: &BundleId, month_bytes: &[u8], rows: u64) -> EnvelopeShard {
    EnvelopeShard::new(
        bundle,
        0,
        month(month_bytes),
        rows,
        rows,
        digest(NONEMPTY_SHA256),
    )
    .expect("test shard is valid")
}

fn ledger(bundle: &BundleId, events: u64) -> EnvelopeLedger {
    let sha256 = if events == 0 {
        EMPTY_CONTENT_SHA256
    } else {
        NONEMPTY_SHA256
    };
    EnvelopeLedger::new(bundle, events, events, digest(sha256)).expect("test ledger is valid")
}

fn plan(bundle: &BundleId, days: Vec<BodyDay>) -> AppleSummaryPlan {
    AppleSummaryPlan::new(bundle, days).expect("test plan is valid")
}

fn apple_inputs() -> Inputs {
    let bundle_id = bundle();
    let days = vec![day(b"20260102")];
    Inputs {
        source_family: BodySourceFamily::AppleHealth,
        source_hash: hash(BodySourceFamily::AppleHealth, ""),
        raw_retention: BodyRawRetention::Discard,
        row_count: 1,
        shards: vec![shard(&bundle_id, b"2026-01", 1)],
        ledger: ledger(&bundle_id, 1),
        summary_plan: Some(plan(&bundle_id, days.clone())),
        bundle_id,
        days,
    }
}

fn bind(inputs: Inputs) -> Result<BodyEnvelope, EnvelopeError> {
    BodyEnvelope::new(
        inputs.bundle_id,
        inputs.source_family,
        inputs.source_hash,
        inputs.raw_retention,
        inputs.row_count,
        inputs.days,
        inputs.shards,
        inputs.ledger,
        inputs.summary_plan,
    )
}

fn assert_failure(
    inputs: Inputs,
    code: EnvelopeErrorCode,
    field: EnvelopeErrorField,
    index: Option<u64>,
) {
    let bundle = inputs.bundle_id.clone();
    let error = bind(inputs).expect_err("invalid envelope must refuse");
    assert_eq!(error.bundle(), Some(&bundle));
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
    assert_eq!(error.index(), index);
}

#[test]
fn body_envelope_rejects_source_hash_family_mismatch() {
    let mut inputs = apple_inputs();
    inputs.source_family = BodySourceFamily::OuraApi;
    assert_failure(
        inputs,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::SourceHash,
        None,
    );
}

#[test]
fn body_envelope_rejects_incompatible_raw_retention() {
    let mut inputs = apple_inputs();
    inputs.source_family = BodySourceFamily::OuraApi;
    inputs.source_hash = hash(BodySourceFamily::OuraApi, "");
    inputs.raw_retention = BodyRawRetention::RetainComplete;
    assert_failure(
        inputs,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::RawRetention,
        None,
    );
}

#[test]
fn body_envelope_rejects_unordered_days_at_the_later_index() {
    let mut inputs = apple_inputs();
    inputs.days = vec![day(b"20260103"), day(b"20260102")];
    assert_failure(
        inputs,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::Days,
        Some(1),
    );
}

#[test]
fn body_envelope_rejects_days_outside_the_source_hash_window() {
    let mut inputs = apple_inputs();
    inputs.source_hash = hash(BodySourceFamily::AppleHealth, "#window:20260103:20260103");
    assert_failure(
        inputs,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::Days,
        Some(0),
    );
}

#[test]
fn body_envelope_rejects_day_count_mismatch() {
    let mut inputs = apple_inputs();
    inputs.row_count = 0;
    assert_failure(
        inputs,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::Days,
        None,
    );
}

#[test]
fn body_envelope_rejects_unordered_shards_at_the_later_index() {
    let mut inputs = apple_inputs();
    let bundle = inputs.bundle_id.clone();
    let days = vec![day(b"20260102"), day(b"20260201")];
    inputs.row_count = 2;
    inputs.days = days.clone();
    inputs.shards = vec![shard(&bundle, b"2026-02", 1), shard(&bundle, b"2026-01", 1)];
    inputs.ledger = ledger(&bundle, 2);
    inputs.summary_plan = Some(plan(&bundle, days));
    assert_failure(
        inputs,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::Shards,
        Some(1),
    );
}

#[test]
fn body_envelope_rejects_shard_presence_mismatch() {
    let mut inputs = apple_inputs();
    inputs.shards = vec![];
    assert_failure(
        inputs,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::Shards,
        None,
    );
}

#[test]
fn body_envelope_rejects_overflowing_shard_row_totals_at_the_overflowing_index() {
    let bundle_id = bundle();
    let days = vec![day(b"20260102"), day(b"20260201")];
    let inputs = Inputs {
        source_family: BodySourceFamily::AppleHealth,
        source_hash: hash(BodySourceFamily::AppleHealth, ""),
        raw_retention: BodyRawRetention::Discard,
        row_count: u64::MAX,
        shards: vec![
            shard(&bundle_id, b"2026-01", u64::MAX),
            shard(&bundle_id, b"2026-02", u64::MAX),
        ],
        ledger: ledger(&bundle_id, u64::MAX),
        summary_plan: Some(plan(&bundle_id, days.clone())),
        bundle_id,
        days,
    };
    assert_failure(
        inputs,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::ShardRows,
        Some(1),
    );
}

#[test]
fn body_envelope_rejects_shard_row_count_mismatch() {
    let mut inputs = apple_inputs();
    let bundle = inputs.bundle_id.clone();
    inputs.row_count = 2;
    inputs.ledger = ledger(&bundle, 2);
    assert_failure(
        inputs,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::ShardRows,
        None,
    );
}

#[test]
fn body_envelope_rejects_mismatched_day_and_shard_month_sets() {
    let mut inputs = apple_inputs();
    let bundle = inputs.bundle_id.clone();
    inputs.shards = vec![shard(&bundle, b"2026-02", 1)];
    assert_failure(
        inputs,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::Shards,
        None,
    );
}

#[test]
fn body_envelope_rejects_ledger_event_count_mismatch() {
    let mut inputs = apple_inputs();
    let bundle = inputs.bundle_id.clone();
    inputs.ledger = ledger(&bundle, 0);
    assert_failure(
        inputs,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::LedgerEvents,
        None,
    );
}

#[test]
fn body_envelope_requires_an_apple_summary_plan() {
    let mut inputs = apple_inputs();
    inputs.summary_plan = None;
    assert_failure(
        inputs,
        EnvelopeErrorCode::MissingField,
        EnvelopeErrorField::SummaryPlan,
        None,
    );
}

#[test]
fn body_envelope_rejects_an_oura_summary_plan() {
    let mut inputs = apple_inputs();
    inputs.source_family = BodySourceFamily::OuraApi;
    inputs.source_hash = hash(BodySourceFamily::OuraApi, "");
    assert_failure(
        inputs,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::SummaryPlan,
        None,
    );
}

#[test]
fn body_envelope_rejects_summary_plan_days_that_differ_from_envelope_days() {
    let mut inputs = apple_inputs();
    let bundle = inputs.bundle_id.clone();
    inputs.summary_plan = Some(plan(&bundle, vec![day(b"20260103")]));
    assert_failure(
        inputs,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::SummaryDays,
        None,
    );
}
