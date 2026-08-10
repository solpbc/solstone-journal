// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use solstone_core_observer::store::prune::{format_result, run_prune};
use solstone_core_observer::store::record::ObserverRecord;
use solstone_core_observer::store::write::save_observer;

const DAY: &str = "20260101";
const STREAM: &str = "workstation";

struct Fixture {
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture(name: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "observer-prune-same-start-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    Fixture { root }
}

fn seed_observer(root: &Path, prefix: &str, stream: &str) {
    let record = ObserverRecord::from_value(json!({
        "key": format!("{prefix}12345678"),
        "name": stream,
        "stream": stream,
    }))
    .expect("record");
    save_observer(root, &record).expect("save observer");
}

fn segment_dir(root: &Path, day: &str, stream: &str, segment: &str) -> PathBuf {
    root.join("chronicle").join(day).join(stream).join(segment)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Write a segment with a valid `ingest.json` manifest and its declared media
/// content, plus a `stream.json` chain marker.
fn write_segment(
    root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    seq: u64,
    prev_segment: Option<&str>,
    audio: &[u8],
) -> PathBuf {
    let dir = segment_dir(root, day, stream, segment);
    fs::create_dir_all(&dir).expect("segment dir");
    fs::write(dir.join("audio.flac"), audio).expect("audio");
    let manifest = json!({
        "schema_version": 1,
        "files": {
            "audio.flac": {"sha256": sha256_hex(audio), "size": audio.len()},
        },
    });
    fs::write(dir.join("ingest.json"), manifest.to_string()).expect("manifest");
    let marker = json!({
        "stream": stream,
        "prev_day": prev_segment.map(|_| day),
        "prev_segment": prev_segment,
        "seq": seq,
    });
    fs::write(dir.join("stream.json"), marker.to_string()).expect("marker");
    dir
}

fn recursive_snapshot(root: &Path) -> Vec<(String, bool, u64)> {
    fn walk(root: &Path, dir: &Path, rows: &mut Vec<(String, bool, u64)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        for path in paths {
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            rows.push((
                path.strip_prefix(root)
                    .expect("relative")
                    .display()
                    .to_string(),
                metadata.is_dir(),
                metadata.len(),
            ));
            if metadata.is_dir() {
                walk(root, &path, rows);
            }
        }
    }
    let mut rows = Vec::new();
    walk(root, root, &mut rows);
    rows.sort();
    rows
}

#[test]
fn dry_run_across_a_deletable_group_and_a_protected_distinct_pair_changes_nothing() {
    let fixture = fixture("dry-run");
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    // A true duplicate pair at 090000: identical bytes, collision-ladder suffixes.
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    // Two silent captures at a DIFFERENT start with identical bytes: must never group.
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "100000_300",
        3,
        Some("090000_301"),
        b"unrelated",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "110000_300",
        4,
        Some("100000_300"),
        b"unrelated",
    );

    let before = recursive_snapshot(&fixture.root);
    let result = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), false, 1_000);
    let after = recursive_snapshot(&fixture.root);
    assert_eq!(before, after, "dry run must not write anything");

    assert_eq!(
        result.groups.len(),
        1,
        "only the same-start duplicate pair groups"
    );
    assert_eq!(result.groups[0].canonical.segment, "090000_300");
    assert_eq!(result.groups[0].candidates.len(), 1);
    assert_eq!(
        result.groups[0].candidates[0].analysis.segment,
        "090000_301"
    );
    assert!(result.refusals.is_empty());
    assert_eq!(result.exit_code(), 0);
}

#[test]
fn execute_deletes_the_duplicate_appends_history_and_deletes_index_rows() {
    let fixture = fixture("execute-basic");
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    let duplicate = write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );

    let result = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert!(
        result.refusals.is_empty(),
        "refusals: {:?}",
        result.refusals
    );
    assert_eq!(result.exit_code(), 0);
    assert_eq!(result.deleted.len(), 1);
    assert_eq!(result.deleted[0].analysis.segment, "090000_301");
    assert!(!duplicate.exists(), "duplicate directory must be removed");
    assert!(
        segment_dir(&fixture.root, DAY, STREAM, "090000_300").exists(),
        "canonical must survive"
    );

    let history_path = fixture
        .root
        .join("apps/observer/observers/abcdefgh/hist")
        .join(format!("{DAY}.jsonl"));
    let history = fs::read_to_string(&history_path).expect("history file");
    assert!(history.contains("\"type\":\"pruned\""));
    assert!(history.contains("\"segment\":\"090000_301\""));
    assert!(history.contains("\"duplicate_of\":\"090000_300\""));

    let output = format_result(&result);
    assert!(output.starts_with("observer prune execute\n"));
    assert!(output.contains("deleted: 1"));

    assert!(
        fixture
            .root
            .join("chronicle")
            .join(DAY)
            .join("health/stream.updated")
            .exists(),
        "health marker must be touched for the affected day"
    );
}

#[test]
fn execute_twice_is_a_clean_noop_second_run() {
    let fixture = fixture("execute-twice");
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );

    let first = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert_eq!(first.exit_code(), 0);
    assert_eq!(first.deleted.len(), 1);

    let second = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 2_000);
    assert_eq!(second.exit_code(), 0);
    assert!(second.refusals.is_empty());
    assert!(second.deleted.is_empty(), "nothing left to delete on rerun");

    let history_path = fixture
        .root
        .join("apps/observer/observers/abcdefgh/hist")
        .join(format!("{DAY}.jsonl"));
    let history = fs::read_to_string(&history_path).expect("history file");
    assert_eq!(
        history.matches("\"type\":\"pruned\"").count(),
        1,
        "rerun must not append a duplicate pruned record"
    );
}

#[test]
fn near_duplicate_content_mismatch_refuses_and_deletes_nothing_but_matching_bytes_still_prune() {
    let dirty = fixture("near-duplicate");
    seed_observer(&dirty.root, "abcdefgh", STREAM);
    // Three same-start segments: 300/301 are byte-identical duplicates, 302 is
    // a near-duplicate (different bytes) that must be refused and left alone.
    write_segment(
        &dirty.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &dirty.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    write_segment(
        &dirty.root,
        DAY,
        STREAM,
        "090000_302",
        3,
        Some("090000_301"),
        b"different!!",
    );

    let result = run_prune(&dirty.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert_eq!(
        result.exit_code(),
        2,
        "the near-duplicate refusal keeps exit 2"
    );
    assert!(
        result
            .refusals
            .iter()
            .any(|refusal| refusal.gate == "content-identity"),
        "refusals: {:?}",
        result.refusals
    );
    assert_eq!(result.deleted.len(), 1, "the true duplicate still prunes");
    assert_eq!(result.deleted[0].analysis.segment, "090000_301");
    assert!(
        segment_dir(&dirty.root, DAY, STREAM, "090000_302").exists(),
        "the near-duplicate must never be deleted"
    );

    // Twin direction: without the mismatch, the same duplicate pair prunes cleanly.
    let clean = fixture("near-duplicate-twin");
    seed_observer(&clean.root, "abcdefgh", STREAM);
    write_segment(
        &clean.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &clean.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    let clean_result = run_prune(&clean.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert_eq!(clean_result.exit_code(), 0);
    assert_eq!(clean_result.deleted.len(), 1);
}

#[test]
fn markerless_candidate_refuses_and_the_twin_without_it_prunes() {
    let dirty = fixture("markerless");
    seed_observer(&dirty.root, "abcdefgh", STREAM);
    write_segment(
        &dirty.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &dirty.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    fs::remove_file(segment_dir(&dirty.root, DAY, STREAM, "090000_301").join("stream.json"))
        .expect("remove marker");

    let result = run_prune(&dirty.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert_eq!(result.exit_code(), 2);
    assert!(
        result
            .refusals
            .iter()
            .any(|refusal| refusal.gate == "chain-identity")
    );
    assert!(result.deleted.is_empty());
    assert!(segment_dir(&dirty.root, DAY, STREAM, "090000_301").exists());

    let twin = fixture("markerless-twin");
    seed_observer(&twin.root, "abcdefgh", STREAM);
    write_segment(
        &twin.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &twin.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    let twin_result = run_prune(&twin.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert_eq!(twin_result.exit_code(), 0);
    assert_eq!(twin_result.deleted.len(), 1);
}

#[test]
fn ambiguous_observer_attribution_refuses_and_deletes_nothing() {
    let fixture = fixture("ambiguous-attribution");
    // No observer registered at all: attribution refuses outright.
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );

    let result = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert_eq!(result.exit_code(), 2);
    assert!(
        result
            .refusals
            .iter()
            .any(|refusal| refusal.gate == "observer-attribution")
    );
    assert!(result.deleted.is_empty());
    assert!(segment_dir(&fixture.root, DAY, STREAM, "090000_301").exists());

    // Twin: register the owning observer, the same fixture now prunes.
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    let second = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 2_000);
    assert_eq!(second.exit_code(), 0);
    assert_eq!(second.deleted.len(), 1);
}

#[cfg(unix)]
#[test]
fn a_removal_failure_stops_that_group_after_the_history_record_is_written() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = fixture("removal-failure");
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    let duplicate = write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    // Make the duplicate's own directory unwritable so the real rmtree call
    // fails on its very first unlink, before deleting any content -- chmodding
    // the *parent* instead would let rmtree wipe the children and only fail
    // on the final rmdir, which is a different (also real) failure mode but
    // not the one this test means to exercise.
    fs::set_permissions(&duplicate, fs::Permissions::from_mode(0o555)).expect("chmod");

    let result = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);

    fs::set_permissions(&duplicate, fs::Permissions::from_mode(0o755)).expect("restore chmod");

    assert_eq!(
        result.exit_code(),
        2,
        "a delete failure is a refusal, not silent success"
    );
    assert!(
        result
            .refusals
            .iter()
            .any(|refusal| refusal.gate == "delete"),
        "refusals: {:?}",
        result.refusals
    );
    assert!(
        duplicate.exists(),
        "the directory must survive a failed removal"
    );
    assert_eq!(
        fs::read(duplicate.join("audio.flac")).expect("content survives"),
        b"same bytes",
        "content untouched, not just the directory entry"
    );

    let history_path = fixture
        .root
        .join("apps/observer/observers/abcdefgh/hist")
        .join(format!("{DAY}.jsonl"));
    let history = fs::read_to_string(&history_path).expect("history file");
    assert_eq!(
        history.matches("\"type\":\"pruned\"").count(),
        1,
        "the history record must exist even though the delete failed"
    );

    // A second run converges: the directory is now actually removed, and the
    // existing history record is deduped rather than duplicated.
    let second = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 2_000);
    assert_eq!(second.exit_code(), 0, "refusals: {:?}", second.refusals);
    assert!(!duplicate.exists());
    let history_after = fs::read_to_string(&history_path).expect("history file");
    assert_eq!(history_after.matches("\"type\":\"pruned\"").count(), 1);
}

#[test]
fn earliest_by_name_that_is_not_a_duplicate_is_never_chosen_as_canonical() {
    // 090000_300 is earliest by name but has DIFFERENT bytes from the other
    // two; the true duplicate pair's earliest member, 090000_301, must be the
    // canonical -- proving canonical selection is "earliest within the
    // byte-identical cluster," not "earliest across the whole same-start set."
    let fixture = fixture("earliest-vs-duplicate");
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"lone recording",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_302",
        3,
        Some("090000_301"),
        b"same bytes",
    );

    let result = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), false, 1_000);
    assert_eq!(result.groups.len(), 1);
    assert_eq!(result.groups[0].canonical.segment, "090000_301");
    assert_eq!(result.groups[0].candidates.len(), 1);
    assert_eq!(
        result.groups[0].candidates[0].analysis.segment,
        "090000_302"
    );
    assert!(
        result
            .refusals
            .iter()
            .any(|refusal| refusal.gate == "content-identity"),
        "090000_300 is a singleton mismatch, refused rather than silently ignored"
    );
}
