// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyEnvelope, BodyLedgerEvent, BodyRawRetention, BodySourceFamily,
    BodySourceHash, BundleId, Coordinate, EnvelopeLedger, EnvelopeShard, LedgerEventErrorCode,
    LedgerEventErrorField, PresentationRow, encode_body_ledger_event, parse, project,
};

const BUNDLE: &str = "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z";
const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VALUE_HASH: &str = "sha256:f3d64f3c75d8c78ebe82d09f697c4c050c2002d4ea1bb1a945a4e5ac1cb64297";

#[test]
fn public_encoder_is_pure_and_returns_the_canonical_frame() {
    let event = event("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/raw/oura/item");
    let untouched = event.clone();
    let before = (
        event.sequence(),
        event.line(),
        event.raw_ref().unwrap().code_points().to_vec(),
    );
    let encoded = encode_body_ledger_event(&event).unwrap();
    assert_eq!(
        encoded,
        b"{\"bundle_id\":\"body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z\",\"day\":\"20260102\",\"dedupe_key\":\"sha256:cf5b6fc199a3bcbc4d9361346d957f9098c356fe75f226803d2bd57580d95258\",\"end_time\":\"2026-01-03\",\"line\":1,\"normalized_ref\":\"imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/normalized/2026-01.jsonl#L1\",\"raw_ref\":\"imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/raw/oura/item\",\"record_type\":\"oura.daily_readiness\",\"row_schema\":\"solstone.health.oura.v1\",\"row_sha256\":\"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"schema\":\"solstone.body.ledger_event.v1\",\"sequence\":1,\"shard\":\"normalized/2026-01.jsonl\",\"source_family\":\"oura_api\",\"source_record_id\":\"synthetic-readiness-1\",\"start_time\":\"2026-01-02\",\"value_hash\":\"sha256:f3d64f3c75d8c78ebe82d09f697c4c050c2002d4ea1bb1a945a4e5ac1cb64297\"}\n"
    );
    assert_eq!(
        (
            event.sequence(),
            event.line(),
            event.raw_ref().unwrap().code_points().to_vec(),
        ),
        before
    );
    assert_eq!(event, untouched);
}

#[test]
fn public_encoder_exposes_structured_overflow() {
    let event = event(&format!("imports/{BUNDLE}/raw/oura/{}", "a".repeat(65_536)));
    let error = encode_body_ledger_event(&event).expect_err("oversized event refuses");
    assert_eq!(error.bundle(), Some(event.bundle_id()));
    assert_eq!(error.code(), LedgerEventErrorCode::InputTooLarge);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), event.sequence());
}

fn event(raw_ref: &str) -> BodyLedgerEvent {
    let bundle = BundleId::from_bytes(BUNDLE.as_bytes()).unwrap();
    let day = BodyDay::from_bytes(b"20260102").unwrap();
    let digest = BodyDigest::from_bytes(DIGEST.as_bytes()).unwrap();
    let envelope = BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::OuraApi,
        BodySourceHash::from_bytes_for_family(
            b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &BodySourceFamily::OuraApi,
        )
        .unwrap(),
        BodyRawRetention::RetainParsed,
        1,
        vec![day.clone()],
        vec![EnvelopeShard::new(&bundle, 0, day.month(), 1, 1, digest.clone()).unwrap()],
        EnvelopeLedger::new(&bundle, 1, 1, digest.clone()).unwrap(),
        None,
    )
    .unwrap();
    let row = format!(
        "{{\"day\":\"20260102\",\"dedupe_key\":\"sha256:cf5b6fc199a3bcbc4d9361346d957f9098c356fe75f226803d2bd57580d95258\",\"end_date\":\"2026-01-03\",\"import_id\":\"{BUNDLE}\",\"month\":\"2026-01\",\"normalized_ref\":\"imports/{BUNDLE}/normalized/2026-01.jsonl#L1\",\"raw_ref\":\"{raw_ref}\",\"record_type\":\"oura.daily_readiness\",\"schema\":\"solstone.health.oura.v1\",\"source_family\":\"oura_api\",\"source_record_id\":\"synthetic-readiness-1\",\"start_date\":\"2026-01-02\"}}"
    );
    let value = parse(row.as_bytes()).unwrap();
    let coordinate = Coordinate::new(BUNDLE, "normalized/2026-01.jsonl", 1);
    let presentation = PresentationRow::new(&value, &coordinate).unwrap();
    let candidate = project(&presentation, coordinate).unwrap();
    BodyLedgerEvent::new(
        &envelope,
        1,
        0,
        1,
        digest,
        BodyDigest::from_bytes(VALUE_HASH.as_bytes()).unwrap(),
        &candidate,
    )
    .unwrap()
}
