// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::observer_render_support::{NOW_MS, seed_full_fixture};
use serde_json::{Value, json};
use solstone_core_observer::store::format::{TimeDisplay, render_list};
use solstone_core_observer::store::reload::load_observers;

#[test]
fn list_json_matches_seeded_fixture() {
    let root = tempfile::tempdir().expect("journal");
    seed_full_fixture(root.path());
    let records = load_observers(root.path()).expect("records");
    let rust: Value = serde_json::from_str(&render_list(&records, true, NOW_MS, TimeDisplay::Utc))
        .expect("rust JSON");
    assert_eq!(
        rust,
        json!([
            {
                "name": "revoked-never",
                "prefix": "cccccccc",
                "status": "revoked",
                "device_binding_kind": null,
                "last_seen": null,
                "last_segment_received_at": null,
                "last_segment_day": null,
                "segments": 4,
                "bytes": 4096
            },
            {
                "name": "unbound-stale",
                "prefix": "bbbbbbbb",
                "status": "disconnected",
                "device_binding_kind": null,
                "last_seen": 1_767_236_100_000_i64,
                "last_segment_received_at": null,
                "last_segment_day": null,
                "segments": 3,
                "bytes": 2048
            },
            {
                "name": "bound-live",
                "prefix": "aaaaaaaa",
                "status": "connected",
                "device_binding_kind": "cert",
                "last_seen": 1_767_236_370_000_i64,
                "last_segment_received_at": null,
                "last_segment_day": null,
                "segments": 2,
                "bytes": 1024
            }
        ]),
        "list json document"
    );
    let rust_names: Vec<_> = rust
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|record| record["name"].as_str())
        .collect();
    assert!(
        rust_names.contains(&"bound-live"),
        "bound-live must be listed"
    );
    assert!(
        rust_names.contains(&"unbound-stale"),
        "unbound-stale must be listed"
    );
    for excluded in [
        "fingerprint-rejected",
        "missing-key-rejected",
        "filename-rejected",
    ] {
        assert!(
            !rust_names.contains(&excluded),
            "{excluded} must be skipped"
        );
    }
}
