// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyInteger, BodyManifestBinding, BodyObject, BodyRawRetention,
    BodySourceFamily, BodySourceHash, BodyString, BodyValue, BundleId, ManifestBindingErrorCode,
    ManifestBindingErrorField, parse,
};

use crate::support;

use support::{
    MAX_BUNDLE, MIN_BUNDLE, assert_body_value_bitwise_eq, native_bundle_manifest_binding_cases,
};

const DIGEST: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const APPLE_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OURA_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect()).expect("ASCII body string")
}

fn digest() -> BodyDigest {
    BodyDigest::from_bytes(DIGEST.as_bytes()).expect("test digest is valid")
}

fn bundle(value: &str) -> BundleId {
    BundleId::from_bytes(value.as_bytes()).expect("test bundle is valid")
}

fn day(value: &str) -> BodyDay {
    BodyDay::from_bytes(value.as_bytes()).expect("test day is valid")
}

fn hash(value: &str, family: BodySourceFamily) -> BodySourceHash {
    BodySourceHash::from_bytes_for_family(value.as_bytes(), &family).expect("test hash is valid")
}

fn binding(
    import_id: BundleId,
    source_type: BodySourceFamily,
    source_hash: BodySourceHash,
    entry_count: u64,
    days_affected: Vec<BodyDay>,
    raw_retention: BodyRawRetention,
) -> Result<BodyManifestBinding, solstone_core_body_source::ManifestBindingError> {
    BodyManifestBinding::new(
        digest(),
        import_id,
        source_type,
        source_hash,
        entry_count,
        days_affected,
        raw_retention,
    )
}

fn expected_object(value: &Value) -> BodyObject {
    let encoded = serde_json::to_string(value).expect("expected binding serializes");
    let BodyValue::Object(object) = parse(encoded.as_bytes()).expect("expected binding parses")
    else {
        panic!("expected binding is an object");
    };
    object
}

fn assert_error(
    result: Result<BodyManifestBinding, solstone_core_body_source::ManifestBindingError>,
    code: ManifestBindingErrorCode,
    field: ManifestBindingErrorField,
) {
    let Err(error) = result else {
        panic!("binding should refuse");
    };
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
}

fn assert_binding_values(
    binding: &BodyManifestBinding,
    import_id: &BundleId,
    source_type: BodySourceFamily,
    source_hash: &BodySourceHash,
    entry_count: u64,
    days_affected: &[BodyDay],
    raw_retention: BodyRawRetention,
) {
    assert_eq!(binding.body_source_schema(), "solstone.body.bundle.v1");
    assert_eq!(binding.body_bundle_ref(), "body-bundle.json");
    assert_eq!(binding.body_bundle_sha256(), &digest());
    assert_eq!(binding.import_id(), import_id);
    assert_eq!(binding.source_type(), source_type);
    assert_eq!(binding.source_hash(), source_hash);
    assert_eq!(binding.entry_count(), entry_count);
    assert_eq!(binding.days_affected(), days_affected);
    assert_eq!(binding.raw_retention(), raw_retention);

    let object = binding.to_body_object();
    let Some(BodyValue::Integer(integer)) = object.get(&body_string("entry_count")) else {
        panic!("entry count is an integer");
    };
    assert_eq!(integer.digits(), entry_count.to_string());
    assert!(!integer.is_negative());
    assert_eq!(
        object.get(&body_string("days_affected")),
        Some(&BodyValue::Array(
            days_affected
                .iter()
                .map(|day| BodyValue::String(day.to_body_string()))
                .collect()
        ))
    );
}

#[test]
fn fixture_bindings_preserve_all_checked_values_and_emit_exactly_nine_keys() {
    let cases = native_bundle_manifest_binding_cases();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let binding = BodyManifestBinding::new(
            case.body_bundle_sha256.clone(),
            case.import_id.clone(),
            case.source_type,
            case.source_hash.clone(),
            case.entry_count,
            case.days_affected.clone(),
            case.raw_retention,
        )
        .unwrap_or_else(|error| panic!("{} should bind: {error}", case.name));

        assert_eq!(
            binding.body_source_schema(),
            "solstone.body.bundle.v1",
            "{}",
            case.name
        );
        assert_eq!(
            binding.body_bundle_ref(),
            "body-bundle.json",
            "{}",
            case.name
        );
        assert_eq!(
            binding.body_bundle_sha256(),
            &case.body_bundle_sha256,
            "{}",
            case.name
        );
        assert_eq!(binding.import_id(), &case.import_id, "{}", case.name);
        assert_eq!(binding.source_type(), case.source_type, "{}", case.name);
        assert_eq!(binding.source_hash(), &case.source_hash, "{}", case.name);
        assert_eq!(binding.entry_count(), case.entry_count, "{}", case.name);
        assert_eq!(
            binding.days_affected(),
            case.days_affected.as_slice(),
            "{}",
            case.name
        );
        assert_eq!(binding.raw_retention(), case.raw_retention, "{}", case.name);

        let actual = BodyValue::Object(binding.to_body_object());
        let expected = BodyValue::Object(expected_object(&case.expected_manifest_binding));
        assert_body_value_bitwise_eq(&actual, &expected);
        let BodyValue::Object(actual) = actual else {
            unreachable!();
        };
        assert_eq!(actual.len(), 9, "{}", case.name);
    }
}

#[test]
fn binding_validates_family_policy_days_and_cardinality_boundaries() {
    for family in [BodySourceFamily::AppleHealth, BodySourceFamily::OuraApi] {
        for retention in [
            BodyRawRetention::Discard,
            BodyRawRetention::RetainComplete,
            BodyRawRetention::RetainParsed,
        ] {
            let result = binding(
                bundle(MIN_BUNDLE),
                family,
                hash(
                    if family == BodySourceFamily::AppleHealth {
                        APPLE_HASH
                    } else {
                        OURA_HASH
                    },
                    family,
                ),
                0,
                vec![],
                retention,
            );
            if family == BodySourceFamily::OuraApi && retention == BodyRawRetention::RetainComplete
            {
                assert_error(
                    result,
                    ManifestBindingErrorCode::IncompatibleField,
                    ManifestBindingErrorField::RawRetention,
                );
            } else {
                assert!(result.is_ok(), "{family:?} {retention:?} should bind");
            }
        }
    }

    for apple_hash in [
        APPLE_HASH.to_owned(),
        format!("{APPLE_HASH}#window:20260101:20260102"),
    ] {
        assert_error(
            binding(
                bundle(MIN_BUNDLE),
                BodySourceFamily::OuraApi,
                hash(&apple_hash, BodySourceFamily::AppleHealth),
                0,
                vec![],
                BodyRawRetention::Discard,
            ),
            ManifestBindingErrorCode::IncompatibleField,
            ManifestBindingErrorField::SourceHash,
        );
    }
    assert_error(
        binding(
            bundle(MIN_BUNDLE),
            BodySourceFamily::AppleHealth,
            hash(OURA_HASH, BodySourceFamily::OuraApi),
            0,
            vec![],
            BodyRawRetention::Discard,
        ),
        ManifestBindingErrorCode::IncompatibleField,
        ManifestBindingErrorField::SourceHash,
    );

    for days in [
        vec![day("20260102"), day("20260101")],
        vec![day("20260101"), day("20260101")],
    ] {
        assert_error(
            binding(
                bundle(MIN_BUNDLE),
                BodySourceFamily::AppleHealth,
                hash(APPLE_HASH, BodySourceFamily::AppleHealth),
                2,
                days,
                BodyRawRetention::Discard,
            ),
            ManifestBindingErrorCode::InvalidField,
            ManifestBindingErrorField::DaysAffected,
        );
    }

    for (entry_count, days) in [
        (0, vec![day("20260101")]),
        (1, vec![]),
        (1, vec![day("20260101"), day("20260102")]),
    ] {
        assert_error(
            binding(
                bundle(MIN_BUNDLE),
                BodySourceFamily::AppleHealth,
                hash(APPLE_HASH, BodySourceFamily::AppleHealth),
                entry_count,
                days,
                BodyRawRetention::Discard,
            ),
            ManifestBindingErrorCode::IncompatibleField,
            ManifestBindingErrorField::DaysAffected,
        );
    }

    let min_max_import_id = bundle(MIN_BUNDLE);
    let min_max_source_hash = hash(APPLE_HASH, BodySourceFamily::AppleHealth);
    let min_max_days = vec![day("00010101"), day("99991231")];
    let min_max = binding(
        min_max_import_id.clone(),
        BodySourceFamily::AppleHealth,
        min_max_source_hash.clone(),
        2,
        min_max_days.clone(),
        BodyRawRetention::Discard,
    )
    .expect("minimum and maximum days bind");
    assert_binding_values(
        &min_max,
        &min_max_import_id,
        BodySourceFamily::AppleHealth,
        &min_max_source_hash,
        2,
        &min_max_days,
        BodyRawRetention::Discard,
    );

    let leap_import_id = bundle(MIN_BUNDLE);
    let leap_source_hash = hash(APPLE_HASH, BodySourceFamily::AppleHealth);
    let leap_days = vec![day("20240228"), day("20240229"), day("20240301")];
    let leap = binding(
        leap_import_id.clone(),
        BodySourceFamily::AppleHealth,
        leap_source_hash.clone(),
        3,
        leap_days.clone(),
        BodyRawRetention::Discard,
    )
    .expect("leap transition binds");
    assert_binding_values(
        &leap,
        &leap_import_id,
        BodySourceFamily::AppleHealth,
        &leap_source_hash,
        3,
        &leap_days,
        BodyRawRetention::Discard,
    );

    let many_rows_import_id = bundle(MIN_BUNDLE);
    let many_rows_source_hash = hash(APPLE_HASH, BodySourceFamily::AppleHealth);
    let many_rows_days = vec![day("20260101")];
    let many_rows = binding(
        many_rows_import_id.clone(),
        BodySourceFamily::AppleHealth,
        many_rows_source_hash.clone(),
        2,
        many_rows_days.clone(),
        BodyRawRetention::Discard,
    )
    .expect("many rows on one day bind");
    assert_binding_values(
        &many_rows,
        &many_rows_import_id,
        BodySourceFamily::AppleHealth,
        &many_rows_source_hash,
        2,
        &many_rows_days,
        BodyRawRetention::Discard,
    );

    let equal_import_id = bundle(MIN_BUNDLE);
    let equal_source_hash = hash(APPLE_HASH, BodySourceFamily::AppleHealth);
    let equal_days = vec![day("20260101"), day("20260102")];
    let equal = binding(
        equal_import_id.clone(),
        BodySourceFamily::AppleHealth,
        equal_source_hash.clone(),
        2,
        equal_days.clone(),
        BodyRawRetention::Discard,
    )
    .expect("one row per affected day binds");
    assert_binding_values(
        &equal,
        &equal_import_id,
        BodySourceFamily::AppleHealth,
        &equal_source_hash,
        2,
        &equal_days,
        BodyRawRetention::Discard,
    );

    let maximum = binding(
        bundle(MAX_BUNDLE),
        BodySourceFamily::AppleHealth,
        hash(APPLE_HASH, BodySourceFamily::AppleHealth),
        u64::MAX,
        vec![day("20260101")],
        BodyRawRetention::Discard,
    )
    .expect("maximum count binds");
    assert_eq!(maximum.entry_count(), u64::MAX);
    let maximum_object = maximum.to_body_object();
    let Some(BodyValue::Integer(integer)) = maximum_object.get(&body_string("entry_count")) else {
        panic!("entry count is an integer");
    };
    assert_eq!(integer.digits(), "18446744073709551615");
    assert!(!integer.is_negative());
}

#[test]
fn binding_failure_precedence_is_stable() {
    assert_error(
        binding(
            bundle(MIN_BUNDLE),
            BodySourceFamily::OuraApi,
            hash(APPLE_HASH, BodySourceFamily::AppleHealth),
            0,
            vec![day("20260102"), day("20260101")],
            BodyRawRetention::RetainComplete,
        ),
        ManifestBindingErrorCode::IncompatibleField,
        ManifestBindingErrorField::SourceHash,
    );
    assert_error(
        binding(
            bundle(MIN_BUNDLE),
            BodySourceFamily::OuraApi,
            hash(OURA_HASH, BodySourceFamily::OuraApi),
            0,
            vec![day("20260102"), day("20260101")],
            BodyRawRetention::RetainComplete,
        ),
        ManifestBindingErrorCode::InvalidField,
        ManifestBindingErrorField::DaysAffected,
    );
    assert_error(
        binding(
            bundle(MIN_BUNDLE),
            BodySourceFamily::OuraApi,
            hash(OURA_HASH, BodySourceFamily::OuraApi),
            0,
            vec![day("20260101")],
            BodyRawRetention::RetainComplete,
        ),
        ManifestBindingErrorCode::IncompatibleField,
        ManifestBindingErrorField::DaysAffected,
    );
    assert_error(
        binding(
            bundle(MIN_BUNDLE),
            BodySourceFamily::OuraApi,
            hash(OURA_HASH, BodySourceFamily::OuraApi),
            1,
            vec![day("20260101")],
            BodyRawRetention::RetainComplete,
        ),
        ManifestBindingErrorCode::IncompatibleField,
        ManifestBindingErrorField::RawRetention,
    );
    assert_error(
        binding(
            bundle(MIN_BUNDLE),
            BodySourceFamily::OuraApi,
            hash(OURA_HASH, BodySourceFamily::OuraApi),
            2,
            vec![day("20260102"), day("20260101")],
            BodyRawRetention::RetainComplete,
        ),
        ManifestBindingErrorCode::InvalidField,
        ManifestBindingErrorField::DaysAffected,
    );
}

#[test]
fn binding_errors_are_bounded_and_source_free_for_bundle_sentinels() {
    for (identifier, source_type, source_hash, raw_retention, code, field) in [
        (
            MIN_BUNDLE,
            BodySourceFamily::OuraApi,
            hash(APPLE_HASH, BodySourceFamily::AppleHealth),
            BodyRawRetention::Discard,
            ManifestBindingErrorCode::IncompatibleField,
            ManifestBindingErrorField::SourceHash,
        ),
        (
            MAX_BUNDLE,
            BodySourceFamily::OuraApi,
            hash(OURA_HASH, BodySourceFamily::OuraApi),
            BodyRawRetention::RetainComplete,
            ManifestBindingErrorCode::IncompatibleField,
            ManifestBindingErrorField::RawRetention,
        ),
    ] {
        let result = binding(
            bundle(identifier),
            source_type,
            source_hash,
            0,
            vec![],
            raw_retention,
        );
        let Err(error) = result else {
            panic!("sentinel case should refuse");
        };
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), field);
        assert_eq!(
            error.to_string(),
            format!(
                "body-manifest[{identifier}] {}: {}",
                code.as_str(),
                field.as_str()
            )
        );
    }
}

#[test]
fn body_integer_from_u64_is_non_negative_and_exact() {
    let integer = BodyInteger::from_u64(u64::MAX);
    assert_eq!(integer.digits(), "18446744073709551615");
    assert!(!integer.is_negative());
}
