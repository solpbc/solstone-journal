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
    let open_ended = examples
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .unwrap()
        .into_iter()
        .filter(|value| value.contains("#window:open:") || value.ends_with(":open"))
        .collect::<Vec<_>>();

    for value in open_ended {
        let hash = SourceHash::new(value.to_owned());
        assert_eq!(hash.as_str(), value);
        assert_eq!(hash.into_inner(), value);
    }
}
