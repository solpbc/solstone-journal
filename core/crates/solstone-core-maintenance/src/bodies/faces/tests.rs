// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use super::*;
use serde_json::json;
#[test]
fn backfill_is_dark_and_bound_sidecars_invalidate_on_screen_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // Photos of people are on by default; turned off, the catch-up touches nothing.
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/journal.json"),
        json!({"faces":{"enabled":false}}).to_string(),
    )
    .unwrap();
    assert_eq!(reconcile(root).unwrap().processed, 0);
    assert!(!root.join("entities").exists());
    let dir = root.join("chronicle").join("20260101").join("120000_300");
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("screen.webm");
    let screen = raw.with_extension("jsonl");
    std::fs::write(&raw, b"fake recording").unwrap();
    let body = json!({"frame_id":1,"content":{"meeting":{"participants":[{"name":"synthetic"}]}}});
    std::fs::write(&screen, format!("{{}}\n{body}\n")).unwrap();
    let digest = solstone_core_segment::screen_body_digest(&screen).unwrap();
    assert!(!valid_sidecar(&raw, &digest).unwrap());
    std::fs::write(
        sidecar_path(&raw),
        json!({"schema_version":1,"source_name":"screen.webm","screen_body_sha256":digest})
            .to_string(),
    )
    .unwrap();
    assert!(valid_sidecar(&raw, &digest).unwrap());
    std::fs::write(&screen, format!("{{}}\n{body}\n{{}}\n")).unwrap();
    let changed = solstone_core_segment::screen_body_digest(&screen).unwrap();
    assert!(!valid_sidecar(&raw, &changed).unwrap());
}

#[test]
fn discovery_resumes_after_cursor_and_unsupported_inputs_are_not_retried() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for i in 0..4 {
        let dir = root
            .join("chronicle")
            .join("20260101")
            .join(format!("{i:06}_300"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("screen.webm"), b"synthetic").unwrap();
        let body =
            json!({"frame_id":1,"content":{"meeting":{"participants":[{"name":"synthetic"}]}}});
        std::fs::write(dir.join("screen.jsonl"), format!("{{}}\n{body}\n")).unwrap();
    }
    let mut backfill = cross::Backfill::default();
    let mut stats = Stats::default();
    let all = discover(root, &mut backfill, &mut stats).unwrap();
    assert_eq!(all.len(), 4);
    backfill.cursor = Some(all[1].key.clone());
    backfill
        .unsupported
        .insert(all[2].key.clone(), all[2].binding.clone());
    stats = Stats::default();
    let pending = discover(root, &mut backfill, &mut stats).unwrap();
    assert_eq!(pending.len(), 3);
    assert_eq!(stats.already_skipped, 1);
    assert_eq!(pending[0].key, all[3].key);
    // A changed input invalidates the old unsupported disposition.
    std::fs::write(&all[2].raw, b"changed synthetic bytes").unwrap();
    assert_eq!(
        discover(root, &mut backfill, &mut Stats::default())
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn one_undescribed_or_damaged_screen_does_not_stop_the_history() {
    // Real journals keep recordings whose screen output was never written
    // (an interrupted draft) or holds a torn row. Each is skipped; the rest
    // of the history is still discovered.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let day = root.join("chronicle").join("20260101");
    let body = json!({"frame_id":1,"content":{"meeting":{"participants":[{"name":"synthetic"}]}}});
    let make = |name: &str, screen: Option<String>| {
        let dir = day.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("screen.webm"), b"synthetic").unwrap();
        if let Some(text) = screen {
            std::fs::write(dir.join("screen.jsonl"), text).unwrap();
        }
    };
    make("000000_300", None);
    make("000001_300", Some("{}\n{\"frame_id\":1,\"con\n".into()));
    make("000002_300", Some("{}\ngarbage\n".into()));
    // A torn tail beside a real meeting row: the helper reads strictly, so this
    // recording is skipped rather than failed every hour.
    make(
        "000004_300",
        Some(format!("{{}}\n{body}\n{{\"frame_id\":2,\"con\n")),
    );
    make("000003_300", Some(format!("{{}}\n{body}\n")));
    let dir = day.join("000005_300");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("screen.webm"), b"synthetic").unwrap();
    let mut torn = format!("{{}}\n{body}\n").into_bytes();
    torn.extend_from_slice(b"{\"frame_id\":2,\"text\":\"\xe2\x80");
    std::fs::write(dir.join("screen.jsonl"), torn).unwrap();
    let mut state = cross::Backfill::default();
    let mut stats = Stats::default();
    let pending = discover(root, &mut state, &mut stats).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].key.contains("000003_300"));
    // The hourly catch-up and the daily recovery both finish over them.
    std::fs::remove_dir_all(day.join("000003_300")).unwrap();
    let hourly = backfill(root).unwrap();
    assert_eq!((hourly.pending, hourly.failed), (0, 0));
    let daily = reconcile(root).unwrap();
    assert_eq!((daily.pending, daily.failed), (0, 0));
}
