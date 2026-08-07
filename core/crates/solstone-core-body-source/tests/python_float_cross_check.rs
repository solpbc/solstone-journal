// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Number, Value};
use solstone_core_body_source::{BodyValue, canonicalize};
use solstone_core_brain::{CanonicalInput, canonical_json};

#[test]
fn body_and_brain_match_the_pinned_python_float_matrix() {
    let cases = [
        (-0.0_f64, "-0.0"),
        (1.0_f64, "1.0"),
        (1e-7_f64, "1e-07"),
        (1e22_f64, "1e+22"),
        (1e-4_f64, "0.0001"),
        (1e-5_f64, "1e-05"),
        (1e15_f64, "1000000000000000.0"),
        (1e16_f64, "1e+16"),
        (5e-324_f64, "5e-324"),
        (1.7976931348623157e308_f64, "1.7976931348623157e+308"),
        (0.12345678901234568_f64, "0.12345678901234568"),
    ];

    for (value, expected) in cases {
        let body = canonicalize(&BodyValue::Number(value)).expect("body value should canonicalize");
        let brain = canonical_json(&CanonicalInput::Json(Value::Number(
            Number::from_f64(value).expect("finite float should be a JSON number"),
        )))
        .expect("brain value should canonicalize");
        assert_eq!(body, expected, "body source {value:?}");
        assert_eq!(brain, expected, "brain {value:?}");
        assert_eq!(body, brain, "{value:?}");
    }
}
