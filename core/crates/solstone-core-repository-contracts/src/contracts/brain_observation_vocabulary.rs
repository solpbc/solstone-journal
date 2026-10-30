// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[test]
fn observation_vocabulary_matches_the_writer_authorities() {
    let source = include_str!("../../../solstone-core-brain/src/observation.rs");
    let fixture = include_str!("../../../../fixtures/brain_observation_vocabulary.tsv");
    let actual: Vec<_> = fixture
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    let mut expected = Vec::new();
    for (field, name) in [
        ("caller", "ObservationCaller"),
        ("phase", "ObservationPhase"),
        ("outcome", "ObservationOutcome"),
        ("failure_category", "ObservationFailure"),
    ] {
        let body = source
            .split_once(&format!("pub enum {name} {{"))
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        for variant in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let variant = variant.strip_suffix(',').expect("unit variant");
            assert!(variant.chars().all(|c| c.is_ascii_alphanumeric()));
            let mut value = String::new();
            for (i, c) in variant.chars().enumerate() {
                if i != 0 && c.is_ascii_uppercase() {
                    value.push('_');
                }
                value.push(c.to_ascii_lowercase());
            }
            expected.push(format!("{field}\t{value}"));
        }
    }
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/local_contract.json")).unwrap();
    let contract = &contract["brain_state"];
    for reason in contract["reason_codes"].as_array().unwrap() {
        expected.push(format!("reason_code\t{}", reason.as_str().unwrap()));
    }
    let mut reasons = std::collections::BTreeSet::new();
    for schema in contract["diagnostic_metadata_schemas"]
        .as_object()
        .unwrap()
        .values()
    {
        if let Some(values) = schema
            .get("verification_reason")
            .and_then(serde_json::Value::as_array)
        {
            for value in values {
                reasons.insert(value.as_str().unwrap());
            }
        }
    }
    expected.extend(
        reasons
            .into_iter()
            .map(|reason| format!("verification_reason\t{reason}")),
    );
    assert_eq!(
        actual, expected,
        "regenerate the closed observation vocabulary"
    );
}
