// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::observer_render_support::{NOW_MS, snapshot, write_record};
use serde_json::json;
use solstone_core_observer::store::reconcile::reconcile_plan;
use solstone_core_observer::store::reload::load_observers;

#[test]
fn reconcile_dry_run_plan_across_duplicate_edge_cases_without_writes() {
    let root = tempfile::tempdir().expect("journal");
    write_record(
        root.path(),
        json!({"key":"aaaaaaaa111","name":"triple","created_at":NOW_MS-3000,"stats":{"segments_received":1,"bytes_received":10,"duplicates_rejected":true,"note":"ignored"}}),
    );
    write_record(
        root.path(),
        json!({"key":"bbbbbbbb222","name":"triple","created_at":NOW_MS-2000,"stats":{"segments_received":2,"bytes_received":20,"duplicates_rejected":2,"ignored":false}}),
    );
    write_record(
        root.path(),
        json!({"key":"cccccccc333","name":"triple","created_at":NOW_MS-1000,"stats":{"segments_received":4,"bytes_received":40,"fraction":1.5}}),
    );
    write_record(
        root.path(),
        json!({"key":"dddddddd444","name":"single","created_at":NOW_MS-500,"stats":{"segments_received":99}}),
    );
    write_record(
        root.path(),
        json!({"key":"eeeeeeee555","name":"revoked-same","created_at":NOW_MS-400,"revoked":true,"stats":{"segments_received":1}}),
    );
    write_record(
        root.path(),
        json!({"key":"ffffffff666","name":"revoked-same","created_at":NOW_MS-300,"revoked":true,"stats":{"segments_received":2}}),
    );
    write_record(
        root.path(),
        json!({"key":"gggggggg777","name":"","created_at":NOW_MS-250,"stats":{"segments_received":3,"bytes_received":30}}),
    );
    write_record(
        root.path(),
        json!({"key":"hhhhhhhh888","name":"","created_at":NOW_MS-150,"stats":{"segments_received":5,"bytes_received":50}}),
    );
    let before = snapshot(root.path());
    let records = load_observers(root.path()).expect("records");
    let rust = serde_json::to_value(
        reconcile_plan(&records)
            .iter()
            .map(|entry| {
                json!({
                    "name": entry.name,
                    "survivor_prefix": entry.survivor_prefix,
                    "revoked_prefixes": entry.revoked_prefixes,
                    "stats": entry.stats
                })
            })
            .collect::<Vec<_>>(),
    )
    .expect("value");
    assert_eq!(
        rust,
        json!([
            {
                "name": "",
                "survivor_prefix": "gggggggg",
                "revoked_prefixes": ["hhhhhhhh"],
                "stats": {"segments_received": 8, "bytes_received": 80}
            },
            {
                "name": "triple",
                "survivor_prefix": "aaaaaaaa",
                "revoked_prefixes": ["cccccccc", "bbbbbbbb"],
                "stats": {
                    "segments_received": 7,
                    "bytes_received": 70,
                    "fraction": 1.5,
                    "duplicates_rejected": 2
                }
            }
        ]),
        "reconcile dry-run plan"
    );
    let names: Vec<_> = rust
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(names.contains(&"triple"), "triple group");
    assert!(names.contains(&""), "empty-name group");
    assert!(!names.contains(&"single"), "single must be omitted");
    assert!(
        !names.contains(&"revoked-same"),
        "revoked-same must be omitted"
    );
    let triple = rust
        .as_array()
        .expect("array")
        .iter()
        .find(|entry| entry["name"] == "triple")
        .expect("triple");
    assert_eq!(
        triple["stats"]["segments_received"], 7,
        "triple segments_received"
    );
    assert_eq!(
        triple["stats"]["bytes_received"], 70,
        "triple bytes_received"
    );
    assert_eq!(
        triple["stats"]["duplicates_rejected"], 2,
        "triple duplicates_rejected"
    );
    assert!(
        triple["stats"].get("note").is_none(),
        "triple drops non-numeric note"
    );
    assert!(
        triple["stats"].get("ignored").is_none(),
        "triple drops boolean ignored"
    );
    assert_eq!(snapshot(root.path()), before, "no-writes snapshot");
}
