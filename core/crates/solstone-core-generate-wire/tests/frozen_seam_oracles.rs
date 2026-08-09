// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The seam oracles whose Python reference the conversion deletes, checked
//! against recorded answers rather than a live interpreter.
//!
//! Two cross-language differentials drove a Python function living in the half
//! of its module the generate conversion removes. When that half goes, a
//! differential and the thing it tested disappear together and nothing reds.
//! `scripts/generate_seam_oracles.py` observed the reference over a corpus
//! while it still ran; this asserts the Rust implementations against those
//! observations.
//!
//! ⚠ This target is deliberately NOT gated behind the `differential` feature.
//! It executes no interpreter, so it belongs in `make ci` -- which is the point
//! of freezing the answers in the first place.

use serde_json::{Value, json};
use solstone_core_generate_wire::{
    OverflowDecision, endpoint_overflow_decision, validate_schema_with_annotations,
};

const OVERFLOW_ORACLE: &str = include_str!("../../../fixtures/endpoint_overflow_oracle.json");
const SCHEMA_ORACLE: &str = include_str!("../../../fixtures/schema_validation_oracle.json");

fn cases(fixture: &str) -> Vec<Value> {
    let document: Value = serde_json::from_str(fixture).expect("oracle fixture parses");
    document
        .get("cases")
        .and_then(Value::as_array)
        .expect("oracle fixture has cases")
        .clone()
}

/// Messages intentionally differ between the Rust and Python jsonschema
/// implementations, and `observed_text` differs by permitted JSON formatting.
/// Path and constraint are the parts both sides must agree on.
fn comparable(validation: &Value) -> Value {
    let mut validation = validation
        .as_object()
        .expect("validation is an object")
        .clone();
    let errors = validation
        .get("errors")
        .and_then(Value::as_array)
        .expect("errors is an array")
        .iter()
        .map(|error| {
            let error = error.as_object().expect("error is an object");
            json!({"path": error["path"], "constraint": error["constraint"]})
        })
        .collect::<Vec<_>>();
    validation.insert("errors".to_owned(), Value::Array(errors));
    Value::Object(validation)
}

#[test]
fn overflow_decisions_match_the_frozen_oracle() {
    let cases = cases(OVERFLOW_ORACLE);
    assert!(
        cases.len() >= 21,
        "the frozen overflow corpus shrank to {} cases; it recorded 21 and the \
         reference it was read from is gone",
        cases.len()
    );

    for case in &cases {
        let name = case["name"].as_str().expect("case name");
        let body = case["body"].as_str().expect("case body");
        let served_window = case["served_window"].as_u64().map(|value| value as u32);
        let attempt = case["attempt"].as_u64().expect("attempt") as u32;

        let (kind, max_tokens) = match endpoint_overflow_decision(body, served_window, attempt) {
            OverflowDecision::Retry(max_tokens) => ("retry", Some(max_tokens)),
            OverflowDecision::Budget => ("budget", None),
            OverflowDecision::Context => ("context", None),
            OverflowDecision::Contract => ("contract", None),
        };

        assert_eq!(
            kind,
            case["kind"].as_str().expect("recorded kind"),
            "case={name}"
        );
        assert_eq!(
            max_tokens,
            case["max_tokens"].as_u64().map(|value| value as u32),
            "case={name}"
        );
    }
}

#[test]
fn schema_validation_matches_the_frozen_oracle() {
    let cases = cases(SCHEMA_ORACLE);
    assert!(
        cases.len() >= 27,
        "the frozen schema corpus shrank to {} cases; it recorded 27 and the \
         reference it was read from is gone",
        cases.len()
    );

    for case in &cases {
        let name = case["name"].as_str().expect("case name");
        let text = case["text"].as_str().expect("case text");
        let result = validate_schema_with_annotations(text, &case["schema"]);

        assert_eq!(
            comparable(&result.validation),
            comparable(&case["validation"]),
            "case={name}"
        );
    }
}

/// ⛔ The corpus is worthless if a later edit can quietly drop the cases that
/// carry the boundaries. These are the answers no live differential ever
/// pinned, and after the cut they cannot be re-derived.
#[test]
fn the_frozen_corpus_still_carries_its_boundaries() {
    let cases = cases(OVERFLOW_ORACLE);
    let named = |name: &str| -> Value {
        cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("the frozen corpus lost case {name}"))
            .clone()
    };

    // The reclamp minimum is a cliff: one token below it stops being a retry.
    assert_eq!(named("reclamp-exactly-at-minimum")["kind"], "retry");
    assert_eq!(named("reclamp-exactly-at-minimum")["max_tokens"], 256);
    assert_eq!(named("reclamp-one-below-minimum")["kind"], "budget");

    // A served window of zero is a limit, not an absent one -- `is not None`,
    // not truthiness. A port that tests truthiness passes every other case.
    assert_eq!(named("served-window-zero-is-a-limit")["kind"], "budget");

    // An explicit limit in the body outranks the configured window.
    assert_eq!(
        named("limit-phrase-outranks-served-window")["max_tokens"],
        384
    );

    for name in [
        "pattern-context-length-exceeded",
        "pattern-available-context-size",
        "pattern-model-context-length",
        "pattern-context-size-exceeded",
    ] {
        assert_eq!(named(name)["kind"], "context", "case={name}");
    }
}
