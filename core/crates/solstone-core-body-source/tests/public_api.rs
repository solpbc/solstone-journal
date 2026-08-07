// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use solstone_core_body_source::{BodyInteger, BodyString, BodyValue, ParseError, parse};

fn codec_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../core/fixtures/body_source_codec_rows.json")
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

    let fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(codec_fixture_path()).expect("fixture should read"),
    )
    .expect("fixture should parse");
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
