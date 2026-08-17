// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::observer_render_support::{NOW_MS, write_history, write_record};
use serde_json::{Value, json};
use solstone_core_observer::store::format::{TimeDisplay, render_status_all, render_status_single};
use solstone_core_observer::store::reload::load_observers;

const EXPECTED_STATUS_ALL_HUMAN: &str = "\
Observers: 1 total
  Connected:    1
  Disconnected: 0
  Revoked:      0
  Total segments: 9
  Total bytes:    2.0 KB

Name                 Prefix             Status         Binding    Last Seen          Last Segment
--------------------------------------------------------------------------------------------------
history-office       abcdefgh           connected      unbound    2026-01-01 02:59   —           
";

const EXPECTED_STATUS_SINGLE_HUMAN: &str = "\
Observer: history-office
  Prefix:       abcdefgh
  Status:       connected
  Binding:      unbound
  Created:      never
  Last seen:    2026-01-01 02:59
  Last segment: —
  Segments:     9
  Bytes:        2.0 KB

  Today (20260101): 6 segment(s) synced
    prune-then-reupload  1 file(s)  2 B  2026-01-01 02:58
    second  1 file(s)  3 B  2026-01-01 02:59
    third  1 file(s)  4 B  2026-01-01 02:59
    fourth  1 file(s)  5 B  2026-01-01 02:59
    boolean-ts  1 file(s)  6 B  1970-01-01 00:00

  Recent days:
    20260101: 6 segment(s)
    20251231: 1 segment(s)
    20251230: 1 segment(s)
    20251229: 1 segment(s)
    20251228: 1 segment(s)
    20251227: 1 segment(s)
    20251226: 1 segment(s)
";

fn seed_status(root: &std::path::Path) {
    let record = write_record(
        root,
        json!({"key":"abcdefgh123", "name":"history-office", "created_at":null, "last_seen":NOW_MS - 30_000, "last_segment":null, "last_segment_received_at":null, "last_segment_day":null, "enabled":true, "revoked":false, "revoked_at":null, "stats":{"segments_received":9,"bytes_received":2048}}),
    );
    write_history(
        root,
        &record.prefix(),
        "20260101",
        &[
            json!({"segment":"prune-then-reupload","files":[{"size":1}],"ts":NOW_MS - 90_000}),
            json!({"segment":"prune-then-reupload","type":"pruned","ts":NOW_MS - 80_000}),
            json!({"segment":"prune-then-reupload","files":[{"size":2}],"ts":NOW_MS - 70_000}),
            json!({"segment":"second","files":[{"size":3}],"ts":NOW_MS - 60_000}),
            json!({"segment":"third","files":[{"size":4}],"ts":NOW_MS - 50_000}),
            json!({"segment":"fourth","files":[{"size":5}],"ts":NOW_MS - 40_000}),
            json!({"segment":"boolean-ts","files":[{"size":6}],"ts":true}),
        ],
    );
    for (offset, day) in [
        (1, "20251231"),
        (2, "20251230"),
        (3, "20251229"),
        (4, "20251228"),
        (5, "20251227"),
        (6, "20251226"),
        (7, "20251225"),
        (8, "20251224"),
    ] {
        write_history(
            root,
            &record.prefix(),
            day,
            &[
                json!({"segment":format!("old-{offset}"),"files":[],"ts":NOW_MS - offset * 86_400_000}),
            ],
        );
    }
}

#[test]
fn status_all_and_single_human_and_json_with_history_fallback() {
    let root = tempfile::tempdir().expect("journal");
    seed_status(root.path());
    let records = load_observers(root.path()).expect("records");
    let all_human = render_status_all(&records, false, NOW_MS, TimeDisplay::Utc);
    assert_eq!(
        format!("{all_human}\n"),
        EXPECTED_STATUS_ALL_HUMAN,
        "status_all human"
    );
    let all_json: Value =
        serde_json::from_str(&render_status_all(&records, true, NOW_MS, TimeDisplay::Utc))
            .expect("status_all json");
    assert_eq!(
        all_json,
        json!({
            "total": 1,
            "connected": 1,
            "disconnected": 0,
            "revoked": 0,
            "total_segments": 9,
            "total_bytes": 2048,
            "observers": [{
                "name": "history-office",
                "prefix": "abcdefgh",
                "status": "connected",
                "device_binding_kind": null,
                "last_seen": 1_767_236_370_000_i64,
                "last_segment_received_at": null,
                "last_segment_day": null
            }]
        }),
        "status_all json"
    );
    let single_human =
        render_status_single(root.path(), &records[0], false, NOW_MS, TimeDisplay::Utc);
    assert_eq!(
        format!("{single_human}\n"),
        EXPECTED_STATUS_SINGLE_HUMAN,
        "status_single human"
    );
    let single_json: Value = serde_json::from_str(&render_status_single(
        root.path(),
        &records[0],
        true,
        NOW_MS,
        TimeDisplay::Utc,
    ))
    .expect("status_single json");
    assert_eq!(
        single_json,
        json!({
            "name": "history-office",
            "prefix": "abcdefgh",
            "status": "connected",
            "device_binding_kind": null,
            "created_at": null,
            "last_seen": 1_767_236_370_000_i64,
            "last_segment_received_at": null,
            "last_segment_day": null,
            "revoked": false,
            "segments": 9,
            "bytes": 2048
        }),
        "status_single json"
    );
}
