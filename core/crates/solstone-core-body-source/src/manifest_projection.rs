// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::manifest_binding::{BODY_BUNDLE_REF_VALUE, BODY_SOURCE_SCHEMA_VALUE};
use crate::{
    BodyDay, BodyDigest, BodyManifestBinding, BodyObject, BodyRawRetention, BodySourceFamily,
    BodySourceHash, BodyString, BodyValue, BundleId, ManifestBindingError,
    ManifestBindingErrorCode, ManifestBindingErrorField, ManifestKnownKey,
};

/// Maps a known manifest key to its corresponding binding-error field.
pub(crate) const fn manifest_binding_error_field(
    key: ManifestKnownKey,
) -> ManifestBindingErrorField {
    match key {
        ManifestKnownKey::BodySourceSchema => ManifestBindingErrorField::BodySourceSchema,
        ManifestKnownKey::BodyBundleRef => ManifestBindingErrorField::BodyBundleRef,
        ManifestKnownKey::BodyBundleSha256 => ManifestBindingErrorField::BodyBundleSha256,
        ManifestKnownKey::ImportId => ManifestBindingErrorField::ImportId,
        ManifestKnownKey::SourceType => ManifestBindingErrorField::SourceType,
        ManifestKnownKey::SourceHash => ManifestBindingErrorField::SourceHash,
        ManifestKnownKey::EntryCount => ManifestBindingErrorField::EntryCount,
        ManifestKnownKey::DaysAffected => ManifestBindingErrorField::DaysAffected,
        ManifestKnownKey::RawRetention => ManifestBindingErrorField::RawRetention,
    }
}

/// Projects decoded manifest fields into a checked binding for an expected bundle.
#[allow(dead_code)]
pub(crate) fn project_manifest_binding(
    object: &BodyObject,
    expected: &BundleId,
) -> Result<BodyManifestBinding, ManifestBindingError> {
    let body_source_schema = required_string(object, ManifestKnownKey::BodySourceSchema, expected)?;
    if !body_string_matches(body_source_schema, BODY_SOURCE_SCHEMA_VALUE) {
        return Err(error(
            expected,
            ManifestBindingErrorCode::InvalidField,
            ManifestKnownKey::BodySourceSchema,
        ));
    }

    let body_bundle_ref = required_string(object, ManifestKnownKey::BodyBundleRef, expected)?;
    if !body_string_matches(body_bundle_ref, BODY_BUNDLE_REF_VALUE) {
        return Err(error(
            expected,
            ManifestBindingErrorCode::InvalidField,
            ManifestKnownKey::BodyBundleRef,
        ));
    }

    let body_bundle_sha256 = BodyDigest::from_body_string(required_string(
        object,
        ManifestKnownKey::BodyBundleSha256,
        expected,
    )?)
    .map_err(|_| {
        error(
            expected,
            ManifestBindingErrorCode::InvalidField,
            ManifestKnownKey::BodyBundleSha256,
        )
    })?;

    let import_id = BundleId::from_body_string(required_string(
        object,
        ManifestKnownKey::ImportId,
        expected,
    )?)
    .map_err(|_| {
        error(
            expected,
            ManifestBindingErrorCode::InvalidField,
            ManifestKnownKey::ImportId,
        )
    })?;
    if import_id != *expected {
        return Err(error(
            expected,
            ManifestBindingErrorCode::IncompatibleField,
            ManifestKnownKey::ImportId,
        ));
    }

    let source_type = BodySourceFamily::from_body_string(required_string(
        object,
        ManifestKnownKey::SourceType,
        expected,
    )?)
    .map_err(|_| {
        error(
            expected,
            ManifestBindingErrorCode::InvalidField,
            ManifestKnownKey::SourceType,
        )
    })?;

    let source_hash = BodySourceHash::from_body_string_for_family(
        required_string(object, ManifestKnownKey::SourceHash, expected)?,
        &source_type,
    )
    .map_err(|_| {
        error(
            expected,
            ManifestBindingErrorCode::InvalidField,
            ManifestKnownKey::SourceHash,
        )
    })?;

    let entry_count = match field_value(object, ManifestKnownKey::EntryCount) {
        None => {
            return Err(error(
                expected,
                ManifestBindingErrorCode::MissingField,
                ManifestKnownKey::EntryCount,
            ));
        }
        Some(BodyValue::Integer(value)) => value,
        Some(_) => {
            return Err(error(
                expected,
                ManifestBindingErrorCode::WrongType,
                ManifestKnownKey::EntryCount,
            ));
        }
    };
    if entry_count.is_negative() {
        return Err(error(
            expected,
            ManifestBindingErrorCode::InvalidField,
            ManifestKnownKey::EntryCount,
        ));
    }
    let entry_count = entry_count.digits().parse::<u64>().map_err(|_| {
        error(
            expected,
            ManifestBindingErrorCode::InvalidField,
            ManifestKnownKey::EntryCount,
        )
    })?;

    let days_affected = match field_value(object, ManifestKnownKey::DaysAffected) {
        None => {
            return Err(error(
                expected,
                ManifestBindingErrorCode::MissingField,
                ManifestKnownKey::DaysAffected,
            ));
        }
        Some(BodyValue::Array(values)) => values,
        Some(_) => {
            return Err(error(
                expected,
                ManifestBindingErrorCode::WrongType,
                ManifestKnownKey::DaysAffected,
            ));
        }
    };
    let mut parsed_days = Vec::with_capacity(days_affected.len());
    for value in days_affected {
        let BodyValue::String(value) = value else {
            return Err(error(
                expected,
                ManifestBindingErrorCode::InvalidField,
                ManifestKnownKey::DaysAffected,
            ));
        };
        parsed_days.push(BodyDay::from_body_string(value).map_err(|_| {
            error(
                expected,
                ManifestBindingErrorCode::InvalidField,
                ManifestKnownKey::DaysAffected,
            )
        })?);
    }

    let raw_retention = BodyRawRetention::from_body_string(required_string(
        object,
        ManifestKnownKey::RawRetention,
        expected,
    )?)
    .map_err(|_| {
        error(
            expected,
            ManifestBindingErrorCode::InvalidField,
            ManifestKnownKey::RawRetention,
        )
    })?;

    BodyManifestBinding::new(
        body_bundle_sha256,
        expected.clone(),
        source_type,
        source_hash,
        entry_count,
        parsed_days,
        raw_retention,
    )
}

fn field_value(object: &BodyObject, key: ManifestKnownKey) -> Option<&BodyValue> {
    let key = BodyString::from_code_points(key.as_str().bytes().map(u32::from).collect())
        .expect("ASCII manifest key is a valid body string");
    object.get(&key)
}

fn required_string<'a>(
    object: &'a BodyObject,
    key: ManifestKnownKey,
    expected: &BundleId,
) -> Result<&'a BodyString, ManifestBindingError> {
    match field_value(object, key) {
        None => Err(error(expected, ManifestBindingErrorCode::MissingField, key)),
        Some(BodyValue::String(value)) => Ok(value),
        Some(_) => Err(error(expected, ManifestBindingErrorCode::WrongType, key)),
    }
}

fn error(
    expected: &BundleId,
    code: ManifestBindingErrorCode,
    key: ManifestKnownKey,
) -> ManifestBindingError {
    ManifestBindingError::new(expected.clone(), code, manifest_binding_error_field(key))
}

fn body_string_matches(value: &BodyString, literal: &str) -> bool {
    value
        .code_points()
        .iter()
        .copied()
        .eq(literal.bytes().map(u32::from))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::path::Path;

    use serde_json::Value;

    use super::*;
    use crate::{
        BODY_BUNDLE_REF_KEY, BODY_BUNDLE_SHA256_KEY, BODY_SOURCE_SCHEMA_KEY, BodyInteger,
        DAYS_AFFECTED_KEY, ENTRY_COUNT_KEY, IMPORT_ID_KEY, RAW_RETENTION_KEY, SOURCE_HASH_KEY,
        SOURCE_TYPE_KEY, parse,
    };

    const BUNDLE: &str = "body-00000000000000000000000000";
    const OTHER_BUNDLE: &str = "body-00000000000000000000000001";
    const DIGEST: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    const APPLE_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OURA_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn ac1_fixture_cases_round_trip_to_exact_bindings() {
        let fixture: Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../core/fixtures/body_source_native_bundle_v1.json"),
            )
            .expect("fixture should read"),
        )
        .expect("fixture should parse");
        let cases = fixture["cases"].as_array().expect("fixture cases");
        assert_eq!(cases.len(), 4);

        for case in cases {
            let name = case["name"].as_str().expect("case name");
            let expected = expected_object(&case["expected_manifest_binding"]);
            let bundle = BundleId::from_body_string(&body_string(
                case["directory"].as_str().expect("case directory"),
            ))
            .expect("fixture directory is valid");
            let actual = project_manifest_binding(&expected, &bundle)
                .unwrap_or_else(|error| panic!("{name} should project: {error}"));
            assert_eq!(actual.to_body_object(), expected, "{name}");
        }
    }

    #[test]
    fn ac2_all_field_phase_one_failures_map_to_manifest_errors() {
        for key in ManifestKnownKey::ALL {
            let mut missing = valid_object();
            missing.remove(&body_string(key.as_str()));
            assert_error(
                &missing,
                ManifestBindingErrorCode::MissingField,
                manifest_binding_error_field(key),
            );

            let mut wrong_type = valid_object();
            wrong_type.insert(body_string(key.as_str()), BodyValue::Null);
            assert_error(
                &wrong_type,
                ManifestBindingErrorCode::WrongType,
                manifest_binding_error_field(key),
            );

            let mut invalid = valid_object();
            invalid.insert(body_string(key.as_str()), invalid_value(key));
            assert_error(
                &invalid,
                ManifestBindingErrorCode::InvalidField,
                manifest_binding_error_field(key),
            );
        }

        let mut incompatible_import = valid_object();
        incompatible_import.insert(
            body_string(IMPORT_ID_KEY),
            BodyValue::String(body_string(OTHER_BUNDLE)),
        );
        assert_error(
            &incompatible_import,
            ManifestBindingErrorCode::IncompatibleField,
            ManifestBindingErrorField::ImportId,
        );

        for suffix in [
            "#window:open:20260101",
            "#window:20260101:open",
            "#window:20260101:20260102",
        ] {
            let mut oura = valid_oura_object(BodyRawRetention::RetainParsed);
            oura.insert(
                body_string(SOURCE_HASH_KEY),
                BodyValue::String(body_string(&format!("{APPLE_HASH}{suffix}"))),
            );
            assert_error(
                &oura,
                ManifestBindingErrorCode::InvalidField,
                ManifestBindingErrorField::SourceHash,
            );
        }
    }

    #[test]
    fn ac3_entry_count_boundaries_are_checked_and_emitted_exactly() {
        for (integer, expected_count, days) in [
            (BodyInteger::from_u64(0), 0, Vec::new()),
            (
                BodyInteger::new(true, "0").expect("negative zero normalizes"),
                0,
                Vec::new(),
            ),
            (
                BodyInteger::from_u64(u64::MAX),
                u64::MAX,
                vec![BodyValue::String(body_string("20260101"))],
            ),
        ] {
            let mut object = valid_object();
            object.insert(body_string(ENTRY_COUNT_KEY), BodyValue::Integer(integer));
            object.insert(body_string(DAYS_AFFECTED_KEY), BodyValue::Array(days));
            let binding =
                project_manifest_binding(&object, &bundle()).expect("count should project");
            assert_eq!(binding.entry_count(), expected_count);
            assert_eq!(
                binding.to_body_object().get(&body_string(ENTRY_COUNT_KEY)),
                Some(&BodyValue::Integer(BodyInteger::from_u64(expected_count)))
            );
        }

        for integer in [
            BodyInteger::new(false, "18446744073709551616").expect("integer is valid"),
            BodyInteger::new(true, "1").expect("integer is valid"),
            BodyInteger::new(false, format!("1{}", "0".repeat(4299))).expect("integer is valid"),
            BodyInteger::new(true, format!("1{}", "0".repeat(4299))).expect("integer is valid"),
        ] {
            let mut object = valid_object();
            object.insert(body_string(ENTRY_COUNT_KEY), BodyValue::Integer(integer));
            assert_error(
                &object,
                ManifestBindingErrorCode::InvalidField,
                ManifestBindingErrorField::EntryCount,
            );
        }

        for value in [
            BodyValue::Number(0.0),
            BodyValue::Number(f64::NAN),
            BodyValue::Number(f64::INFINITY),
            BodyValue::String(body_string("0")),
            BodyValue::Bool(false),
            BodyValue::Null,
        ] {
            let mut object = valid_object();
            object.insert(body_string(ENTRY_COUNT_KEY), value);
            assert_error(
                &object,
                ManifestBindingErrorCode::WrongType,
                ManifestBindingErrorField::EntryCount,
            );
        }
    }

    #[test]
    fn ac4_days_affected_preserves_phase_one_order_and_defers_ordering_checks() {
        for (days, entry_count) in [
            (Vec::new(), 0),
            (vec!["20260101"], 1),
            (vec!["00010101"], 1),
            (vec!["99991231"], 1),
            (vec!["20240228", "20240229", "20240301"], 3),
        ] {
            let mut object = valid_object();
            object.insert(
                body_string(DAYS_AFFECTED_KEY),
                BodyValue::Array(
                    days.into_iter()
                        .map(|day| BodyValue::String(body_string(day)))
                        .collect(),
                ),
            );
            object.insert(
                body_string(ENTRY_COUNT_KEY),
                BodyValue::Integer(BodyInteger::from_u64(entry_count)),
            );
            assert!(project_manifest_binding(&object, &bundle()).is_ok());
        }

        for value in [BodyValue::Null, BodyValue::String(body_string("20260101"))] {
            let mut object = valid_object();
            object.insert(body_string(DAYS_AFFECTED_KEY), value);
            assert_error(
                &object,
                ManifestBindingErrorCode::WrongType,
                ManifestBindingErrorField::DaysAffected,
            );
        }
        for value in [
            BodyValue::Array(vec![BodyValue::Integer(BodyInteger::from_u64(1))]),
            BodyValue::Array(vec![BodyValue::String(body_string("20230229"))]),
        ] {
            let mut object = valid_object();
            object.insert(body_string(DAYS_AFFECTED_KEY), value);
            assert_error(
                &object,
                ManifestBindingErrorCode::InvalidField,
                ManifestBindingErrorField::DaysAffected,
            );
        }
        for days in [["20260102", "20260101"], ["20260101", "20260101"]] {
            let mut object = valid_object();
            object.insert(
                body_string(DAYS_AFFECTED_KEY),
                BodyValue::Array(
                    days.into_iter()
                        .map(|day| BodyValue::String(body_string(day)))
                        .collect(),
                ),
            );
            object.insert(
                body_string(ENTRY_COUNT_KEY),
                BodyValue::Integer(BodyInteger::from_u64(2)),
            );
            assert_error(
                &object,
                ManifestBindingErrorCode::InvalidField,
                ManifestBindingErrorField::DaysAffected,
            );
        }
    }

    #[test]
    fn ac5_projection_uses_registry_order_and_delegates_phase_two_checks() {
        let mut registry_order = valid_object();
        registry_order.insert(
            body_string(BODY_SOURCE_SCHEMA_KEY),
            BodyValue::String(body_string("wrong schema")),
        );
        registry_order.insert(
            body_string(BODY_BUNDLE_REF_KEY),
            BodyValue::String(body_string("wrong ref")),
        );
        assert_error(
            &registry_order,
            ManifestBindingErrorCode::InvalidField,
            ManifestBindingErrorField::BodySourceSchema,
        );

        let mut raw_retention_first = valid_object();
        raw_retention_first.insert(
            body_string(DAYS_AFFECTED_KEY),
            BodyValue::Array(vec![
                BodyValue::String(body_string("20260102")),
                BodyValue::String(body_string("20260101")),
            ]),
        );
        raw_retention_first.insert(
            body_string(ENTRY_COUNT_KEY),
            BodyValue::Integer(BodyInteger::from_u64(2)),
        );
        raw_retention_first.insert(
            body_string(RAW_RETENTION_KEY),
            BodyValue::String(body_string("invalid")),
        );
        assert_error(
            &raw_retention_first,
            ManifestBindingErrorCode::InvalidField,
            ManifestBindingErrorField::RawRetention,
        );

        let oura = valid_oura_object(BodyRawRetention::RetainComplete);
        let Err(projected) = project_manifest_binding(&oura, &bundle()) else {
            panic!("policy should refuse");
        };
        let Err(direct) = BodyManifestBinding::new(
            BodyDigest::from_bytes(DIGEST.as_bytes()).expect("digest is valid"),
            bundle(),
            BodySourceFamily::OuraApi,
            BodySourceHash::from_bytes_for_family(OURA_HASH.as_bytes(), &BodySourceFamily::OuraApi)
                .expect("hash is valid"),
            1,
            vec![BodyDay::from_bytes(b"20260101").expect("day is valid")],
            BodyRawRetention::RetainComplete,
        ) else {
            panic!("direct policy should refuse");
        };
        assert_eq!(projected, direct);
    }

    #[test]
    fn ac6_errors_are_bounded_redacting_and_leave_sources_unchanged() {
        let sentinel = format!(
            "manifest-projection-private-sentinel{}",
            "S".repeat(1_000_000)
        );
        let marker = "manifest-projection-private-sentinel";

        let mut hash_value = valid_object();
        hash_value.insert(
            body_string(SOURCE_HASH_KEY),
            BodyValue::String(body_string(&sentinel)),
        );
        assert_sentinel_error(
            hash_value,
            ManifestBindingErrorCode::InvalidField,
            ManifestBindingErrorField::SourceHash,
            marker,
        );

        let mut ref_value = valid_object();
        ref_value.insert(
            body_string(BODY_BUNDLE_REF_KEY),
            BodyValue::String(body_string(&sentinel)),
        );
        assert_sentinel_error(
            ref_value,
            ManifestBindingErrorCode::InvalidField,
            ManifestBindingErrorField::BodyBundleRef,
            marker,
        );

        let mut extra_name = valid_object();
        extra_name.remove(&body_string(BODY_SOURCE_SCHEMA_KEY));
        extra_name.insert(body_string(&sentinel), BodyValue::Null);
        assert_sentinel_error(
            extra_name,
            ManifestBindingErrorCode::MissingField,
            ManifestBindingErrorField::BodySourceSchema,
            marker,
        );

        let mut extra_value = valid_object();
        extra_value.remove(&body_string(BODY_SOURCE_SCHEMA_KEY));
        extra_value.insert(
            body_string("ordinary_extra"),
            BodyValue::String(body_string(&sentinel)),
        );
        assert_sentinel_error(
            extra_value,
            ManifestBindingErrorCode::MissingField,
            ManifestBindingErrorField::BodySourceSchema,
            marker,
        );

        let mut import_value = valid_object();
        import_value.insert(
            body_string(IMPORT_ID_KEY),
            BodyValue::String(body_string(&sentinel)),
        );
        assert_sentinel_error(
            import_value,
            ManifestBindingErrorCode::InvalidField,
            ManifestBindingErrorField::ImportId,
            marker,
        );
    }

    fn bundle() -> BundleId {
        BundleId::from_bytes(BUNDLE.as_bytes()).expect("test bundle is valid")
    }

    fn valid_object() -> BodyObject {
        object([
            (
                BODY_SOURCE_SCHEMA_KEY,
                BodyValue::String(body_string(BODY_SOURCE_SCHEMA_VALUE)),
            ),
            (
                BODY_BUNDLE_REF_KEY,
                BodyValue::String(body_string(BODY_BUNDLE_REF_VALUE)),
            ),
            (
                BODY_BUNDLE_SHA256_KEY,
                BodyValue::String(body_string(DIGEST)),
            ),
            (IMPORT_ID_KEY, BodyValue::String(body_string(BUNDLE))),
            (
                SOURCE_TYPE_KEY,
                BodyValue::String(body_string("apple_health")),
            ),
            (SOURCE_HASH_KEY, BodyValue::String(body_string(APPLE_HASH))),
            (
                ENTRY_COUNT_KEY,
                BodyValue::Integer(BodyInteger::from_u64(1)),
            ),
            (
                DAYS_AFFECTED_KEY,
                BodyValue::Array(vec![BodyValue::String(body_string("20260101"))]),
            ),
            (RAW_RETENTION_KEY, BodyValue::String(body_string("discard"))),
        ])
    }

    fn valid_oura_object(raw_retention: BodyRawRetention) -> BodyObject {
        let mut object = valid_object();
        object.insert(
            body_string(SOURCE_TYPE_KEY),
            BodyValue::String(body_string("oura_api")),
        );
        object.insert(
            body_string(SOURCE_HASH_KEY),
            BodyValue::String(body_string(OURA_HASH)),
        );
        object.insert(
            body_string(RAW_RETENTION_KEY),
            BodyValue::String(raw_retention.to_body_string()),
        );
        object
    }

    fn object(entries: impl IntoIterator<Item = (&'static str, BodyValue)>) -> BodyObject {
        entries
            .into_iter()
            .map(|(key, value)| (body_string(key), value))
            .collect::<BTreeMap<_, _>>()
    }

    fn invalid_value(key: ManifestKnownKey) -> BodyValue {
        match key {
            ManifestKnownKey::BodySourceSchema => BodyValue::String(body_string("wrong schema")),
            ManifestKnownKey::BodyBundleRef => BodyValue::String(body_string("wrong ref")),
            ManifestKnownKey::BodyBundleSha256 => BodyValue::String(body_string("sha256:bad")),
            ManifestKnownKey::ImportId => BodyValue::String(body_string("invalid")),
            ManifestKnownKey::SourceType => BodyValue::String(body_string("invalid")),
            ManifestKnownKey::SourceHash => BodyValue::String(body_string("invalid")),
            ManifestKnownKey::EntryCount => {
                BodyValue::Integer(BodyInteger::new(true, "1").expect("integer is valid"))
            }
            ManifestKnownKey::DaysAffected => {
                BodyValue::Array(vec![BodyValue::String(body_string("20230229"))])
            }
            ManifestKnownKey::RawRetention => BodyValue::String(body_string("invalid")),
        }
    }

    fn expected_object(value: &Value) -> BodyObject {
        let encoded = serde_json::to_string(value).expect("expected binding serializes");
        let BodyValue::Object(object) = parse(encoded.as_bytes()).expect("expected binding parses")
        else {
            panic!("expected binding is an object");
        };
        object
    }

    fn body_string(value: &str) -> BodyString {
        BodyString::from_code_points(value.chars().map(u32::from).collect())
            .expect("test body string is valid")
    }

    fn assert_error(
        object: &BodyObject,
        code: ManifestBindingErrorCode,
        field: ManifestBindingErrorField,
    ) {
        let Err(error) = project_manifest_binding(object, &bundle()) else {
            panic!("projection should refuse");
        };
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), field);
        assert_eq!(error.bundle(), &bundle());
    }

    fn assert_sentinel_error(
        object: BodyObject,
        code: ManifestBindingErrorCode,
        field: ManifestBindingErrorField,
        marker: &str,
    ) {
        let snapshot = object.clone();
        let Err(error) = project_manifest_binding(&object, &bundle()) else {
            panic!("projection should refuse");
        };
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert_eq!(display, debug);
        assert!(display.len() <= 160);
        assert!(Error::source(&error).is_none());
        assert!(!display.contains(marker));
        assert!(!debug.contains(marker));
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), field);
        assert_eq!(object, snapshot);
    }
}
