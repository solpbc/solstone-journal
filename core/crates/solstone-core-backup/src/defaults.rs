// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};

/// The complete backup section defaults. This is the sole constructor in this crate.
pub fn backup_defaults() -> Map<String, Value> {
    serde_json::from_value(json!({
        "enabled": false,
        "mode": "byo",
        "destination": {"repository": null, "backend": null, "credentials": {}},
        "daily_key": null,
        "recovery_key": null,
        "confirmed_recovery_key": false,
        "retention": {"hourly": 24, "daily": 7, "weekly": 4, "monthly": 12},
        "offload": {"enabled": false, "budget_bytes": null, "floor_bytes": null},
        "schedule": {"every": "daily", "enabled": false},
        "last_backup": {"time": null, "snapshot_id": null, "status": null, "error_reason": null},
        "last_prune": {"time": null, "status": null, "error_reason": null},
        "last_offload": {
            "time": null, "status": null, "reason": null, "last_ok_time": null,
            "files_marked": 0, "bytes_marked": 0, "ran_out_of_markable_media": false
        },
        "last_verification": {
            "time": null, "status": null, "reason": null, "last_ok_time": null,
            "checked_subset": null
        },
        "last_restore": {
            "time": null, "status": null, "reason": null, "scope": null, "day": null,
            "segments_selected": 0, "segments_restored": 0, "files_expected": 0,
            "files_restored": 0, "bytes_expected": 0, "bytes_restored": 0
        }
    }))
    .expect("backup defaults are valid JSON objects")
}
