// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use solstone_core_body_source::{
    BodyInteger, BodyString, BodyValue, ParseError, canonicalize, parse,
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
