// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_body_source::{BodyInteger, BodyString, BodyValue, ParseError, parse};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../core/fixtures/body_source_python_json_vectors.json")
}

fn vectors() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_path()).expect("fixture should read"))
        .expect("fixture should parse")
}

fn expand(pattern: &Value) -> String {
    let prefix = pattern
        .get("prefix")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let repeat = pattern
        .get("repeat")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let count = pattern["repeat_count"].as_u64().expect("repeat count") as usize;
    let suffix = pattern
        .get("suffix")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{prefix}{}{suffix}", repeat.repeat(count))
}

fn expect_malformed(error: ParseError, offset: usize) {
    assert_eq!(
        error,
        ParseError::MalformedJson {
            byte_offset: offset
        }
    );
}

fn nth_container_opener_offset(text: &str, ordinal: usize) -> usize {
    text.bytes()
        .enumerate()
        .filter(|(_, byte)| matches!(byte, b'[' | b'{'))
        .map(|(offset, _)| offset)
        .nth(ordinal - 1)
        .expect("nested test should contain the requested opener")
}

#[test]
fn canonical_strings_and_object_structure_match_python() {
    let fixture = vectors();
    for case in fixture["canonical_cases"]
        .as_array()
        .expect("canonical cases")
    {
        parse(case["raw_json"].as_str().expect("raw JSON").as_bytes()).expect("case should parse");
    }

    let duplicate = parse(
        fixture["canonical_cases"][0]["raw_json"]
            .as_str()
            .expect("raw JSON")
            .as_bytes(),
    )
    .expect("duplicate case should parse");
    let BodyValue::Object(object) = duplicate else {
        panic!("expected object");
    };
    assert_eq!(object.len(), 2);
    assert_eq!(
        object[&BodyString::from_code_points(vec![u32::from(b'a')]).unwrap()],
        BodyValue::Integer(BodyInteger::new(false, "2").unwrap())
    );
    let BodyValue::Object(nested) = &object[&BodyString::from_code_points(vec![
        u32::from(b'n'),
        u32::from(b'e'),
        u32::from(b's'),
        u32::from(b't'),
        u32::from(b'e'),
        u32::from(b'd'),
    ])
    .unwrap()] else {
        panic!("expected nested object");
    };
    assert_eq!(nested.len(), 1);
    assert_eq!(
        nested.values().next(),
        Some(&BodyValue::Integer(BodyInteger::new(false, "3").unwrap()))
    );

    let edges = parse(
        fixture["canonical_cases"][1]["raw_json"]
            .as_str()
            .expect("raw JSON")
            .as_bytes(),
    )
    .expect("string edge case should parse");
    let BodyValue::Array(values) = edges else {
        panic!("expected array");
    };
    assert_eq!(values.len(), 16);
    assert_eq!(
        values[11],
        BodyValue::String(BodyString::from_code_points(vec![0xd800]).unwrap())
    );
    assert_eq!(
        values[14],
        BodyValue::String(BodyString::from_code_points(vec![0xd800, 0xd800]).unwrap())
    );

    for case in fixture["string_decode_cases"]
        .as_array()
        .expect("string cases")
    {
        let value = parse(case["raw_json"].as_str().expect("raw JSON").as_bytes())
            .expect("string should parse");
        let BodyValue::String(string) = value else {
            panic!("expected string");
        };
        let expected = case["expected_python_code_points_hex"]
            .as_array()
            .expect("code points")
            .iter()
            .map(|point| {
                u32::from_str_radix(
                    point.as_str().expect("hex point").trim_start_matches("0x"),
                    16,
                )
                .expect("valid hex")
            })
            .collect::<Vec<_>>();
        assert_eq!(string.code_points(), expected);
    }
}

#[test]
fn ordered_strings_and_objects_use_code_points_not_utf16() {
    let strings = [
        BodyString::from_code_points(vec![]).unwrap(),
        BodyString::from_code_points(vec![u32::from(b'a')]).unwrap(),
        BodyString::from_code_points(vec![u32::from(b'a'), u32::from(b'a')]).unwrap(),
        BodyString::from_code_points(vec![u32::from(b'a'), u32::from(b'b')]).unwrap(),
        BodyString::from_code_points(vec![u32::from(b'b')]).unwrap(),
    ];
    assert!(strings.windows(2).all(|pair| pair[0] < pair[1]));
    let ordered = [0xd800, 0xdfff, 0xe000, 0x1fac0]
        .into_iter()
        .map(|point| BodyString::from_code_points(vec![point]).unwrap())
        .collect::<Vec<_>>();
    assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]));
    let equal_prefix = [0xd800, 0xe000, 0x1fac0]
        .into_iter()
        .map(|tail| BodyString::from_code_points(vec![u32::from(b'a'), tail]).unwrap())
        .collect::<Vec<_>>();
    assert!(equal_prefix.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        BodyString::from_code_points(vec![0xdfff]).unwrap()
            < BodyString::from_code_points(vec![0x1fac0]).unwrap()
    );
    let astral_first_utf16 = "🫀".encode_utf16().next().expect("astral UTF-16 unit");
    assert!(
        astral_first_utf16 < 0xdfff_u16,
        "UTF-16 would rank the astral leading unit first"
    );

    let texts = [
        r#"{"\ud800":1,"\udfff":2,"🫀":3,"a":4,"":5}"#,
        r#"{"":5,"a":4,"🫀":3,"\udfff":2,"\ud800":1}"#,
        r#"{"🫀":3,"\ud800":1,"":5,"\udfff":2,"a":4}"#,
    ];
    let expected = vec![
        vec![0x61],
        vec![0xd800],
        vec![0xdfff],
        vec![0xe000],
        vec![0x1fac0],
    ];
    for text in texts {
        let BodyValue::Object(object) = parse(text.as_bytes()).expect("object should parse") else {
            panic!("expected object");
        };
        assert_eq!(
            object
                .keys()
                .map(|key| key.code_points().to_vec())
                .collect::<Vec<_>>(),
            expected
        );
    }
    let BodyValue::Array(array) = parse(br#"[3,2,1]"#).expect("array should parse") else {
        panic!("expected array");
    };
    assert_eq!(
        array,
        vec![
            BodyValue::Integer(BodyInteger::new(false, "3").unwrap()),
            BodyValue::Integer(BodyInteger::new(false, "2").unwrap()),
            BodyValue::Integer(BodyInteger::new(false, "1").unwrap())
        ]
    );
}

#[test]
fn float_and_integer_vectors_preserve_classification_and_bits() {
    let fixture = vectors();
    for case in fixture["float_cases"].as_array().expect("float cases") {
        let BodyValue::Number(number) =
            parse(case["raw_json"].as_str().expect("raw JSON").as_bytes())
                .expect("float should parse")
        else {
            panic!("expected number");
        };
        let bits = u64::from_str_radix(
            case["expected_f64_bits_hex"]
                .as_str()
                .expect("bits")
                .trim_start_matches("0x"),
            16,
        )
        .expect("valid bits");
        assert_eq!(number.to_bits(), bits, "{}", case["name"]);
    }
    for case in fixture["long_numeric_cases"]
        .as_array()
        .expect("long numeric cases")
    {
        let text = expand(&case["raw_pattern"]);
        match case["expected_kind"].as_str().expect("kind") {
            "number" => {
                let BodyValue::Number(number) =
                    parse(text.as_bytes()).expect("number should parse")
                else {
                    panic!("expected float");
                };
                let bits = u64::from_str_radix(
                    case["expected_f64_bits_hex"]
                        .as_str()
                        .expect("bits")
                        .trim_start_matches("0x"),
                    16,
                )
                .expect("valid bits");
                assert_eq!(number.to_bits(), bits, "{}", case["name"]);
            }
            "integer" => {
                let BodyValue::Integer(integer) =
                    parse(text.as_bytes()).expect("integer should parse")
                else {
                    panic!("expected integer");
                };
                assert_eq!(integer.digits().len(), 4300);
                assert_eq!(
                    integer.is_negative(),
                    case["name"].as_str().expect("name").contains("negative")
                );
            }
            _ => panic!("unexpected kind"),
        }
    }

    let BodyValue::Array(edges) = parse(
        fixture["canonical_cases"][3]["raw_json"]
            .as_str()
            .expect("raw JSON")
            .as_bytes(),
    )
    .expect("numeric edges should parse") else {
        panic!("expected array");
    };
    assert_eq!(
        edges[0],
        BodyValue::Integer(BodyInteger::new(false, "0").unwrap())
    );
    assert!(matches!(edges[1], BodyValue::Number(number) if number == 1.0));
    assert!(
        matches!(edges[4], BodyValue::Number(number) if number.is_infinite() && !number.is_sign_negative())
    );
    assert!(
        matches!(edges[5], BodyValue::Number(number) if number.is_infinite() && number.is_sign_negative())
    );
    assert!(
        matches!(edges[7], BodyValue::Number(number) if number == 0.0 && number.is_sign_negative())
    );
    assert!(matches!(edges[10], BodyValue::Number(number) if number.is_nan()));
    assert_eq!(
        edges[8],
        BodyValue::Integer(BodyInteger::new(false, "18446744073709551616").unwrap())
    );
    assert_eq!(
        edges[9],
        BodyValue::Integer(BodyInteger::new(true, "9223372036854775809").unwrap())
    );
}

#[test]
fn malformed_and_policy_vectors_report_byte_offsets() {
    let fixture = vectors();
    for case in fixture["malformed_cases"]
        .as_array()
        .expect("malformed cases")
    {
        let error = parse(case["raw_json"].as_str().expect("raw JSON").as_bytes())
            .expect_err("case should fail");
        expect_malformed(
            error,
            case["expected_byte_offset"].as_u64().expect("offset") as usize,
        );
    }
    for case in fixture["policy_cases"]["invalid_utf8"]
        .as_array()
        .expect("invalid UTF-8 cases")
    {
        let hex = case["raw_hex"].as_str().expect("hex");
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex byte"))
            .collect::<Vec<_>>();
        let error = parse(&bytes).expect_err("invalid UTF-8 should fail");
        expect_malformed(
            error,
            case["expected_byte_offset"].as_u64().expect("offset") as usize,
        );
    }
    let too_deep = &fixture["policy_cases"]["too_deep"];
    let pattern = &too_deep["raw_pattern"];
    let text = format!(
        "{}{}",
        pattern["prefix_repeat"]
            .as_str()
            .expect("prefix")
            .repeat(pattern["repeat_count"].as_u64().expect("count") as usize),
        pattern["suffix_repeat"]
            .as_str()
            .expect("suffix")
            .repeat(pattern["repeat_count"].as_u64().expect("count") as usize)
    );
    expect_malformed(
        parse(text.as_bytes()).expect_err("too deep should fail"),
        128,
    );

    for (text, offset) in [
        ("\u{feff}null", 0),
        ("//", 0),
        ("/* */", 0),
        ("[1,]", 3),
        (r#"{"a":1,}"#, 7),
        ("01", 1),
        ("-01", 2),
        ("1.", 1),
        (".1", 0),
        ("1e", 1),
        ("1e+", 1),
        ("--1", 0),
        ("\x0cnull", 0),
        ("\x0bnull", 0),
        ("\u{a0}null", 0),
    ] {
        expect_malformed(
            parse(text.as_bytes()).expect_err("case should fail"),
            offset,
        );
    }
    expect_malformed(
        parse(&[0xff, 0xfe, b'n']).expect_err("UTF-16 BOM is invalid UTF-8"),
        0,
    );
    expect_malformed(
        parse(&[0xff, 0xfe, 0, 0, b'n']).expect_err("UTF-32 BOM is invalid UTF-8"),
        0,
    );
    assert_eq!(
        parse(b" \t\n\rnull \t\n\r").expect("whitespace should be accepted"),
        BodyValue::Null
    );

    let bmp = parse("\"é\"[]".as_bytes()).expect_err("extra data should fail");
    let astral = parse("\"🫀\"[]".as_bytes()).expect_err("extra data should fail");
    expect_malformed(bmp, 4);
    expect_malformed(astral, 6);
    assert_ne!(6, 3, "byte offsets must not use Python character offsets");
}

#[test]
fn integer_limits_depth_and_drop_are_safe() {
    let fixture = vectors();
    for case in fixture["policy_cases"]["too_long_integers"]
        .as_array()
        .expect("too long cases")
    {
        let text = expand(&case["raw_pattern"]);
        assert_eq!(
            parse(text.as_bytes()).expect_err("integer should be too long"),
            ParseError::NumberTooLong {
                byte_offset: case["expected_byte_offset"].as_u64().expect("offset") as usize
            }
        );
    }
    assert_eq!(BodyInteger::new(true, "0"), BodyInteger::new(false, "0"));

    let array_128 = format!("{}0{}", "[".repeat(128), "]".repeat(128));
    parse(array_128.as_bytes()).expect("128 arrays should parse");
    let object_128 = (0..128).fold("0".to_owned(), |inner, _| format!(r#"{{"a":{inner}}}"#));
    parse(object_128.as_bytes()).expect("128 objects should parse");
    let alternating_128 = (0..128).fold("0".to_owned(), |inner, index| {
        if index % 2 == 0 {
            format!("[{inner}]")
        } else {
            format!(r#"{{"a":{inner}}}"#)
        }
    });
    parse(alternating_128.as_bytes()).expect("128 alternating containers should parse");
    let deep = format!("{}0{}", "[".repeat(129), "]".repeat(129));
    expect_malformed(
        parse(deep.as_bytes()).expect_err("129 arrays should fail"),
        128,
    );
    let object_129 = (0..129).fold("0".to_owned(), |inner, _| format!(r#"{{"a":{inner}}}"#));
    expect_malformed(
        parse(object_129.as_bytes()).expect_err("129 objects should fail"),
        nth_container_opener_offset(&object_129, 129),
    );
    let alternating_129 = (0..129).fold("0".to_owned(), |inner, index| {
        if index % 2 == 0 {
            format!("[{inner}]")
        } else {
            format!(r#"{{"a":{inner}}}"#)
        }
    });
    expect_malformed(
        parse(alternating_129.as_bytes()).expect_err("129 alternating containers should fail"),
        nth_container_opener_offset(&alternating_129, 129),
    );

    let within_limit = format!("{}0{}", "[".repeat(127), "]".repeat(127));
    drop(parse(within_limit.as_bytes()).expect("deep value should parse and drop"));
}
