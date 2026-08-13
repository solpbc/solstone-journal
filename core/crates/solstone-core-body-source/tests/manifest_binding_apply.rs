// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyInteger, BodyManifestBinding, BodyObject, BodyRawRetention,
    BodySourceFamily, BodySourceHash, BodyString, BodyValue, BundleId, ManifestBindingError,
    ManifestBindingErrorCode, ManifestBindingErrorField, parse,
};

use crate::support;

use support::{MAX_BUNDLE, assert_body_value_bitwise_eq, native_bundle_manifest_binding_cases};

const DIGEST: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const APPLE_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.chars().map(u32::from).collect())
        .expect("test body string is valid")
}

fn body_string_from_code_points(code_points: Vec<u32>) -> BodyString {
    BodyString::from_code_points(code_points).expect("test code points are valid")
}

fn object(entries: impl IntoIterator<Item = (BodyString, BodyValue)>) -> BodyObject {
    entries.into_iter().collect()
}

fn binding() -> BodyManifestBinding {
    BodyManifestBinding::new(
        BodyDigest::from_bytes(DIGEST.as_bytes()).expect("test digest is valid"),
        BundleId::from_bytes(b"body-00000000000000000000000000").expect("test bundle is valid"),
        BodySourceFamily::AppleHealth,
        BodySourceHash::from_bytes_for_family(
            APPLE_HASH.as_bytes(),
            &BodySourceFamily::AppleHealth,
        )
        .expect("test hash is valid"),
        1,
        vec![BodyDay::from_bytes(b"20260101").expect("test day is valid")],
        BodyRawRetention::Discard,
    )
    .expect("test values bind")
}

fn max_bundle_binding() -> BodyManifestBinding {
    BodyManifestBinding::new(
        BodyDigest::from_bytes(DIGEST.as_bytes()).expect("test digest is valid"),
        BundleId::from_bytes(MAX_BUNDLE.as_bytes()).expect("maximum bundle is valid"),
        BodySourceFamily::AppleHealth,
        BodySourceHash::from_bytes_for_family(
            APPLE_HASH.as_bytes(),
            &BodySourceFamily::AppleHealth,
        )
        .expect("test hash is valid"),
        0,
        vec![],
        BodyRawRetention::Discard,
    )
    .expect("maximum bundle values bind")
}

fn binding_for_fixture(case: &support::NativeBundleManifestBindingCase) -> BodyManifestBinding {
    BodyManifestBinding::new(
        case.body_bundle_sha256.clone(),
        case.import_id.clone(),
        case.source_type,
        case.source_hash.clone(),
        case.entry_count,
        case.days_affected.clone(),
        case.raw_retention,
    )
    .unwrap_or_else(|error| panic!("{} should bind: {error}", case.name))
}

fn assert_apply_error(
    result: Result<BodyObject, ManifestBindingError>,
    code: ManifestBindingErrorCode,
) -> ManifestBindingError {
    let error = result.expect_err("application should refuse");
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), ManifestBindingErrorField::Manifest);
    error
}

#[test]
fn fixture_manifests_apply_checked_bindings_preserving_extensions_and_idempotence() {
    let unrelated_keys = [
        "files_created",
        "imported_at",
        "imported_via",
        "link_id",
        "observer_handle",
        "fixture_extension",
    ];

    let cases = native_bundle_manifest_binding_cases();
    assert_eq!(cases.len(), 4);
    for case in &cases {
        let binding = binding_for_fixture(case);
        let source = &case.source_manifest;
        let BodyValue::Object(source_object) = source else {
            panic!("fixture source manifest is an object");
        };

        let result = binding
            .apply_to(source)
            .unwrap_or_else(|error| panic!("{} should apply: {error}", case.name));
        for key in unrelated_keys {
            assert_body_value_bitwise_eq(
                result.get(&body_string(key)).expect("unrelated result key"),
                source_object
                    .get(&body_string(key))
                    .expect("unrelated source key"),
            );
        }
        for (key, expected) in binding.to_body_object() {
            assert_body_value_bitwise_eq(result.get(&key).expect("known result key"), &expected);
        }

        let first = BodyValue::Object(result);
        let second = binding
            .apply_to(&first)
            .unwrap_or_else(|error| panic!("{} should reapply: {error}", case.name));
        assert_body_value_bitwise_eq(&BodyValue::Object(second), &first);
    }
}

#[test]
fn apply_preserves_unrelated_adversarial_values_bitwise_without_mutating_source() {
    let quiet = 0x7ff8_0000_0000_0001;
    let signaling = 0x7ff0_0000_0000_0001;
    let digits_4300 = format!("1{}", "0".repeat(4299));
    let duplicate_decoded = parse(
        br#"{"\u0064uplicate":"top-first","duplicate":"top-last","nested":{"\u0064uplicate":"nested-first","duplicate":"nested-last"}}"#,
    )
    .expect("duplicate decoded placements parse");
    let nested = BodyValue::Object(object([
        (
            body_string("quiet_positive"),
            BodyValue::Number(f64::from_bits(quiet)),
        ),
        (
            body_string("signaling_negative"),
            BodyValue::Number(f64::from_bits(signaling | (1_u64 << 63))),
        ),
        (
            body_string("largest_finite"),
            BodyValue::Number(f64::from_bits(0x7fef_ffff_ffff_ffff)),
        ),
        (body_string("duplicate_decoded"), duplicate_decoded),
    ]));
    let array = BodyValue::Array(vec![
        BodyValue::Null,
        BodyValue::Bool(true),
        BodyValue::Bool(false),
        BodyValue::Number(f64::from_bits(quiet | (1_u64 << 63))),
        BodyValue::Number(f64::from_bits(signaling)),
        BodyValue::Number(0.0),
        BodyValue::Number(-0.0),
        BodyValue::Number(f64::INFINITY),
        BodyValue::Number(f64::NEG_INFINITY),
        BodyValue::Number(f64::from_bits(0x3f1a_36e2_eb1c_432d)),
        BodyValue::Number(f64::from_bits(0x3ee4_f8b5_88e3_68f1)),
        BodyValue::Number(f64::from_bits(0x430c_6bf5_2634_0000)),
        BodyValue::Number(f64::from_bits(0x4341_c379_37e0_8000)),
        BodyValue::Number(f64::from_bits(0x0000_0000_0000_0001)),
        BodyValue::Integer(BodyInteger::new(false, "18446744073709551616").unwrap()),
        BodyValue::Integer(BodyInteger::new(true, "18446744073709551616").unwrap()),
        BodyValue::Integer(BodyInteger::new(false, digits_4300.clone()).unwrap()),
        BodyValue::Integer(BodyInteger::new(true, digits_4300).unwrap()),
        BodyValue::String(body_string_from_code_points(vec![0xd800, 0xdfff])),
        BodyValue::String(body_string_from_code_points(vec![0xe000])),
        BodyValue::String(body_string_from_code_points(vec![0x1fac0])),
    ]);
    let source = BodyValue::Object(object([
        (body_string("adversarial_array"), array),
        (body_string("adversarial_nested"), nested),
        (body_string("ordinary_null"), BodyValue::Null),
    ]));
    let snapshot = source.clone();
    let binding = binding();

    let result = binding
        .apply_to(&source)
        .expect("adversarial source applies");
    let mut expected = match snapshot.clone() {
        BodyValue::Object(object) => object,
        _ => unreachable!(),
    };
    expected.extend(binding.to_body_object());
    assert_body_value_bitwise_eq(&BodyValue::Object(result), &BodyValue::Object(expected));
    assert_body_value_bitwise_eq(&source, &snapshot);
}

#[test]
fn apply_preserves_near_miss_and_nested_reserved_keys() {
    let source = BodyValue::Object(object([
        (body_string("body"), BodyValue::Bool(true)),
        (body_string("Body_"), BodyValue::Bool(false)),
        (body_string(" body_"), BodyValue::Null),
        (
            body_string("xbody_"),
            BodyValue::String(body_string("body_x")),
        ),
        (
            body_string("body-"),
            BodyValue::Integer(BodyInteger::from_u64(7)),
        ),
        (
            body_string("nested"),
            BodyValue::Array(vec![BodyValue::Object(object([
                (
                    body_string("body_source_schema"),
                    BodyValue::String(body_string("nested")),
                ),
                (body_string("body_x"), BodyValue::Bool(true)),
            ]))]),
        ),
    ]));
    let snapshot = source.clone();
    let binding = binding();

    let result = binding.apply_to(&source).expect("near misses apply");
    let mut expected = match snapshot.clone() {
        BodyValue::Object(object) => object,
        _ => unreachable!(),
    };
    expected.extend(binding.to_body_object());
    assert_body_value_bitwise_eq(&BodyValue::Object(result), &BodyValue::Object(expected));
    assert_body_value_bitwise_eq(&source, &snapshot);
}

#[test]
fn apply_rejects_unknown_reserved_top_level_keys_without_mutating_source() {
    let binding = binding();
    let escaped = parse(br#"{"bo\u0064y_x":"escaped"}"#).expect("escaped key parses");
    let sources = vec![
        BodyValue::Object(object([(body_string("body_"), BodyValue::Null)])),
        BodyValue::Object(object([(body_string("body_x"), BodyValue::Bool(true))])),
        escaped,
        BodyValue::Object(object([(
            body_string_from_code_points(vec![
                u32::from(b'b'),
                u32::from(b'o'),
                u32::from(b'd'),
                u32::from(b'y'),
                u32::from(b'_'),
                0xe000,
            ]),
            BodyValue::Null,
        )])),
        BodyValue::Object(object([(
            body_string_from_code_points(vec![
                u32::from(b'b'),
                u32::from(b'o'),
                u32::from(b'd'),
                u32::from(b'y'),
                u32::from(b'_'),
                0x1fac0,
            ]),
            BodyValue::Null,
        )])),
        BodyValue::Object(object([(
            body_string_from_code_points(vec![
                u32::from(b'b'),
                u32::from(b'o'),
                u32::from(b'd'),
                u32::from(b'y'),
                u32::from(b'_'),
                0xd800,
            ]),
            BodyValue::Null,
        )])),
    ];

    for source in sources {
        let snapshot = source.clone();
        assert_apply_error(
            binding.apply_to(&source),
            ManifestBindingErrorCode::UnknownField,
        );
        assert_body_value_bitwise_eq(&source, &snapshot);
    }
}

#[test]
fn apply_errors_are_atomic_for_nonobjects_and_stale_unknown_manifests() {
    let binding = binding();
    let nonobjects = [
        BodyValue::Null,
        BodyValue::Bool(true),
        BodyValue::Integer(BodyInteger::from_u64(1)),
        BodyValue::Number(f64::from_bits(0x7ff8_0000_0000_0001)),
        BodyValue::String(body_string("not a manifest")),
        BodyValue::Array(vec![BodyValue::Object(object([(
            body_string("nested"),
            BodyValue::Number(f64::from_bits(0x7ff8_0000_0000_0001)),
        )]))]),
    ];
    for source in nonobjects {
        let snapshot = source.clone();
        assert_apply_error(
            binding.apply_to(&source),
            ManifestBindingErrorCode::WrongType,
        );
        assert_body_value_bitwise_eq(&source, &snapshot);
    }

    let unknown_source = BodyValue::Object(object([(body_string("body_x"), BodyValue::Null)]));
    let unknown_snapshot = unknown_source.clone();
    let unknown_error = assert_apply_error(
        binding.apply_to(&unknown_source),
        ManifestBindingErrorCode::UnknownField,
    );
    assert_body_value_bitwise_eq(&unknown_source, &unknown_snapshot);

    let mut stale = binding.to_body_object();
    for value in stale.values_mut() {
        *value = BodyValue::Null;
    }
    stale.insert(body_string("body_x"), BodyValue::Bool(true));
    let stale_source = BodyValue::Object(stale);
    let stale_snapshot = stale_source.clone();
    let stale_error = assert_apply_error(
        binding.apply_to(&stale_source),
        ManifestBindingErrorCode::UnknownField,
    );
    assert_eq!(stale_error, unknown_error);
    assert_body_value_bitwise_eq(&stale_source, &stale_snapshot);
}

#[test]
fn apply_overwrites_stale_known_fields_and_adds_missing_known_fields() {
    let binding = binding();
    let mut source = binding.to_body_object();
    for value in source.values_mut() {
        *value = BodyValue::Null;
    }
    source.remove(&body_string("raw_retention"));
    source.insert(body_string("extension"), BodyValue::Bool(true));
    let source = BodyValue::Object(source);
    let snapshot = source.clone();

    let result = binding.apply_to(&source).expect("stale manifest applies");
    for (key, expected) in binding.to_body_object() {
        assert_body_value_bitwise_eq(result.get(&key).expect("known result key"), &expected);
    }
    assert_body_value_bitwise_eq(
        result
            .get(&body_string("extension"))
            .expect("extension survives"),
        &BodyValue::Bool(true),
    );
    assert_body_value_bitwise_eq(&source, &snapshot);
}

#[test]
fn apply_errors_are_bounded_and_redacting_at_megabyte_scale() {
    let binding = max_bundle_binding();
    let sentinel = "body-apply-private-sentinel";
    let large_text = sentinel.repeat(40_000);
    let nonobject = BodyValue::String(body_string(&large_text));
    let nonobject_snapshot = nonobject.clone();
    let wrong_type = assert_apply_error(
        binding.apply_to(&nonobject),
        ManifestBindingErrorCode::WrongType,
    );
    assert_bounded_redacted(&wrong_type, sentinel);
    assert_body_value_bitwise_eq(&nonobject, &nonobject_snapshot);

    let mut key_points = "body_".chars().map(u32::from).collect::<Vec<_>>();
    key_points.extend(sentinel.repeat(40_000).chars().map(u32::from));
    let unknown = BodyValue::Object(object([
        (
            body_string_from_code_points(key_points),
            BodyValue::String(body_string(&large_text)),
        ),
        (
            body_string("payload"),
            BodyValue::String(body_string(&large_text)),
        ),
    ]));
    let unknown_snapshot = unknown.clone();
    let unknown_field = assert_apply_error(
        binding.apply_to(&unknown),
        ManifestBindingErrorCode::UnknownField,
    );
    assert_bounded_redacted(&unknown_field, sentinel);
    assert_body_value_bitwise_eq(&unknown, &unknown_snapshot);
}

fn assert_bounded_redacted(error: &ManifestBindingError, sentinel: &str) {
    let display = error.to_string();
    assert_eq!(display, format!("{error:?}"));
    assert!(display.len() <= 160);
    assert!(Error::source(error).is_none());
    assert!(!display.contains(sentinel));
}
