// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use super::*;
use serde_json::json;
#[test]
fn backfill_is_dark_and_bound_sidecars_invalidate_on_screen_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert_eq!(reconcile(root).unwrap().processed, 0);
    assert!(!root.join("entities").exists());
    let dir = root.join("chronicle/20260101/120000_300");
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
        let dir = root.join(format!("chronicle/20260101/{i:06}_300"));
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
