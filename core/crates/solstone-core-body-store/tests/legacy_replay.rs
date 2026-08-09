// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_body_source::{
    BodyMonth, Coordinate, LedgerCandidate, PresentationRow, parse, project,
};
use solstone_core_body_store::{
    BodyDedupeDisposition, BodyDedupeState, LegacyBodyRowErrorField, LegacyBodyRowErrorKind,
    validate_legacy_body_row,
};

mod support;

use support::fixture_observation;

const LEGACY_KEY: &str = "apple-health:synthetic:legacy-1";
const OURA_KEY: &str = "sha256:cf5b6fc199a3bcbc4d9361346d957f9098c356fe75f226803d2bd57580d95258";
const OURA_VALUE_HASH: &str =
    "sha256:f3d64f3c75d8c78ebe82d09f697c4c050c2002d4ea1bb1a945a4e5ac1cb64297";
const OURA_BUNDLE: &str = "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z";

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../core/fixtures")
        .join(name)
}

fn fixture(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_path(name)).expect("fixture should read"))
        .expect("fixture should parse")
}

fn candidate_from_bytes(bytes: &[u8], line: u64) -> LedgerCandidate {
    let value = parse(bytes).expect("fixture row parses through F1");
    let coordinate = Coordinate::new("legacy-import", "normalized/2026-01.jsonl", line);
    let row = PresentationRow::new(&value, &coordinate).expect("fixture row presents");
    project(&row, coordinate).expect("fixture row projects")
}

fn codec_candidate(name: &str) -> LedgerCandidate {
    let fixture = fixture("body_source_codec_rows.json");
    let case = fixture["rows"]
        .as_array()
        .expect("codec rows")
        .iter()
        .find(|case| case["name"].as_str() == Some(name))
        .expect("named codec row");
    let bytes = serde_json::to_vec(&case["row"]).expect("codec row serializes");
    candidate_from_bytes(&bytes, 1)
}

fn native_candidate(name: &str) -> LedgerCandidate {
    let fixture = fixture("body_source_native_bundle_v1.json");
    let case = fixture["cases"]
        .as_array()
        .expect("native cases")
        .iter()
        .find(|case| case["name"].as_str() == Some(name))
        .expect("named native case");
    candidate_from_bytes(
        case["expected_normalized_jsonl"]
            .as_str()
            .expect("normalized row")
            .as_bytes(),
        1,
    )
}

fn month(value: &str) -> BodyMonth {
    BodyMonth::from_bytes(value.as_bytes()).expect("fixture month is valid")
}

#[test]
fn legacy_schema_preserves_arbitrary_key_and_nullable_hash() {
    let checked = validate_legacy_body_row(
        &codec_candidate("legacy_normalized_v1"),
        "legacy-synthetic-import",
        &month("2026-01"),
        1,
    )
    .expect("legacy row validates");
    let row = checked.row();

    assert_eq!(row.dedupe_key(), LEGACY_KEY);
    assert_eq!(row.source_family().as_str(), "apple_health");
    assert_eq!(row.record_type(), "HKQuantityTypeIdentifierHeartRate");
    assert_eq!(row.start_time(), "2026-01-01T08:00:00-07:00");
    assert_eq!(row.end_time(), None);
    assert_eq!(row.value_hash(), None);
    assert_eq!(row.first_import_id(), Some("legacy-synthetic-import"));
    assert_eq!(row.latest_import_id(), Some("legacy-synthetic-import"));
    assert_eq!(
        row.normalized_ref(),
        Some("imports/legacy-synthetic-import/normalized/2026-01.jsonl#L1")
    );
    assert_eq!(row.raw_ref(), None);

    let mut state = BodyDedupeState::new();
    assert_eq!(
        state.apply_legacy(&checked),
        BodyDedupeDisposition::Inserted
    );
    assert_eq!(state.get_by_key(LEGACY_KEY), Some(row));
}

#[test]
fn apple_hash_stays_absent_while_oura_hash_is_reconstructed() {
    let apple = validate_legacy_body_row(
        &native_candidate("apple_retain_complete_one_row"),
        "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y",
        &month("2026-01"),
        1,
    )
    .expect("Apple row validates");
    assert_eq!(apple.row().value_hash(), None);

    let oura = validate_legacy_body_row(
        &native_candidate("oura_retain_parsed_one_row"),
        OURA_BUNDLE,
        &month("2026-01"),
        1,
    )
    .expect("Oura row validates");
    assert_eq!(
        oura.row().value_hash().map(|digest| digest.as_str()),
        Some(OURA_VALUE_HASH)
    );
}

#[test]
fn legacy_then_native_replay_matches_shipping_upsert_coalescing() {
    let legacy = validate_legacy_body_row(
        &codec_candidate("oura_v1_all_shapes"),
        "synthetic-import",
        &month("2026-01"),
        1,
    )
    .expect("pre-native Oura row validates");
    let native_observation = fixture_observation("oura_retain_parsed_one_row");
    let native = native_observation.validate();
    let mut state = BodyDedupeState::new();

    assert_eq!(state.apply_legacy(&legacy), BodyDedupeDisposition::Inserted);
    assert_eq!(state.apply(&native), Ok(BodyDedupeDisposition::Updated));

    let row = state.get_by_key(OURA_KEY).expect("Oura row exists");
    assert_eq!(row.first_import_id(), Some("synthetic-import"));
    assert_eq!(row.latest_import_id(), Some(OURA_BUNDLE));
    assert_eq!(
        row.value_hash().map(|digest| digest.as_str()),
        Some(OURA_VALUE_HASH)
    );
    assert_eq!(
        row.normalized_ref(),
        Some("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/normalized/2026-01.jsonl#L1")
    );
}

#[test]
fn missing_embedded_provenance_is_derived_from_physical_location() {
    let candidate = candidate_from_bytes(
        br#"{"schema":"solstone.health.normalized.v1","source_family":"apple_health","record_type":"synthetic","dedupe_key":"legacy-key","start_date":"2026-01-01","day":"20260101"}"#,
        1,
    );
    let checked = validate_legacy_body_row(&candidate, "legacy-import", &month("2026-01"), 1)
        .expect("minimal legacy row validates");
    assert_eq!(checked.row().first_import_id(), Some("legacy-import"));
    assert_eq!(checked.row().latest_import_id(), Some("legacy-import"));
    assert_eq!(
        checked.row().normalized_ref(),
        Some("imports/legacy-import/normalized/2026-01.jsonl#L1")
    );
    assert_eq!(checked.row().raw_ref(), None);
}

#[test]
fn sqlite_unrepresentable_text_is_refused_without_value_leakage() {
    for (field, expected) in [
        ("dedupe_key", LegacyBodyRowErrorField::DedupeKey),
        ("import_id", LegacyBodyRowErrorField::ImportId),
        ("normalized_ref", LegacyBodyRowErrorField::NormalizedRef),
        ("raw_ref", LegacyBodyRowErrorField::RawRef),
    ] {
        let json = format!(
            "{{\"schema\":\"solstone.health.normalized.v1\",\"source_family\":\"apple_health\",\"record_type\":\"synthetic\",\"dedupe_key\":\"legacy-key\",\"start_date\":\"2026-01-01\",\"day\":\"20260101\",\"{field}\":\"\\ud800-secret\"}}"
        );
        let candidate = candidate_from_bytes(json.as_bytes(), 1);
        let error = validate_legacy_body_row(&candidate, "legacy-import", &month("2026-01"), 1)
            .expect_err("surrogate is refused");
        assert_eq!(error.kind(), LegacyBodyRowErrorKind::InvalidText);
        assert_eq!(error.field(), expected);
        assert_eq!(
            error.to_string(),
            format!("legacy-body-row invalid_text: {}", expected.as_str())
        );
        assert_eq!(format!("{error:?}"), error.to_string());
        assert!(!error.to_string().contains("secret"));
        assert!(error.to_string().len() <= 256);
    }
}

#[test]
fn embedded_provenance_must_match_the_physical_row() {
    let candidate = codec_candidate("legacy_normalized_v1");
    for (import_id, month_value, line, field) in [
        (
            "wrong-import",
            "2026-01",
            1,
            LegacyBodyRowErrorField::ImportId,
        ),
        (
            "legacy-synthetic-import",
            "2026-02",
            1,
            LegacyBodyRowErrorField::Month,
        ),
        (
            "legacy-synthetic-import",
            "2026-01",
            2,
            LegacyBodyRowErrorField::NormalizedRef,
        ),
    ] {
        let error = validate_legacy_body_row(&candidate, import_id, &month(month_value), line)
            .expect_err("mismatched physical provenance is refused");
        assert_eq!(error.kind(), LegacyBodyRowErrorKind::ReferenceMismatch);
        assert_eq!(error.field(), field);
    }

    let invalid = validate_legacy_body_row(
        &candidate,
        "../legacy-synthetic-import",
        &month("2026-01"),
        1,
    )
    .expect_err("invalid physical import id is refused");
    assert_eq!(invalid.kind(), LegacyBodyRowErrorKind::InvalidText);
    assert_eq!(invalid.field(), LegacyBodyRowErrorField::ImportId);
}

#[test]
fn raw_reference_must_stay_inside_the_physical_import() {
    for raw_ref in [
        "imports/other-import/raw/export.xml#record-1",
        "../raw/export.xml#record-1",
        "raw/../export.xml#record-1",
        "/tmp/export.xml#record-1",
    ] {
        let json = format!(
            "{{\"schema\":\"solstone.health.normalized.v1\",\"source_family\":\"apple_health\",\"record_type\":\"synthetic\",\"dedupe_key\":\"legacy-key\",\"start_date\":\"2026-01-01\",\"day\":\"20260101\",\"raw_ref\":{}}}",
            serde_json::to_string(raw_ref).expect("raw ref serializes")
        );
        let candidate = candidate_from_bytes(json.as_bytes(), 1);
        let error = validate_legacy_body_row(&candidate, "legacy-import", &month("2026-01"), 1)
            .expect_err("out-of-import raw reference is refused");
        assert_eq!(error.kind(), LegacyBodyRowErrorKind::ReferenceMismatch);
        assert_eq!(error.field(), LegacyBodyRowErrorField::RawRef);
    }

    for raw_ref in [
        "imports/legacy-import/raw/apple health/export.xml#record-1",
        "raw/apple health/export.xml#record-1",
    ] {
        let json = format!(
            "{{\"schema\":\"solstone.health.normalized.v1\",\"source_family\":\"apple_health\",\"record_type\":\"synthetic\",\"dedupe_key\":\"legacy-key\",\"start_date\":\"2026-01-01\",\"day\":\"20260101\",\"raw_ref\":{}}}",
            serde_json::to_string(raw_ref).expect("raw ref serializes")
        );
        let candidate = candidate_from_bytes(json.as_bytes(), 1);
        assert!(
            validate_legacy_body_row(&candidate, "legacy-import", &month("2026-01"), 1,).is_ok()
        );
    }
}
