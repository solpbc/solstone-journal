// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_import::stream_name::{
    StreamNameError, canonicalize_stream_name, import_stream_name,
};

const ORACLE: &str = include_str!("../../../fixtures/import_stream_name_oracle.json");

#[test]
fn vendored_stream_oracle_is_literal() {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    for case in oracle["cases"].as_object().unwrap().values() {
        assert_eq!(
            import_stream_name(case["import_source"].as_str().unwrap()).unwrap(),
            case["stream"].as_str().unwrap()
        );
    }
}

#[test]
fn canonicalisation_folds_separators_and_refuses_unsafe_names() {
    // not measured — derived from streams.py:98-116
    assert_eq!(
        canonicalize_stream_name("Import", Some("a / b\\c\n")).unwrap(),
        "import.a-b-c"
    );
    assert_eq!(
        canonicalize_stream_name("import", Some("a..b")),
        Err(StreamNameError::DoubleDot)
    );
    assert_eq!(
        canonicalize_stream_name("import", Some("é")),
        Err(StreamNameError::Invalid)
    );
}
