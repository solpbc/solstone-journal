// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/fixtures.rs"]
mod fixtures;

use solstone_core_system_health::sanitize_for_terminal;

#[test]
fn pinned_fixture_and_all_unsafe_scalars_match_the_sanitizer() {
    fixtures::assert_fixture_shapes();
    let fixture = fixtures::health_text_fixture();
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
