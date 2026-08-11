// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_import::SourceHash;

const ORACLES: &str = include_str!("../../../fixtures/import_reference_oracles.json");

#[test]
fn source_hash_round_trips_open_ended_windows_from_fixture() {
    let fixture: Value = serde_json::from_str(ORACLES).unwrap();
    let examples = fixture["windowed_source_hash"]["examples"]
        .as_array()
        .unwrap();
    let values = examples
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .unwrap();
    let open_start = values
        .iter()
        .copied()
        .filter(|value| value.contains("#window:open:"))
        .collect::<Vec<_>>();
    let open_end = values
        .iter()
        .copied()
        .filter(|value| value.ends_with(":open"))
        .collect::<Vec<_>>();

    assert!(!open_start.is_empty());
    assert!(!open_end.is_empty());

    for value in open_start.into_iter().chain(open_end) {
        let hash = SourceHash::new(value.to_owned());
        assert_eq!(hash.as_str(), value);
        assert_eq!(hash.into_inner(), value);
    }
}
