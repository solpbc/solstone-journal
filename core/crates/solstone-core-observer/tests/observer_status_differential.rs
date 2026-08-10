mod common;

use chrono::{Duration, Utc};
use serde_json::json;
use solstone_core_observer::store::format::{render_status_all, render_status_single};
use solstone_core_observer::store::reload::load_observers;

#[test]
fn status_all_and_single_match_python_json_and_human_with_history_fallback() {
    common::with_utc_tz(|| {
        status_all_and_single_match_python_json_and_human_with_history_fallback_inner()
    });
}

fn status_all_and_single_match_python_json_and_human_with_history_fallback_inner() {
    let root = common::root("status");
    let now = common::now_ms();
    let record = common::write_record(
        &root,
        json!({"key":"abcdefgh123", "name":"history-office", "created_at":null, "last_seen":now - 30_000, "last_segment":null, "last_segment_received_at":null, "last_segment_day":null, "enabled":true, "revoked":false, "revoked_at":null, "stats":{"segments_received":9,"bytes_received":2048}}),
    );
    let today = chrono::DateTime::from_timestamp_millis(now)
        .expect("timestamp")
        .format("%Y%m%d")
        .to_string();
    common::write_history(
        &root,
        &record.prefix(),
        &today,
        &[
            json!({"segment":"prune-then-reupload","files":[{"size":1}],"ts":now - 90_000}),
            json!({"segment":"prune-then-reupload","type":"pruned","ts":now - 80_000}),
            json!({"segment":"prune-then-reupload","files":[{"size":2}],"ts":now - 70_000}),
            json!({"segment":"second","files":[{"size":3}],"ts":now - 60_000}),
            json!({"segment":"third","files":[{"size":4}],"ts":now - 50_000}),
            json!({"segment":"fourth","files":[{"size":5}],"ts":now - 40_000}),
            json!({"segment":"boolean-ts","files":[{"size":6}],"ts":true}),
        ],
    );
    let anchor = chrono::DateTime::<Utc>::from_timestamp_millis(now)
        .expect("timestamp")
        .date_naive();
    for offset in 1..=8 {
        let day = (anchor - Duration::days(offset))
            .format("%Y%m%d")
            .to_string();
        common::write_history(
            &root,
            &record.prefix(),
            &day,
            &[json!({"segment":format!("old-{offset}"),"files":[],"ts":now - offset * 86_400_000})],
        );
    }
    let records = load_observers(&root).expect("records");
    for json_output in [false, true] {
        let all = render_status_all(&records, json_output, now);
        let all_oracle = common::oracle(&root, "status_all", json_output, now, None);
        assert_output_matches(&all, &all_oracle, json_output);
        let single = render_status_single(&root, &records[0], json_output, now);
        let single_oracle = common::oracle(
            &root,
            "status_single",
            json_output,
            now,
            Some("history-office"),
        );
        assert_output_matches(&single, &single_oracle, json_output);
        if !json_output {
            assert!(single.contains("Today ("));
            assert!(single.contains("Recent days:"));
            assert!(single.contains("boolean-ts"));
            assert!(single.contains("Last segment: —"));
        }
    }
    common::cleanup(root);
}

fn assert_output_matches(output: &str, oracle: &serde_json::Value, json_output: bool) {
    if json_output {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(output).expect("rust JSON"),
            serde_json::from_str::<serde_json::Value>(
                oracle["stdout"].as_str().expect("out").trim()
            )
            .expect("Python JSON")
        );
    } else {
        assert_eq!(
            format!("{output}\n"),
            oracle["stdout"].as_str().expect("out")
        );
    }
}
