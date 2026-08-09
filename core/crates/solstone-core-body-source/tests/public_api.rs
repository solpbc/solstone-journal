// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashSet};

use solstone_core_body_source::{
    AppleSummaryPlan, AuthorityError, BODY_BUNDLE_REF_KEY, BODY_BUNDLE_SHA256_KEY,
    BODY_SOURCE_SCHEMA_KEY, BodyCalendarError, BodyCalendarField, BodyDay, BodyDigest,
    BodyEnvelope, BodyInteger, BodyManifestBinding, BodyMonth, BodyRawRetention, BodySourceFamily,
    BodySourceHash, BodySourceHashError, BodySourcePolicyError, BodySourcePolicyField, BodyString,
    BodyValue, BodyWireIdentityError, BodyWireIdentityField, BundleClass, BundleId,
    DAYS_AFFECTED_KEY, DirectoryObservation, ENTRY_COUNT_KEY, EnvelopeErrorCode,
    EnvelopeErrorField, EnvelopeLedger, EnvelopeShard, IMPORT_ID_KEY, ManifestBindingErrorCode,
    ManifestBindingErrorField, ManifestKeySignal, ManifestKnownKey, ManifestScanError,
    NativeAuthority, ParseError, RAW_RETENTION_KEY, SOURCE_HASH_KEY, SOURCE_TYPE_KEY,
    ScannedBodyManifest, authorize_native_bundle, canonicalize, classify_bundle_directory,
    decode_body_envelope, decode_body_manifest, encode_body_envelope, inspect_body_manifest_signal,
    parse, scan_body_manifest,
};

mod support;

use support::{
    codec_rows, envelope_multimonth_fixture, native_bundle_directory_cases, native_bundle_fixture,
};

fn assert_authority_error(
    result: Result<NativeAuthority, AuthorityError>,
    expected: AuthorityError,
) {
    let Err(actual) = result else {
        panic!("authority should refuse");
    };
    assert_eq!(actual, expected);
}

#[test]
fn public_authority_api_imports_and_covers_fixture_cases() {
    fn assert_observation_traits<T: Clone + Copy + std::fmt::Debug>() {}
    fn assert_class_traits<T: Clone + Copy + std::fmt::Debug + PartialEq + Eq>() {}
    fn assert_error_traits<T: Clone + PartialEq + Eq + std::error::Error>() {}
    assert_observation_traits::<DirectoryObservation<'static>>();
    assert_class_traits::<BundleClass>();
    assert_error_traits::<AuthorityError>();

    let cases = native_bundle_directory_cases();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let authority: NativeAuthority = authorize_native_bundle(DirectoryObservation {
            name: case.name.as_bytes(),
            envelope_present: true,
            ledger_present: true,
            manifest: Some(&case.manifest_bytes),
        })
        .unwrap_or_else(|error| panic!("{} should authorize: {error}", case.name));
        assert_eq!(authority.id(), &case.expected_import_id);
        assert_eq!(authority.id(), authority.binding().import_id());
        assert_eq!(BodyValue::Object(authority.binding().to_body_object()), {
            let encoded = serde_json::to_vec(&case.expected_manifest_binding).unwrap();
            parse(&encoded).unwrap()
        });
    }
}

#[test]
fn public_authority_missing_signal_stays_legacy() {
    assert_eq!(
        classify_bundle_directory(DirectoryObservation {
            name: b"legacy-directory",
            envelope_present: false,
            ledger_present: false,
            manifest: None,
        }),
        BundleClass::LegacyCandidate
    );
}

#[test]
fn public_authority_native_to_legacy_downgrade_on_signal_removal() {
    let native = DirectoryObservation {
        name: b"legacy-directory",
        envelope_present: true,
        ledger_present: false,
        manifest: None,
    };
    let legacy = DirectoryObservation {
        envelope_present: false,
        ..native
    };
    assert_eq!(
        classify_bundle_directory(native),
        BundleClass::NativeCandidate
    );
    assert_eq!(
        classify_bundle_directory(legacy),
        BundleClass::LegacyCandidate
    );
}

#[test]
fn public_authority_staging_precedence_survives_added_native_signals() {
    let observation = DirectoryObservation {
        name: b".body-staging-partial",
        envelope_present: true,
        ledger_present: true,
        manifest: Some(br#"{"body_source_schema":null}"#),
    };
    assert_eq!(
        classify_bundle_directory(observation),
        BundleClass::StagingExcluded
    );
    assert_authority_error(
        authorize_native_bundle(observation),
        AuthorityError::NotNativeCandidate,
    );
}

#[test]
fn public_authority_invalid_utf8_does_not_suppress_native_signal() {
    let observation = DirectoryObservation {
        name: b"body-\xff",
        envelope_present: false,
        ledger_present: false,
        manifest: None,
    };
    assert_eq!(
        classify_bundle_directory(observation),
        BundleClass::NativeCandidate
    );
    assert_authority_error(
        authorize_native_bundle(observation),
        AuthorityError::InvalidDirectory,
    );
}

#[test]
fn public_authority_classification_native_can_still_fail_validation() {
    let observation = DirectoryObservation {
        name: b"body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y",
        envelope_present: false,
        ledger_present: false,
        manifest: None,
    };
    assert_eq!(
        classify_bundle_directory(observation),
        BundleClass::NativeCandidate
    );
    assert_authority_error(
        authorize_native_bundle(observation),
        AuthorityError::MissingEnvelope,
    );
}

#[test]
fn public_authority_component_precedence_is_stable() {
    assert_authority_error(
        authorize_native_bundle(DirectoryObservation {
            name: b"body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y",
            envelope_present: false,
            ledger_present: false,
            manifest: None,
        }),
        AuthorityError::MissingEnvelope,
    );
    assert_authority_error(
        authorize_native_bundle(DirectoryObservation {
            name: b"body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y",
            envelope_present: true,
            ledger_present: false,
            manifest: None,
        }),
        AuthorityError::MissingLedger,
    );
}

#[test]
fn public_authority_errors_never_leak_raw_bytes() {
    let sentinel = "authority-public-private-sentinel";
    let name = format!("body-{sentinel}");
    let Err(error) = authorize_native_bundle(DirectoryObservation {
        name: name.as_bytes(),
        envelope_present: false,
        ledger_present: false,
        manifest: None,
    }) else {
        panic!("invalid directory should refuse");
    };
    assert_eq!(error, AuthorityError::InvalidDirectory);
    assert_eq!(
        error.to_string(),
        "body-authority: invalid directory <invalid>"
    );
    assert_eq!(error.to_string(), format!("{error:?}"));
    assert!(!error.to_string().contains(sentinel));
}

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

#[test]
fn public_manifest_binding_api_checks_and_emits_all_fields() {
    let digest_text = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let bundle_text = "body-00000000000000000000000000";
    let hash_text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let digest = BodyDigest::from_bytes(digest_text.as_bytes()).expect("digest is valid");
    let import_id = BundleId::from_bytes(bundle_text.as_bytes()).expect("bundle is valid");
    let source_hash =
        BodySourceHash::from_bytes_for_family(hash_text.as_bytes(), &BodySourceFamily::AppleHealth)
            .expect("hash is valid");
    let days = vec![
        BodyDay::from_bytes(b"20240228").expect("day is valid"),
        BodyDay::from_bytes(b"20240229").expect("leap day is valid"),
    ];
    let binding = BodyManifestBinding::new(
        digest.clone(),
        import_id.clone(),
        BodySourceFamily::AppleHealth,
        source_hash.clone(),
        2,
        days.clone(),
        BodyRawRetention::RetainParsed,
    )
    .expect("checked values bind");

    assert_eq!(binding.body_source_schema(), "solstone.body.bundle.v1");
    assert_eq!(binding.body_bundle_ref(), "body-bundle.json");
    assert_eq!(binding.body_bundle_sha256(), &digest);
    assert_eq!(binding.import_id(), &import_id);
    assert_eq!(binding.source_type(), BodySourceFamily::AppleHealth);
    assert_eq!(binding.source_hash(), &source_hash);
    assert_eq!(binding.entry_count(), 2);
    assert_eq!(binding.days_affected(), days.as_slice());
    assert_eq!(binding.raw_retention(), BodyRawRetention::RetainParsed);

    let object = binding.to_body_object();
    let expected_keys: BTreeSet<_> = [
        BODY_SOURCE_SCHEMA_KEY,
        BODY_BUNDLE_REF_KEY,
        BODY_BUNDLE_SHA256_KEY,
        IMPORT_ID_KEY,
        SOURCE_TYPE_KEY,
        SOURCE_HASH_KEY,
        ENTRY_COUNT_KEY,
        DAYS_AFFECTED_KEY,
        RAW_RETENTION_KEY,
    ]
    .into_iter()
    .map(|key| BodyString::from_code_points(key.bytes().map(u32::from).collect()).unwrap())
    .collect();
    assert_eq!(object.len(), 9);
    assert_eq!(
        object.keys().cloned().collect::<BTreeSet<_>>(),
        expected_keys
    );
    assert_eq!(
        object.get(
            &BodyString::from_code_points(ENTRY_COUNT_KEY.bytes().map(u32::from).collect())
                .unwrap()
        ),
        Some(&BodyValue::Integer(BodyInteger::from_u64(2)))
    );

    let family_relabel = BodyManifestBinding::new(
        digest.clone(),
        import_id.clone(),
        BodySourceFamily::OuraApi,
        source_hash.clone(),
        0,
        vec![],
        BodyRawRetention::Discard,
    );
    let Err(error) = family_relabel else {
        panic!("family relabeling must refuse");
    };
    assert_eq!(error.code(), ManifestBindingErrorCode::IncompatibleField);
    assert_eq!(error.field(), ManifestBindingErrorField::SourceHash);

    for invalid_days in [
        vec![
            BodyDay::from_bytes(b"20240229").unwrap(),
            BodyDay::from_bytes(b"20240228").unwrap(),
        ],
        vec![
            BodyDay::from_bytes(b"20240228").unwrap(),
            BodyDay::from_bytes(b"20240228").unwrap(),
        ],
    ] {
        let result = BodyManifestBinding::new(
            digest.clone(),
            import_id.clone(),
            BodySourceFamily::AppleHealth,
            source_hash.clone(),
            2,
            invalid_days,
            BodyRawRetention::Discard,
        );
        let Err(error) = result else {
            panic!("unordered days must refuse");
        };
        assert_eq!(error.code(), ManifestBindingErrorCode::InvalidField);
        assert_eq!(error.field(), ManifestBindingErrorField::DaysAffected);
    }

    let count_mismatch = BodyManifestBinding::new(
        digest.clone(),
        import_id.clone(),
        BodySourceFamily::AppleHealth,
        source_hash.clone(),
        0,
        vec![BodyDay::from_bytes(b"20240228").unwrap()],
        BodyRawRetention::Discard,
    );
    let Err(error) = count_mismatch else {
        panic!("count mismatch must refuse");
    };
    assert_eq!(error.code(), ManifestBindingErrorCode::IncompatibleField);
    assert_eq!(error.field(), ManifestBindingErrorField::DaysAffected);

    let oura_hash = BodySourceHash::from_bytes_for_family(
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".as_bytes(),
        &BodySourceFamily::OuraApi,
    )
    .expect("Oura hash is valid");
    let retention_mismatch = BodyManifestBinding::new(
        digest,
        import_id,
        BodySourceFamily::OuraApi,
        oura_hash,
        1,
        vec![BodyDay::from_bytes(b"20240228").unwrap()],
        BodyRawRetention::RetainComplete,
    );
    let Err(error) = retention_mismatch else {
        panic!("incompatible retention must refuse");
    };
    assert_eq!(error.code(), ManifestBindingErrorCode::IncompatibleField);
    assert_eq!(error.field(), ManifestBindingErrorField::RawRetention);

    // `new` returns only `Result<Self, _>`; an error leaves no binding value to observe.
}

#[test]
fn public_envelope_shard_api_checks_and_rejects_invalid_descriptors() {
    fn assert_traits<T: Clone + std::fmt::Debug + PartialEq + Eq>() {}

    let bundle = BundleId::from_bytes(b"body-00000000000000000000000000").expect("bundle is valid");
    let month = BodyMonth::from_bytes(b"2026-01").expect("month is valid");
    let digest = BodyDigest::from_bytes(
        b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .expect("digest is valid");
    let shard = EnvelopeShard::new(&bundle, 1, month.clone(), 2, 1, digest.clone())
        .expect("checked values bind");

    assert_traits::<EnvelopeShard>();
    assert_eq!(shard.path(), "normalized/2026-01.jsonl");
    assert_eq!(shard.month(), &month);
    assert_eq!(shard.bytes(), 2);
    assert_eq!(shard.rows(), 1);
    assert_eq!(shard.sha256(), &digest);
    assert_eq!(shard.clone(), shard);

    let field_different = EnvelopeShard::new(&bundle, 1, month.clone(), 3, 1, digest.clone())
        .expect("field-different descriptor binds");
    assert_ne!(shard, field_different);

    let rows_different = EnvelopeShard::new(&bundle, 1, month.clone(), 2, 2, digest.clone())
        .expect("rows-different descriptor binds");
    assert_ne!(shard, rows_different);

    let other_digest = BodyDigest::from_bytes(
        b"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )
    .expect("other digest is valid");
    let digest_different = EnvelopeShard::new(&bundle, 1, month.clone(), 2, 1, other_digest)
        .expect("digest-different descriptor binds");
    assert_ne!(shard, digest_different);

    let other_month = BodyMonth::from_bytes(b"2026-02").expect("other month is valid");
    let month_and_path_different =
        EnvelopeShard::new(&bundle, 1, other_month, 2, 1, digest.clone())
            .expect("month-different descriptor binds");
    assert_eq!(month_and_path_different.path(), "normalized/2026-02.jsonl");
    assert_ne!(shard, month_and_path_different);

    let empty_digest = BodyDigest::from_bytes(
        b"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    )
    .expect("empty digest is valid");
    let failures = [
        (
            EnvelopeShard::new(&bundle, 1, month.clone(), 0, 1, digest.clone()),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::ShardBytes,
        ),
        (
            EnvelopeShard::new(&bundle, 1, month.clone(), 1, 0, digest.clone()),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::ShardRows,
        ),
        (
            EnvelopeShard::new(&bundle, 1, month.clone(), 1, 2, digest.clone()),
            EnvelopeErrorCode::IncompatibleField,
            EnvelopeErrorField::ShardRows,
        ),
        (
            EnvelopeShard::new(&bundle, 1, month, 1, 1, empty_digest),
            EnvelopeErrorCode::IncompatibleField,
            EnvelopeErrorField::ShardSha256,
        ),
    ];
    for (result, code, field) in failures {
        let Err(error) = result else {
            panic!("invalid descriptor must refuse");
        };
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), field);
    }

    // `new` returns only `Result<Self, _>`; an error leaves no shard value to observe.
}

#[test]
fn public_envelope_ledger_api_checks_and_rejects_invalid_descriptors() {
    fn assert_traits<T: Clone + std::fmt::Debug + PartialEq + Eq>() {}

    let bundle = BundleId::from_bytes(b"body-00000000000000000000000000").expect("bundle is valid");
    let digest = BodyDigest::from_bytes(
        b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .expect("digest is valid");
    let ledger = EnvelopeLedger::new(&bundle, 1, 1, digest.clone()).expect("checked values bind");

    assert_traits::<EnvelopeLedger>();
    assert_eq!(ledger.path(), "body-ledger.jsonl");
    assert_eq!(ledger.bytes(), 1);
    assert_eq!(ledger.events(), 1);
    assert_eq!(ledger.sha256(), &digest);
    assert_eq!(ledger.clone(), ledger);

    let bytes_different =
        EnvelopeLedger::new(&bundle, 2, 1, digest.clone()).expect("bytes-different ledger binds");
    assert_ne!(ledger, bytes_different);

    let events_different =
        EnvelopeLedger::new(&bundle, 2, 2, digest.clone()).expect("events-different ledger binds");
    assert_ne!(bytes_different, events_different);

    let other_bundle =
        BundleId::from_bytes(b"body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ").expect("other bundle is valid");
    let same_ledger_other_bundle = EnvelopeLedger::new(&other_bundle, 1, 1, digest.clone())
        .expect("same ledger under another diagnostic bundle binds");
    assert_eq!(ledger, same_ledger_other_bundle);

    let other_digest = BodyDigest::from_bytes(
        b"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )
    .expect("other digest is valid");
    let digest_different =
        EnvelopeLedger::new(&bundle, 1, 1, other_digest).expect("digest-different ledger binds");
    assert_ne!(ledger, digest_different);

    let failures = [
        (
            EnvelopeLedger::new(&bundle, 0, 0, digest.clone()),
            EnvelopeErrorCode::IncompatibleField,
            EnvelopeErrorField::LedgerSha256,
        ),
        (
            EnvelopeLedger::new(&bundle, 1, 2, digest),
            EnvelopeErrorCode::IncompatibleField,
            EnvelopeErrorField::LedgerEvents,
        ),
    ];
    for (result, code, field) in failures {
        let Err(error) = result else {
            panic!("invalid descriptor must refuse");
        };
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), field);
    }

    // `new` returns only `Result<Self, _>`; an error leaves no ledger value to observe.
}

#[test]
fn public_apple_summary_plan_api_checks_and_rejects_unordered_days() {
    fn assert_traits<T: Clone + std::fmt::Debug + PartialEq + Eq>() {}

    let bundle = BundleId::from_bytes(b"body-00000000000000000000000000").expect("bundle is valid");
    let sorted = vec![
        BodyDay::from_bytes(b"20260102").expect("day is valid"),
        BodyDay::from_bytes(b"20260103").expect("day is valid"),
        BodyDay::from_bytes(b"20260201").expect("day is valid"),
    ];
    let plan = AppleSummaryPlan::new(&bundle, sorted.clone()).expect("ordered days bind");

    assert_traits::<AppleSummaryPlan>();
    assert_eq!(plan.schema(), "solstone.body.apple_day_summaries.v1");
    assert_eq!(plan.days(), sorted.as_slice());
    assert_eq!(plan.clone(), plan);

    let days_different =
        AppleSummaryPlan::new(&bundle, sorted[..2].to_vec()).expect("different ordered days bind");
    assert_ne!(plan, days_different);

    let other_bundle =
        BundleId::from_bytes(b"body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ").expect("other bundle is valid");
    let same_plan_other_bundle = AppleSummaryPlan::new(&other_bundle, sorted.clone())
        .expect("same plan under another diagnostic bundle binds");
    assert_eq!(plan, same_plan_other_bundle);

    let reverse = AppleSummaryPlan::new(
        &bundle,
        vec![sorted[2].clone(), sorted[1].clone(), sorted[0].clone()],
    );
    let duplicate = AppleSummaryPlan::new(
        &bundle,
        vec![sorted[0].clone(), sorted[1].clone(), sorted[1].clone()],
    );
    for result in [reverse, duplicate] {
        let Err(error) = result else {
            panic!("unordered days must refuse");
        };
        assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
        assert_eq!(error.field(), EnvelopeErrorField::SummaryDays);
    }

    // `new` returns only `Result<Self, _>`; an error leaves no summary plan value to observe.
}

#[test]
fn public_raw_manifest_decoder_is_checked_and_keeps_non_reserved_extensions_permissive() {
    let fixture = native_bundle_fixture();
    let manifest = &fixture["cases"][0]["manifest"];
    let bytes = serde_json::to_vec(manifest).expect("fixture manifest serializes");
    let bundle = BundleId::from_bytes(
        manifest["import_id"]
            .as_str()
            .expect("fixture import ID")
            .as_bytes(),
    )
    .expect("fixture import ID is valid");
    let binding = decode_body_manifest(&bytes, &bundle).expect("fixture manifest decodes");
    assert_eq!(binding.body_source_schema(), "solstone.body.bundle.v1");
    assert_eq!(binding.body_bundle_ref(), "body-bundle.json");
    assert_eq!(
        binding.body_bundle_sha256().as_str(),
        manifest["body_bundle_sha256"]
    );
    assert_eq!(binding.import_id(), &bundle);
    assert_eq!(binding.source_type().as_str(), manifest["source_type"]);
    assert_eq!(binding.source_hash().as_str(), manifest["source_hash"]);
    assert_eq!(
        binding.entry_count(),
        manifest["entry_count"].as_u64().unwrap()
    );
    assert_eq!(
        binding.days_affected().len(),
        manifest["days_affected"].as_array().unwrap().len()
    );
    assert_eq!(binding.raw_retention().as_str(), manifest["raw_retention"]);

    let mut permissive = manifest.clone();
    let object = permissive
        .as_object_mut()
        .expect("fixture manifest is an object");
    object.insert("ordinary".into(), serde_json::json!("body_x"));
    object.insert(
        "nested".into(),
        serde_json::json!({"body_x": true, "body_": false}),
    );
    let permissive_bytes = serde_json::to_vec(&permissive).expect("permissive manifest serializes");
    assert!(decode_body_manifest(&permissive_bytes, &bundle).is_ok());

    let duplicate = br#"{"raw_retention":null,"raw_retention":null,"body_source_schema":null,"body_source_schema":null}"#;
    let Err(duplicate_error) = decode_body_manifest(duplicate, &bundle) else {
        panic!("duplicate refuses");
    };
    assert_eq!(
        duplicate_error.code(),
        ManifestBindingErrorCode::DuplicateField
    );
    assert_eq!(
        duplicate_error.field(),
        ManifestBindingErrorField::BodySourceSchema
    );

    let unknown = br#"{"body_x":null}"#;
    let unknown_result = decode_body_manifest(unknown, &bundle);
    assert!(unknown_result.is_err());
    let Err(unknown_error) = unknown_result else {
        panic!("unknown field refuses as a binding error");
    };
    assert_eq!(unknown_error.code(), ManifestBindingErrorCode::UnknownField);
    assert_eq!(unknown_error.field(), ManifestBindingErrorField::Manifest);
}

#[test]
fn public_body_envelope_api_checks_and_rejects_invalid_aggregates() {
    fn assert_traits<T: Clone + std::fmt::Debug + PartialEq + Eq>() {}

    let bundle = BundleId::from_bytes(b"body-00000000000000000000000000").expect("bundle is valid");
    let digest = BodyDigest::from_bytes(
        b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .expect("digest is valid");
    let empty_digest = BodyDigest::from_bytes(
        b"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    )
    .expect("empty digest is valid");
    let days = vec![
        BodyDay::from_bytes(b"20260102").expect("day is valid"),
        BodyDay::from_bytes(b"20260103").expect("day is valid"),
        BodyDay::from_bytes(b"20260201").expect("day is valid"),
    ];
    let source_hash = BodySourceHash::from_bytes_for_family(
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#window:20260102:20260201",
        &BodySourceFamily::AppleHealth,
    )
    .expect("source hash is valid");
    let january = BodyMonth::from_bytes(b"2026-01").expect("month is valid");
    let february = BodyMonth::from_bytes(b"2026-02").expect("month is valid");
    let shards = vec![
        EnvelopeShard::new(&bundle, 0, january.clone(), 3, 2, digest.clone())
            .expect("January shard is valid"),
        EnvelopeShard::new(&bundle, 1, february.clone(), 1, 1, digest.clone())
            .expect("February shard is valid"),
    ];
    let ledger = EnvelopeLedger::new(&bundle, 3, 3, digest.clone()).expect("ledger is valid");
    let plan = AppleSummaryPlan::new(&bundle, days.clone()).expect("plan is valid");
    let envelope = BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::AppleHealth,
        source_hash.clone(),
        BodyRawRetention::RetainParsed,
        3,
        days.clone(),
        shards.clone(),
        ledger.clone(),
        Some(plan.clone()),
    )
    .expect("multi-month Apple envelope binds");

    assert_traits::<BodyEnvelope>();
    assert_eq!(envelope.schema(), "solstone.body.bundle.v1");
    assert_eq!(envelope.bundle_id(), &bundle);
    assert_eq!(envelope.source_family(), BodySourceFamily::AppleHealth);
    assert_eq!(envelope.source_hash(), &source_hash);
    assert_eq!(envelope.raw_retention(), BodyRawRetention::RetainParsed);
    assert_eq!(envelope.row_count(), 3);
    assert_eq!(envelope.days(), days.as_slice());
    assert_eq!(envelope.shards(), shards.as_slice());
    assert_eq!(envelope.ledger(), &ledger);
    assert_eq!(envelope.summary_plan(), Some(&plan));
    assert_eq!(envelope.clone(), envelope);

    let other_bundle =
        BundleId::from_bytes(b"body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ").expect("bundle is valid");
    let bundle_different = BodyEnvelope::new(
        other_bundle,
        BodySourceFamily::AppleHealth,
        source_hash.clone(),
        BodyRawRetention::RetainParsed,
        3,
        days.clone(),
        shards.clone(),
        ledger.clone(),
        Some(plan.clone()),
    )
    .expect("bundle-different envelope binds");
    assert_ne!(envelope, bundle_different);

    let other_hash = BodySourceHash::from_bytes_for_family(
        b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb#window:20260102:20260201",
        &BodySourceFamily::AppleHealth,
    )
    .expect("other source hash is valid");
    let hash_different = BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::AppleHealth,
        other_hash,
        BodyRawRetention::RetainParsed,
        3,
        days.clone(),
        shards.clone(),
        ledger.clone(),
        Some(plan.clone()),
    )
    .expect("hash-different envelope binds");
    assert_ne!(envelope, hash_different);

    let retention_different = BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::AppleHealth,
        source_hash.clone(),
        BodyRawRetention::Discard,
        3,
        days.clone(),
        shards.clone(),
        ledger.clone(),
        Some(plan.clone()),
    )
    .expect("retention-different envelope binds");
    assert_ne!(envelope, retention_different);

    let shards_different = vec![
        EnvelopeShard::new(&bundle, 0, january, 4, 2, digest.clone())
            .expect("other January shard is valid"),
        EnvelopeShard::new(&bundle, 1, february, 1, 1, digest.clone())
            .expect("other February shard is valid"),
    ];
    let shard_different = BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::AppleHealth,
        source_hash.clone(),
        BodyRawRetention::RetainParsed,
        3,
        days.clone(),
        shards_different,
        ledger.clone(),
        Some(plan.clone()),
    )
    .expect("shard-different envelope binds");
    assert_ne!(envelope, shard_different);

    let ledger_different =
        EnvelopeLedger::new(&bundle, 4, 3, digest.clone()).expect("other ledger is valid");
    let ledger_different = BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::AppleHealth,
        source_hash.clone(),
        BodyRawRetention::RetainParsed,
        3,
        days.clone(),
        shards.clone(),
        ledger_different,
        Some(plan.clone()),
    )
    .expect("ledger-different envelope binds");
    assert_ne!(envelope, ledger_different);

    let zero_bundle =
        BundleId::from_bytes(b"body-00000000000000000000000001").expect("bundle is valid");
    let zero_hash = BodySourceHash::from_bytes_for_family(
        b"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        &BodySourceFamily::OuraApi,
    )
    .expect("Oura hash is valid");
    let zero_envelope = BodyEnvelope::new(
        zero_bundle.clone(),
        BodySourceFamily::OuraApi,
        zero_hash.clone(),
        BodyRawRetention::Discard,
        0,
        vec![],
        vec![],
        EnvelopeLedger::new(&zero_bundle, 0, 0, empty_digest.clone()).expect("ledger is valid"),
        None,
    )
    .expect("zero-row Oura envelope binds");
    assert_eq!(zero_envelope.row_count(), 0);
    assert!(zero_envelope.days().is_empty());
    assert!(zero_envelope.shards().is_empty());
    assert_eq!(zero_envelope.source_family(), BodySourceFamily::OuraApi);
    assert_eq!(zero_envelope.source_hash(), &zero_hash);
    assert_eq!(zero_envelope.summary_plan(), None);
    assert_ne!(envelope, zero_envelope);

    let failures = [
        (
            BodyEnvelope::new(
                bundle.clone(),
                BodySourceFamily::AppleHealth,
                source_hash.clone(),
                BodyRawRetention::RetainParsed,
                3,
                vec![days[0].clone(), days[0].clone()],
                shards.clone(),
                ledger.clone(),
                Some(plan.clone()),
            ),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::Days,
        ),
        (
            BodyEnvelope::new(
                bundle.clone(),
                BodySourceFamily::AppleHealth,
                source_hash.clone(),
                BodyRawRetention::RetainParsed,
                3,
                days.clone(),
                vec![shards[1].clone(), shards[0].clone()],
                ledger.clone(),
                Some(plan.clone()),
            ),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::Shards,
        ),
        (
            BodyEnvelope::new(
                bundle,
                BodySourceFamily::AppleHealth,
                source_hash,
                BodyRawRetention::RetainParsed,
                3,
                days,
                shards,
                ledger,
                None,
            ),
            EnvelopeErrorCode::MissingField,
            EnvelopeErrorField::SummaryPlan,
        ),
    ];
    for (result, code, field) in failures {
        let Err(error) = result else {
            panic!("invalid aggregate must refuse");
        };
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), field);
    }

    // `new` returns only `Result<Self, _>`; an error leaves no envelope value to observe.
}

#[test]
fn public_body_envelope_encoder_encodes_the_independently_constructed_multimonth_case() {
    let case = &envelope_multimonth_fixture()["cases"][0];
    let binding = &case["expected_manifest_binding"];
    let bundle = BundleId::from_bytes(case["directory"].as_str().unwrap().as_bytes()).unwrap();
    let family =
        BodySourceFamily::from_bytes(binding["source_type"].as_str().unwrap().as_bytes()).unwrap();
    let days = binding["days_affected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|day| BodyDay::from_bytes(day.as_str().unwrap().as_bytes()).unwrap())
        .collect::<Vec<_>>();
    let expected = &case["expected_envelope"];
    let shards = case["digest_basis"]["shards"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, basis)| {
            let path = basis["path"].as_str().unwrap();
            let month = path
                .strip_prefix("normalized/")
                .unwrap()
                .strip_suffix(".jsonl")
                .unwrap();
            let text = basis["exact_bytes"].as_str().unwrap();
            EnvelopeShard::new(
                &bundle,
                index as u64,
                BodyMonth::from_bytes(month.as_bytes()).unwrap(),
                text.len() as u64,
                text.lines().count() as u64,
                BodyDigest::from_bytes(
                    expected["shards"][index]["sha256"]
                        .as_str()
                        .unwrap()
                        .as_bytes(),
                )
                .unwrap(),
            )
            .unwrap()
        })
        .collect();
    let ledger_text = case["digest_basis"]["ledger"]["exact_bytes"]
        .as_str()
        .unwrap();
    let envelope = BodyEnvelope::new(
        bundle.clone(),
        family,
        BodySourceHash::from_bytes_for_family(
            binding["source_hash"].as_str().unwrap().as_bytes(),
            &family,
        )
        .unwrap(),
        BodyRawRetention::from_bytes(binding["raw_retention"].as_str().unwrap().as_bytes())
            .unwrap(),
        binding["entry_count"].as_u64().unwrap(),
        days.clone(),
        shards,
        EnvelopeLedger::new(
            &bundle,
            ledger_text.len() as u64,
            ledger_text.lines().count() as u64,
            BodyDigest::from_bytes(expected["ledger"]["sha256"].as_str().unwrap().as_bytes())
                .unwrap(),
        )
        .unwrap(),
        Some(AppleSummaryPlan::new(&bundle, days).unwrap()),
    )
    .unwrap();

    assert_eq!(
        encode_body_envelope(&envelope).unwrap(),
        case["expected_envelope_jsonl"].as_str().unwrap().as_bytes()
    );
}

#[test]
fn public_body_envelope_decoder_exposes_checked_values_and_structured_errors() {
    let case = &native_bundle_fixture()["cases"][0];
    let input = case["expected_envelope_jsonl"].as_str().unwrap().as_bytes();
    let envelope = decode_body_envelope(input).expect("fixture envelope decodes publicly");
    assert_eq!(
        envelope.bundle_id().as_str(),
        case["directory"].as_str().unwrap()
    );
    assert_eq!(envelope.row_count(), 1);
    assert_eq!(encode_body_envelope(&envelope).unwrap(), input);

    let error = decode_body_envelope(b"null\n").expect_err("non-object envelope refuses");
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::Envelope);
    assert_eq!(error.bundle(), None);
    assert_eq!(error.index(), None);
}
