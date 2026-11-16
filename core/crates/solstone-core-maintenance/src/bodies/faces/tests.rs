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
        .insert(all[2].key.clone(), unsupported_mark(&all[2].binding));
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

fn age(path: &Path, seconds: u64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .unwrap();
}
fn reads() -> usize {
    BODY_READS.with(|n| n.replace(0))
}
/// Two plain recordings and one meeting already described, all settled.
fn settled_history(root: &Path) -> (PathBuf, PathBuf) {
    let day = root.join("chronicle").join("20260101");
    let meeting =
        json!({"frame_id":1,"content":{"meeting":{"participants":[{"name":"synthetic"}]}}});
    for (name, body) in [
        ("000000_300", json!({"frame_id":1,"content":{}})),
        ("000001_300", json!({"frame_id":1,"content":{}})),
        ("000002_300", meeting),
    ] {
        let dir = day.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("screen.webm"), b"synthetic").unwrap();
        std::fs::write(dir.join("screen.jsonl"), format!("{{}}\n{body}\n")).unwrap();
        age(&dir.join("screen.jsonl"), 600);
    }
    let raw = day.join("000002_300").join("screen.webm");
    let digest = solstone_core_segment::screen_body_digest(&raw.with_extension("jsonl")).unwrap();
    std::fs::write(
        sidecar_path(&raw),
        json!({"schema_version":1,"source_name":"screen.webm","screen_body_sha256":digest})
            .to_string(),
    )
    .unwrap();
    age(&sidecar_path(&raw), 600);
    (day.join("000001_300").join("screen.jsonl"), raw)
}

#[test]
fn an_unchanged_history_costs_a_stat_not_a_read() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    settled_history(root);
    reads();
    let first = backfill(root).unwrap();
    assert_eq!((first.valid, first.pending, first.failed), (1, 0, 0));
    // Three screen outputs and one sidecar were read to settle them.
    assert_eq!(reads(), 4);
    let second = backfill(root).unwrap();
    assert_eq!((second.valid, second.pending, second.failed), (1, 0, 0));
    assert_eq!(reads(), 0, "an unchanged history reads no screen bodies");
    let daily = reconcile(root).unwrap();
    assert_eq!((daily.valid, daily.pending), (1, 0));
    assert_eq!(reads(), 0);
}

#[test]
fn a_changed_screen_or_sidecar_is_read_again() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let (plain, raw) = settled_history(root);
    backfill(root).unwrap();
    reads();
    // A plain recording rewritten as a meeting becomes a candidate.
    let meeting =
        json!({"frame_id":1,"content":{"meeting":{"participants":[{"name":"synthetic"}]}}});
    std::fs::write(&plain, format!("{{}}\n{meeting}\n")).unwrap();
    age(&plain, 300);
    // A sidecar bound to an older screen body no longer counts as described.
    std::fs::write(
        sidecar_path(&raw),
        json!({"schema_version":1,"source_name":"screen.webm","screen_body_sha256":"stale"})
            .to_string(),
    )
    .unwrap();
    age(&sidecar_path(&raw), 300);
    let mut stats = Stats::default();
    let pending = discover(root, &mut cross::Backfill::default(), &mut stats).unwrap();
    assert_eq!((pending.len(), stats.valid), (2, 0));
    // Each changed recording: its screen output and its sidecar.
    assert_eq!(reads(), 4);
    // A file still being written is not trusted from its stamp.
    let fresh = root.join("chronicle/20260101/000000_300/screen.jsonl");
    std::fs::write(&fresh, "{}\n{\"frame_id\":2,\"content\":{}}\n").unwrap();
    discover(root, &mut cross::Backfill::default(), &mut Stats::default()).unwrap();
    discover(root, &mut cross::Backfill::default(), &mut Stats::default()).unwrap();
    // The two owed meetings are remembered, so only the fresh output is read,
    // on both passes.
    assert_eq!(reads(), 2);
}

#[test]
fn an_owed_meeting_costs_a_stat_and_a_new_version_retries_an_unsupported_one() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dir = root.join("chronicle/20260101/000000_300");
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("screen.webm");
    std::fs::write(&raw, b"synthetic").unwrap();
    let meeting =
        json!({"frame_id":1,"content":{"meeting":{"participants":[{"name":"synthetic"}]}}});
    std::fs::write(dir.join("screen.jsonl"), format!("{{}}\n{meeting}\n")).unwrap();
    age(&dir.join("screen.jsonl"), 600);
    age(&raw, 600);
    let mut state = cross::Backfill::default();
    reads();
    let first = discover(root, &mut state, &mut Stats::default()).unwrap();
    assert_eq!((first.len(), reads()), (1, 2));
    // Still owed and unchanged: found again from the cache.
    let again = discover(root, &mut state, &mut Stats::default()).unwrap();
    assert_eq!((again.len(), reads()), (1, 0));
    assert_eq!(again[0].binding, first[0].binding);
    // Found unsupported by this version: skipped, still without a read.
    state
        .unsupported
        .insert(first[0].key.clone(), unsupported_mark(&first[0].binding));
    let mut stats = Stats::default();
    assert!(discover(root, &mut state, &mut stats).unwrap().is_empty());
    assert_eq!((stats.already_skipped, reads()), (1, 0));
    // Found unsupported by an older version, which may have decoded less.
    state.unsupported.insert(
        first[0].key.clone(),
        format!("{} 2.0.0-older", first[0].binding),
    );
    assert_eq!(
        discover(root, &mut state, &mut Stats::default())
            .unwrap()
            .len(),
        1
    );
    // A sidecar appearing is read, and the meeting counts as described.
    let digest = solstone_core_segment::screen_body_digest(&dir.join("screen.jsonl")).unwrap();
    std::fs::write(
        sidecar_path(&raw),
        json!({"schema_version":1,"source_name":"screen.webm","screen_body_sha256":digest})
            .to_string(),
    )
    .unwrap();
    age(&sidecar_path(&raw), 600);
    reads();
    let mut stats = Stats::default();
    assert!(discover(root, &mut state, &mut stats).unwrap().is_empty());
    assert_eq!((stats.valid, reads()), (1, 2));
}

#[test]
fn a_removed_sidecar_or_recording_leaves_the_scan_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let (plain, raw) = settled_history(root);
    backfill(root).unwrap();
    assert_eq!(load_scan(root).len(), 3);
    // Without its sidecar the meeting is owed again, not remembered as described.
    std::fs::remove_file(sidecar_path(&raw)).unwrap();
    let mut stats = Stats::default();
    let pending = discover(root, &mut cross::Backfill::default(), &mut stats).unwrap();
    assert_eq!((pending.len(), stats.valid), (1, 0));
    let scan = load_scan(root);
    let owed = scan
        .iter()
        .find(|(k, _)| k.contains("000002_300"))
        .unwrap()
        .1;
    assert!(owed.ends_with(" -"), "remembered as owed, with no sidecar");
    // A recording no longer in the journal drops out of the cache.
    std::fs::remove_dir_all(plain.parent().unwrap()).unwrap();
    discover(root, &mut cross::Backfill::default(), &mut Stats::default()).unwrap();
    let scan = load_scan(root);
    assert_eq!(scan.len(), 2);
    assert!(!scan.keys().any(|k| k.contains("000001_300")));
    // Turned off, the catch-up keeps nothing of its own.
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/journal.json"),
        json!({"faces":{"enabled":false}}).to_string(),
    )
    .unwrap();
    backfill(root).unwrap();
    assert!(!root.join(SCAN_CACHE).exists());
}
