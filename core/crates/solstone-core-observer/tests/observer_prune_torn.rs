// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::observer_render_support::{
    seed_observer_owning_stream, segment_dir, write_history, write_segment,
};
use serde_json::json;
use solstone_core_observer::store::paths::history_path;
use solstone_core_observer::store::prune::run_prune;
use std::fs;

const DAY: &str = "20260101";
const STREAM: &str = "workstation";
const PREFIX: &str = "abcdefgh";

fn seed_duplicates(root: &std::path::Path) {
    seed_observer_owning_stream(root, PREFIX, STREAM);
    write_segment(root, DAY, STREAM, "090000_300", 1, None, b"same bytes");
    write_segment(
        root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
}

#[test]
fn execute_plan_refuses_torn_day_without_mutating() {
    let root = tempfile::tempdir().expect("journal");
    seed_duplicates(root.path());
    let hist = history_path(root.path(), PREFIX, DAY);
    let before = "{\"segment\":\"090000_300\",\"stream\":\"workstation\"}\n{broken}\n";
    fs::create_dir_all(hist.parent().unwrap()).unwrap();
    fs::write(&hist, before).unwrap();
    let candidate = segment_dir(root.path(), DAY, STREAM, "090000_301");
    assert!(candidate.is_dir());
    let result = run_prune(root.path(), &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert!(
        result
            .refusals
            .iter()
            .any(|refusal| refusal.gate == "sync-history"),
        "{:?}",
        result.refusals
    );
    assert!(result.deleted.is_empty());
    assert_eq!(fs::read_to_string(&hist).unwrap(), before);
    assert!(candidate.is_dir());

    fs::write(
        &hist,
        "{\"segment\":\"090000_300\",\"stream\":\"workstation\"}\n",
    )
    .unwrap();
    let clean = run_prune(root.path(), &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert!(
        clean
            .refusals
            .iter()
            .all(|refusal| refusal.gate != "sync-history"),
        "{:?}",
        clean.refusals
    );
    assert!(!candidate.is_dir());
    let after = fs::read_to_string(&hist).unwrap();
    assert!(after.contains("\"type\":\"pruned\"") || after.contains("pruned"));
}

#[cfg(unix)]
#[test]
fn execute_refuses_unrepresentable_stream_before_crash_repair_writes() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let root = tempfile::tempdir().expect("journal");
    seed_observer_owning_stream(root.path(), PREFIX, STREAM);
    let survivor = write_segment(
        root.path(),
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"bytes",
    );
    write_history(
        root.path(),
        PREFIX,
        DAY,
        &[json!({
            "type": "pruned",
            "ts": 1,
            "segment": "090000_300",
            "stream": STREAM,
            "duplicate_of": "",
        })],
    );
    let unreadable = root
        .path()
        .join("chronicle")
        .join(DAY)
        .join(OsStr::from_bytes(b"s\xff"))
        .join("080000_60");
    fs::create_dir_all(&unreadable).unwrap();
    let marker_before = fs::read(survivor.join("stream.json")).unwrap();

    let result = run_prune(root.path(), &[DAY.to_owned()], None, true, 1_000);

    assert!(
        result
            .refusals
            .iter()
            .any(|refusal| refusal.gate == "segment-identity"),
        "{:?}",
        result.refusals
    );
    assert_eq!(result.crash_repaired, 0);
    assert!(result.deleted.is_empty());
    assert_eq!(
        fs::read(survivor.join("stream.json")).unwrap(),
        marker_before
    );
    assert!(survivor.is_dir());
}

#[test]
fn execute_plan_clean_day_still_deletes() {
    let root = tempfile::tempdir().expect("journal");
    seed_duplicates(root.path());
    write_history(
        root.path(),
        PREFIX,
        DAY,
        &[json!({"segment":"090000_300","stream":STREAM})],
    );
    let candidate = segment_dir(root.path(), DAY, STREAM, "090000_301");
    let result = run_prune(root.path(), &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert!(
        result
            .deleted
            .iter()
            .any(|row| row.analysis.segment == "090000_301")
    );
    assert!(!candidate.is_dir());
}

#[test]
fn crash_repair_marks_successful_days_even_when_a_sibling_stream_refuses() {
    let root = tempfile::tempdir().expect("journal");
    let repaired_stream = "alpha";
    let refused_stream = "beta";
    seed_observer_owning_stream(root.path(), "aaaaaaaa", repaired_stream);
    seed_observer_owning_stream(root.path(), "bbbbbbbb", refused_stream);

    let repaired = write_segment(
        root.path(),
        DAY,
        repaired_stream,
        "090000_301",
        2,
        Some("090000_300"),
        b"repaired",
    );
    write_history(
        root.path(),
        "aaaaaaaa",
        DAY,
        &[json!({
            "type": "pruned",
            "ts": 1,
            "segment": "090000_300",
            "stream": repaired_stream,
            "duplicate_of": "",
        })],
    );
    let refused = write_segment(
        root.path(),
        DAY,
        refused_stream,
        "080000_301",
        2,
        Some("080000_300"),
        b"refused",
    );

    let result = run_prune(root.path(), &[DAY.to_owned()], None, true, 1_000);

    assert_eq!(result.crash_repaired, 1);
    assert!(
        result
            .refusals
            .iter()
            .any(|refusal| refusal.gate == "chain-repair"),
        "{:?}",
        result.refusals
    );
    let repaired_marker: serde_json::Value =
        serde_json::from_slice(&fs::read(repaired.join("stream.json")).unwrap()).unwrap();
    assert_eq!(repaired_marker["prev_day"], serde_json::Value::Null);
    assert_eq!(repaired_marker["prev_segment"], serde_json::Value::Null);
    let refused_marker: serde_json::Value =
        serde_json::from_slice(&fs::read(refused.join("stream.json")).unwrap()).unwrap();
    assert_eq!(refused_marker["prev_segment"], "080000_300");
    assert!(
        root.path()
            .join("chronicle")
            .join(DAY)
            .join("health/stream.updated")
            .is_file(),
        "the successful durable repair must dirty its day before the refusal returns"
    );
}
