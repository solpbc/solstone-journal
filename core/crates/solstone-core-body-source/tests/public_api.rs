// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashSet};

use solstone_core_body_source::{
    BODY_BUNDLE_REF_KEY, BODY_BUNDLE_SHA256_KEY, BODY_SOURCE_SCHEMA_KEY, BodyCalendarError,
    BodyCalendarField, BodyDay, BodyDigest, BodyInteger, BodyMonth, BodyRawRetention,
    BodySourceFamily, BodySourceHash, BodySourceHashError, BodySourcePolicyError,
    BodySourcePolicyField, BodyString, BodyValue, BodyWireIdentityError, BodyWireIdentityField,
    BundleId, DAYS_AFFECTED_KEY, ENTRY_COUNT_KEY, IMPORT_ID_KEY, ManifestKeySignal,
    ManifestKnownKey, ManifestScanError, ParseError, RAW_RETENTION_KEY, SOURCE_HASH_KEY,
    SOURCE_TYPE_KEY, ScannedBodyManifest, canonicalize, inspect_body_manifest_signal, parse,
    scan_body_manifest,
};

mod support;

use support::codec_rows;

#[test]
fn public_value_model_and_codec_rows_are_usable() {
    let key = BodyString::from_code_points(vec![u32::from(b'k')]).unwrap();
    let integer = BodyInteger::new(true, "42").unwrap();
    let mut object = BTreeMap::new();
    object.insert(key.clone(), BodyValue::Null);
    let values = [
        BodyValue::Null,
        BodyValue::Bool(true),
        BodyValue::Integer(integer),
        BodyValue::Number(-0.0),
        BodyValue::String(key.clone()),
        BodyValue::Array(vec![]),
        BodyValue::Object(object),
    ];
    assert_eq!(values.len(), 7);
    let BodyValue::Object(parsed) = parse(br#"{"k":1}"#).expect("object should parse") else {
        panic!("expected object");
    };
    assert_eq!(
        parsed.get(&key),
        Some(&BodyValue::Integer(BodyInteger::new(false, "1").unwrap()))
    );

    let fixture = codec_rows();
    for row in fixture["rows"].as_array().expect("rows") {
        let compact = serde_json::to_string(&row["row"]).expect("row should serialize");
        let parsed = parse(compact.as_bytes()).expect("codec row should parse");
        let BodyValue::Object(object) = parsed else {
            panic!("codec row must be object");
        };
        assert!(object.contains_key(
            &BodyString::from_code_points("schema".chars().map(u32::from).collect()).unwrap()
        ));
        assert!(
            object
                .values()
                .any(|value| matches!(value, BodyValue::Array(_) | BodyValue::Object(_)))
        );
        assert_eq!(
            canonicalize(&parse(compact.as_bytes()).expect("codec row should parse"))
                .expect("codec row should canonicalize"),
            row["expected_canonical_json"]
                .as_str()
                .expect("expected canonical JSON"),
            "{}",
            row["name"]
        );
    }
}

#[test]
fn public_api_differs_from_serde_at_required_fault_lines() {
    let exact = "18446744073709551616";
    let serde_value: serde_json::Value =
        serde_json::from_str(exact).expect("serde JSON accepts number");
    assert_ne!(
        serde_value.to_string(),
        exact,
        "serde must not retain this integer exactly without arbitrary_precision"
    );
    let BodyValue::Integer(integer) =
        parse(exact.as_bytes()).expect("body source should parse exact integer")
    else {
        panic!("expected exact integer");
    };
    assert_eq!(integer.digits(), exact);
    for literal in ["NaN", "Infinity", "-Infinity"] {
        assert!(serde_json::from_str::<serde_json::Value>(literal).is_err());
        assert!(matches!(
            parse(literal.as_bytes()),
            Ok(BodyValue::Number(_))
        ));
    }
    let lone = "\"\\ud800\"";
    let serde_lone = serde_json::from_str::<serde_json::Value>(lone);
    assert!(serde_lone.ok().is_none_or(|value| {
        value
            .as_str()
            .is_none_or(|text| !text.chars().any(|character| u32::from(character) == 0xd800))
    }));
    let BodyValue::String(body_lone) =
        parse(lone.as_bytes()).expect("body source should preserve lone surrogate")
    else {
        panic!("expected string");
    };
    assert_eq!(body_lone.code_points(), &[0xd800]);
    let high_surrogate_first_unit = "🫀".encode_utf16().next().expect("astral UTF-16 unit");
    assert!(high_surrogate_first_unit < 0xdfff_u16);
    assert!(
        BodyString::from_code_points(vec![0xdfff]).unwrap()
            < BodyString::from_code_points(vec![0x1fac0]).unwrap()
    );
    assert_eq!(
        parse("\"🫀\"[]".as_bytes()),
        Err(ParseError::MalformedJson { byte_offset: 6 })
    );
    assert_ne!(6, 3);
}

#[test]
fn public_constructors_enforce_integer_limits_and_canonicalize_nan_payloads() {
    let digits_4300 = format!("1{}", "0".repeat(4299));
    let digits_4301 = format!("1{}", "0".repeat(4300));
    assert!(BodyInteger::new(false, digits_4300.clone()).is_some());
    assert!(BodyInteger::new(true, digits_4300).is_some());
    assert!(BodyInteger::new(false, digits_4301.clone()).is_none());
    assert!(BodyInteger::new(true, digits_4301).is_none());

    let quiet = 0x7ff8_0000_0000_0001;
    let signaling = 0x7ff0_0000_0000_0001;
    let bits = [
        quiet,
        quiet | (1_u64 << 63),
        signaling,
        signaling | (1_u64 << 63),
    ];
    assert!(bits.windows(2).all(|pair| pair[0] != pair[1]));
    for bits in bits {
        let value = f64::from_bits(bits);
        assert!(value.is_nan());
        assert_eq!(value.to_bits(), bits);
        assert_eq!(canonicalize(&BodyValue::Number(value)).unwrap(), "NaN");
    }
}

#[test]
fn direct_values_canonicalize_without_mutation() {
    let astral_and_lone = BodyString::from_code_points(vec![0x1fac0, 0xd800]).unwrap();
    let mut object = BTreeMap::new();
    object.insert(
        BodyString::from_code_points(vec![u32::from(b'k')]).unwrap(),
        BodyValue::String(astral_and_lone.clone()),
    );
    let value = BodyValue::Array(vec![
        BodyValue::Null,
        BodyValue::Bool(true),
        BodyValue::Bool(false),
        BodyValue::Integer(BodyInteger::new(true, "42").unwrap()),
        BodyValue::Number(1.25),
        BodyValue::Number(f64::NEG_INFINITY),
        BodyValue::String(astral_and_lone),
        BodyValue::Array(vec![BodyValue::Null]),
        BodyValue::Object(object),
    ]);
    let snapshot = value.clone();
    let first = canonicalize(&value).expect("direct value should canonicalize");
    let second = canonicalize(&value).expect("direct value should canonicalize again");
    assert_eq!(first, second);
    assert_eq!(value, snapshot);
}

#[test]
fn codec_object_keys_sort_but_arrays_keep_stored_order() {
    let fixture = codec_rows();
    let apple = fixture["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["name"] == "apple_v1_all_shapes")
        .expect("apple row");
    let original = serde_json::to_string(&apple["row"]).expect("row should serialize");
    let key_swapped = original.replace(
        r#""future_extension":{"z":2,"a":1}"#,
        r#""future_extension":{"a":1,"z":2}"#,
    );
    assert_ne!(original, key_swapped, "object-key mutation should apply");
    let array_swapped = original.replace(
        r#""unknown_array":[1,"two",false,null,{"z":2,"a":1}]"#,
        r#""unknown_array":["two",1,false,null,{"z":2,"a":1}]"#,
    );
    assert_ne!(original, array_swapped, "array mutation should apply");

    let original_canonical = canonicalize(&parse(original.as_bytes()).unwrap()).unwrap();
    assert_eq!(
        original_canonical,
        canonicalize(&parse(key_swapped.as_bytes()).unwrap()).unwrap()
    );
    assert_ne!(
        original_canonical,
        canonicalize(&parse(array_swapped.as_bytes()).unwrap()).unwrap()
    );
}

#[test]
fn public_wire_identity_types_are_checked_ordered_and_hashable() {
    let bundle_text = "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y";
    let digest_text = "sha256:dc9b29d0ee818f2ae3cdd600a15066f4404002171a4eb99a39118b88303bd71b";
    let bundle_body_string =
        BodyString::from_code_points(bundle_text.bytes().map(u32::from).collect()).unwrap();
    let digest_body_string =
        BodyString::from_code_points(digest_text.bytes().map(u32::from).collect()).unwrap();

    let bundle_from_bytes = BundleId::from_bytes(bundle_text.as_bytes()).unwrap();
    let bundle_from_body_string = BundleId::from_body_string(&bundle_body_string).unwrap();
    let digest_from_bytes = BodyDigest::from_bytes(digest_text.as_bytes()).unwrap();
    let digest_from_body_string = BodyDigest::from_body_string(&digest_body_string).unwrap();
    assert_eq!(bundle_from_bytes, bundle_from_body_string);
    assert_eq!(bundle_from_bytes.as_str(), bundle_text);
    assert_eq!(digest_from_bytes, digest_from_body_string);
    assert_eq!(digest_from_bytes.as_str(), digest_text);

    let mut hashes = HashSet::new();
    hashes.insert((bundle_from_bytes.clone(), digest_from_bytes.clone()));
    hashes.insert((
        bundle_from_body_string.clone(),
        digest_from_body_string.clone(),
    ));
    assert_eq!(hashes.len(), 1);
    let mut ordered = BTreeSet::new();
    ordered.insert((bundle_from_bytes, digest_from_bytes));
    ordered.insert((bundle_from_body_string, digest_from_body_string));
    assert_eq!(ordered.len(), 1);

    assert_eq!(
        BundleId::from_bytes(b"body-81J9ZK2F5M7Q8R3S4T6V0W1X2Y"),
        Err(BodyWireIdentityError::InvalidFormat(
            BodyWireIdentityField::BundleId
        ))
    );
}

#[test]
fn public_source_policy_types_are_checked_ordered_and_hashable() {
    let family_text = "oura_api";
    let retention_text = "retain_complete";
    let family_body_string =
        BodyString::from_code_points(family_text.bytes().map(u32::from).collect()).unwrap();
    let retention_body_string =
        BodyString::from_code_points(retention_text.bytes().map(u32::from).collect()).unwrap();

    let family_from_bytes = BodySourceFamily::from_bytes(family_text.as_bytes()).unwrap();
    let family_from_body_string = BodySourceFamily::from_body_string(&family_body_string).unwrap();
    let retention_from_bytes = BodyRawRetention::from_bytes(retention_text.as_bytes()).unwrap();
    let retention_from_body_string =
        BodyRawRetention::from_body_string(&retention_body_string).unwrap();
    assert_eq!(family_from_bytes, family_from_body_string);
    assert_eq!(family_from_bytes.as_str(), family_text);
    assert_eq!(retention_from_bytes, retention_from_body_string);
    assert_eq!(retention_from_bytes.as_str(), retention_text);

    let mut hashes = HashSet::new();
    hashes.insert((family_from_bytes, retention_from_bytes));
    hashes.insert((family_from_body_string, retention_from_body_string));
    assert_eq!(hashes.len(), 1);
    let mut ordered = BTreeSet::new();
    ordered.insert((family_from_bytes, retention_from_bytes));
    ordered.insert((family_from_body_string, retention_from_body_string));
    assert_eq!(ordered.len(), 1);

    assert!(BodyRawRetention::RetainComplete < BodyRawRetention::RetainParsed);
    for family in [BodySourceFamily::AppleHealth, BodySourceFamily::OuraApi] {
        for retention in [
            BodyRawRetention::Discard,
            BodyRawRetention::RetainComplete,
            BodyRawRetention::RetainParsed,
        ] {
            let expected = if family == BodySourceFamily::OuraApi
                && retention == BodyRawRetention::RetainComplete
            {
                Err(BodySourcePolicyError::Incompatible(
                    BodySourcePolicyField::RawRetention,
                ))
            } else {
                Ok(())
            };
            assert_eq!(retention.check_compatible(&family), expected);
        }
    }

    assert_eq!(
        BodySourceFamily::from_bytes(b"oura"),
        Err(BodySourcePolicyError::InvalidFormat(
            BodySourcePolicyField::SourceFamily
        ))
    );
}

#[test]
fn public_calendar_types_are_checked_ordered_and_hashable() {
    let day_text = "20240229";
    let month_text = "2024-02";
    let day_body_string =
        BodyString::from_code_points(day_text.bytes().map(u32::from).collect()).unwrap();
    let month_body_string =
        BodyString::from_code_points(month_text.bytes().map(u32::from).collect()).unwrap();

    let day_from_bytes = BodyDay::from_bytes(day_text.as_bytes()).unwrap();
    let day_from_body_string = BodyDay::from_body_string(&day_body_string).unwrap();
    let month_from_bytes = BodyMonth::from_bytes(month_text.as_bytes()).unwrap();
    let month_from_body_string = BodyMonth::from_body_string(&month_body_string).unwrap();
    assert_eq!(day_from_bytes, day_from_body_string);
    assert_eq!(day_from_bytes.as_str(), day_text);
    assert_eq!(day_from_bytes.month(), month_from_bytes);
    assert_eq!(month_from_bytes, month_from_body_string);
    assert_eq!(month_from_bytes.as_str(), month_text);

    let mut hashes = HashSet::new();
    hashes.insert((day_from_bytes.clone(), month_from_bytes.clone()));
    hashes.insert((day_from_body_string.clone(), month_from_body_string.clone()));
    assert_eq!(hashes.len(), 1);
    let mut ordered = BTreeSet::new();
    ordered.insert((day_from_bytes, month_from_bytes));
    ordered.insert((day_from_body_string, month_from_body_string));
    assert_eq!(ordered.len(), 1);

    assert_eq!(
        BodyDay::from_bytes(b"20230229"),
        Err(BodyCalendarError::InvalidFormat(BodyCalendarField::Day))
    );
    assert_eq!(
        BodyMonth::from_bytes(b"2024-13"),
        Err(BodyCalendarError::InvalidFormat(BodyCalendarField::Month))
    );
}

#[test]
fn public_source_hash_is_checked_family_bound_ordered_and_hashable() {
    let plain_text = "a".repeat(64);
    let window_text = format!("{plain_text}#window:20260101:20260102");
    let plain_body_string =
        BodyString::from_code_points(plain_text.bytes().map(u32::from).collect()).unwrap();
    let window_body_string =
        BodyString::from_code_points(window_text.bytes().map(u32::from).collect()).unwrap();

    let apple_from_bytes = BodySourceHash::from_bytes_for_family(
        plain_text.as_bytes(),
        &BodySourceFamily::AppleHealth,
    )
    .unwrap();
    let apple_from_body_string = BodySourceHash::from_body_string_for_family(
        &plain_body_string,
        &BodySourceFamily::AppleHealth,
    )
    .unwrap();
    let oura_from_bytes =
        BodySourceHash::from_bytes_for_family(plain_text.as_bytes(), &BodySourceFamily::OuraApi)
            .unwrap();
    let oura_from_body_string =
        BodySourceHash::from_body_string_for_family(&plain_body_string, &BodySourceFamily::OuraApi)
            .unwrap();
    let window_from_body_string = BodySourceHash::from_body_string_for_family(
        &window_body_string,
        &BodySourceFamily::AppleHealth,
    )
    .unwrap();
    assert_eq!(apple_from_bytes, apple_from_body_string);
    assert_eq!(apple_from_bytes.as_str(), plain_text);
    assert_eq!(apple_from_bytes.family(), BodySourceFamily::AppleHealth);
    assert_eq!(oura_from_bytes, oura_from_body_string);
    assert_eq!(oura_from_bytes.as_str(), plain_text);
    assert_eq!(oura_from_bytes.family(), BodySourceFamily::OuraApi);
    assert_eq!(oura_from_body_string.as_str(), plain_text);
    assert_eq!(oura_from_body_string.family(), BodySourceFamily::OuraApi);
    assert_eq!(window_from_body_string.as_str(), window_text);
    assert_eq!(
        window_from_body_string.family(),
        BodySourceFamily::AppleHealth
    );
    assert_ne!(apple_from_bytes, oura_from_bytes);

    let mut hashes = HashSet::new();
    hashes.insert(apple_from_bytes.clone());
    hashes.insert(apple_from_body_string.clone());
    hashes.insert(oura_from_bytes.clone());
    hashes.insert(oura_from_body_string.clone());
    hashes.insert(window_from_body_string.clone());
    assert_eq!(hashes.len(), 3);
    let mut ordered = BTreeSet::new();
    ordered.insert(apple_from_bytes);
    ordered.insert(apple_from_body_string);
    ordered.insert(oura_from_bytes);
    ordered.insert(oura_from_body_string);
    ordered.insert(window_from_body_string);
    assert_eq!(ordered.len(), 3);

    assert_eq!(
        BodySourceHash::from_bytes_for_family(window_text.as_bytes(), &BodySourceFamily::OuraApi),
        Err(BodySourceHashError::InvalidFormat)
    );
}

#[test]
fn public_manifest_scan_api_is_lossless_and_redacting() {
    let known_keys = [
        (ManifestKnownKey::BodySourceSchema, BODY_SOURCE_SCHEMA_KEY),
        (ManifestKnownKey::BodyBundleRef, BODY_BUNDLE_REF_KEY),
        (ManifestKnownKey::BodyBundleSha256, BODY_BUNDLE_SHA256_KEY),
        (ManifestKnownKey::ImportId, IMPORT_ID_KEY),
        (ManifestKnownKey::SourceType, SOURCE_TYPE_KEY),
        (ManifestKnownKey::SourceHash, SOURCE_HASH_KEY),
        (ManifestKnownKey::EntryCount, ENTRY_COUNT_KEY),
        (ManifestKnownKey::DaysAffected, DAYS_AFFECTED_KEY),
        (ManifestKnownKey::RawRetention, RAW_RETENTION_KEY),
    ];
    for (known, spelling) in known_keys {
        let string = BodyString::from_code_points(spelling.bytes().map(u32::from).collect())
            .expect("known key is ASCII");
        assert_eq!(known.as_str(), spelling);
        assert_eq!(ManifestKnownKey::from_body_string(&string), Some(known));
    }
    let unknown = BodyString::from_code_points("other".bytes().map(u32::from).collect()).unwrap();
    assert_eq!(ManifestKnownKey::from_body_string(&unknown), None);

    let input = br#"{
        "body_source_schema": null,
        "body_bundle_ref": null,
        "body_bundle_sha256": null,
        "import_id": 1,
        "import_id": 2,
        "source_type": null,
        "source_hash": null,
        "entry_count": null,
        "days_affected": null,
        "raw_retention": null,
        "body_future_field": true
    }"#;
    let scanned: ScannedBodyManifest = scan_body_manifest(input).expect("manifest scans");
    assert!(scanned.has_body_prefixed_key());
    assert!(scanned.has_unknown_body_prefixed_key());
    assert_eq!(
        scanned.duplicated_known_keys(),
        &[ManifestKnownKey::ImportId]
    );
    let future_key =
        BodyString::from_code_points("body_future_field".bytes().map(u32::from).collect())
            .expect("future key is ASCII");
    assert_eq!(
        scanned.object().get(&future_key),
        Some(&BodyValue::Bool(true))
    );

    assert_eq!(
        inspect_body_manifest_signal(Some(input)),
        ManifestKeySignal::BodyKeyPresent {
            unknown_body_key: true,
        }
    );
    assert_eq!(
        inspect_body_manifest_signal(Some(br#"{"import_id":null}"#)),
        ManifestKeySignal::NoBodyKey
    );
    assert_eq!(
        inspect_body_manifest_signal(None),
        ManifestKeySignal::Unreadable
    );

    let spelling = "body_future_field";
    assert!(!format!("{:?}", scanned.duplicated_known_keys()).contains(spelling));
    assert!(
        !format!(
            "{:?}",
            ManifestKeySignal::BodyKeyPresent {
                unknown_body_key: true
            }
        )
        .contains(spelling)
    );
    for error in [
        ManifestScanError::InputTooLarge,
        ManifestScanError::MalformedManifest,
    ] {
        assert!(!error.to_string().contains(spelling));
    }
}
