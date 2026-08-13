// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/fixtures.rs"]
mod fixtures;

use solstone_core_system_health::{sanitize_for_terminal, unsafe_ranges};

#[test]
fn pinned_fixture_and_all_unsafe_scalars_match_the_sanitizer() {
    fixtures::assert_fixture_shapes();
    let fixture = fixtures::health_text_fixture();
    let expected_ranges = fixture
        .unsafe_unicode
        .ranges
        .iter()
        .map(|range| (range.start, range.end))
        .collect::<Vec<_>>();
    assert_eq!(unsafe_ranges(), expected_ranges);
    let mut all = fixture
        .unsafe_unicode
        .categories
        .values()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 237);
    for scalar in all {
        let input = char::from_u32(scalar).unwrap().to_string();
        let expected = match scalar {
            10 => "\\n".into(),
            13 => "\\r".into(),
            9 => "\\t".into(),
            27 => "\\x1b".into(),
            _ => format!("\\u{{{scalar:x}}}"),
        };
        assert_eq!(sanitize_for_terminal(&input), expected, "U+{scalar:04X}");
    }
    for range in &fixture.unsafe_unicode.ranges {
        for scalar in [range.lower, range.upper].into_iter().flatten() {
            let input = char::from_u32(scalar).unwrap().to_string();
            assert_eq!(
                sanitize_for_terminal(&input),
                input,
                "safe neighbor U+{scalar:04X}"
            );
        }
    }
}

#[test]
fn fixture_digest_and_unknown_field_guards_hold() {
    assert_eq!(
        fixtures::health_text_raw_sha256(),
        fixtures::HEALTH_TEXT_SHA256
    );
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/health_text_reference.json")).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), serde_json::json!(true));
    assert!(fixtures::parse_health_text_fixture(&serde_json::to_string(&value).unwrap()).is_err());
}

fn parsed_mutation(value: &serde_json::Value) -> Result<(), &'static str> {
    let fixture = fixtures::parse_health_text_fixture(&serde_json::to_string(value).unwrap())
        .map_err(|_| "schema")?;
    fixtures::validate_health_text_fixture(&fixture)
}

#[test]
fn below_digest_semantic_mutations_reach_their_own_guards() {
    let original: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/health_text_reference.json")).unwrap();

    let mut provenance = original.clone();
    provenance["provenance"]["service_source"]["sha256"] = serde_json::json!("0".repeat(64));
    assert_eq!(parsed_mutation(&provenance), Err("provenance"));

    let mut runtime = original.clone();
    runtime["runtime"]["unicode"] = serde_json::json!("15.1.0");
    assert_eq!(parsed_mutation(&runtime), Err("runtime"));

    let mut reclassified = original.clone();
    let cc = reclassified["unsafe_unicode"]["categories"]["Cc"][0].clone();
    let cf = reclassified["unsafe_unicode"]["categories"]["Cf"][0].clone();
    reclassified["unsafe_unicode"]["categories"]["Cc"][0] = cf;
    reclassified["unsafe_unicode"]["categories"]["Cf"][0] = cc;
    assert_eq!(parsed_mutation(&reclassified), Err("unicode-category"));

    let mut shortened_range = original.clone();
    shortened_range["unsafe_unicode"]["ranges"][0]["end"] = serde_json::json!(30);
    assert_eq!(parsed_mutation(&shortened_range), Err("unicode-range"));

    let mut omitted = original.clone();
    omitted["unsafe_unicode"]["categories"]["Cf"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert_eq!(parsed_mutation(&omitted), Err("unicode-category"));

    let mut ignored_result_field = original.clone();
    ignored_result_field["scalar_cases"][0]["result"]["ignored"] = serde_json::json!(true);
    assert_eq!(parsed_mutation(&ignored_result_field), Err("schema"));
}

#[test]
fn integer_codepoint_arms_preserve_surrogates_but_scalar_arms_reject_them() {
    let fixture = fixtures::health_text_fixture();
    assert!(fixture.scalar_cases.iter().any(|case| {
        matches!(&case.recipe, fixtures::ScalarRecipe::Codepoints { values } if values == &[0xd800])
    }));
    assert!(fixture.port_cases.iter().any(|case| {
        matches!(&case.result, fixtures::PortResult::Exit { stderr_codepoints, .. } if stderr_codepoints.contains(&0xd800))
    }));
    assert!(fixture.port_cases.iter().any(|case| {
        matches!(&case.result, fixtures::PortResult::Exit { stderr_codepoints, .. } if stderr_codepoints.contains(&0xdcff))
    }));
    fixtures::validate_health_text_fixture(fixture).unwrap();

    let original: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/health_text_reference.json")).unwrap();
    let mut out_of_range = original.clone();
    let codepoints = out_of_range["scalar_cases"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|case| case["recipe"]["kind"] == "codepoints")
        .unwrap();
    codepoints["recipe"]["values"] = serde_json::json!([0x110000]);
    assert_eq!(parsed_mutation(&out_of_range), Err("scalar-codepoints"));

    let mut negative = original.clone();
    let codepoints = negative["scalar_cases"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|case| case["recipe"]["kind"] == "codepoints")
        .unwrap();
    codepoints["recipe"]["values"] = serde_json::json!([-1]);
    assert_eq!(parsed_mutation(&negative), Err("schema"));

    let mut scalar_surrogate = original;
    let repeat = scalar_surrogate["scalar_cases"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|case| case["recipe"]["kind"] == "repeat")
        .unwrap();
    repeat["recipe"]["codepoint"] = serde_json::json!(0xd800);
    assert_eq!(parsed_mutation(&scalar_surrogate), Err("scalar-repeat"));
}
