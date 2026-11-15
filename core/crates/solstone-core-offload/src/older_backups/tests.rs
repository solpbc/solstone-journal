// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

use super::*;
use solstone_core_retention::archive_catalog::OffloadFile;
use tempfile::TempDir;

const DAY: &str = "20261101";
const NOW: i64 = 1_763_200_000; // 2025-11-15T09:46:40Z

fn journal() -> TempDir {
    let temp = TempDir::new().unwrap();
    std::fs::create_dir_all(temp.path().join("config")).unwrap();
    temp
}

fn write_backup(journal: &Path, backup: Value) {
    std::fs::write(
        journal.join("config/journal.json"),
        serde_json::to_vec_pretty(&json!({"backup": backup})).unwrap(),
    )
    .unwrap();
}

fn iso(unix: i64) -> String {
    DateTime::from_timestamp(unix, 0).unwrap().to_rfc3339()
}

/// An erase entry as the trash writes it, its files gone at `at`.
fn erase(journal: &Path, at: i64, segment: &str) {
    let operation = DateTime::from_timestamp(at, 0)
        .unwrap()
        .format("%Y%m%dT%H%M%S%3fZ")
        .to_string();
    let id = format!("{operation}-erased-{segment}");
    let dir = journal.join("config/removals");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_vec(&json!({
            "schema": 1, "id": id, "operation": operation, "event": "erased",
            "at": iso(at), "action": "erase", "surface": "policy", "policy": "trash",
            "unit": {"kind": "segment", "day": DAY, "stream": "macbook", "segment": segment},
        }))
        .unwrap(),
    )
    .unwrap();
}

fn dated(id: &str, unix: i64) -> Value {
    json!({"id": id, "time": iso(unix)})
}

fn backup_with(last_ok: (i64, &str), older_backups: Value) -> Value {
    json!({
        "enabled": true,
        "last_backup": {"time": last_ok.0, "snapshot_id": last_ok.1, "status": "ok", "error_reason": null},
        "older_backups": older_backups,
    })
}

fn survey(newest: (&str, i64), older: &[(&str, i64)]) -> Value {
    json!({
        "time": newest.1 + 60,
        "newest": dated(newest.0, newest.1),
        "older": older.iter().map(|(id, time)| dated(id, *time)).collect::<Vec<_>>(),
        "kept_before_restore": [], "archives_removable": [], "archives_mixed": [],
        "archives_kept": 0, "storage": "removes", "operated": false,
    })
}

fn state_of(view: &Value) -> &str {
    view["state"].as_str().unwrap()
}

#[test]
fn nothing_erased_shows_nothing() {
    let temp = journal();
    write_backup(
        temp.path(),
        backup_with(
            (NOW - 100, "aa"),
            json!({"survey": survey(("aa", NOW - 200), &[("bb", NOW - 4000)])}),
        ),
    );
    assert_eq!(state_of(&view(temp.path(), NOW, false)), "hidden");
}

#[test]
fn backup_off_or_never_on_shows_nothing() {
    let temp = journal();
    erase(temp.path(), NOW - 1000, "090000_300");
    write_backup(temp.path(), json!({"enabled": false}));
    assert_eq!(state_of(&view(temp.path(), NOW, false)), "hidden");
}

#[test]
fn an_erase_after_the_last_backup_waits() {
    let temp = journal();
    erase(temp.path(), NOW - 1000, "090000_300");
    write_backup(temp.path(), backup_with((NOW - 2000, "aa"), json!({})));
    assert_eq!(state_of(&view(temp.path(), NOW, false)), "waiting");
}

#[test]
fn a_backup_after_the_erase_with_older_ones_is_ready() {
    let temp = journal();
    erase(temp.path(), NOW - 1000, "090000_300");
    write_backup(
        temp.path(),
        backup_with(
            (NOW - 100, "aa11"),
            json!({"survey": survey(("aa11ff", NOW - 400), &[("bb", NOW - 5000), ("cc", NOW - 4000)])}),
        ),
    );
    let shown = view(temp.path(), NOW, false);
    assert_eq!(state_of(&shown), "ready");
    assert_eq!(shown["older"], 2);
    assert_eq!(shown["older_from"], json!(iso(NOW - 5000)));
}

#[test]
fn a_listing_from_before_the_last_backup_does_not_offer() {
    let temp = journal();
    erase(temp.path(), NOW - 1000, "090000_300");
    // The listing's newest isn't the last successful backup: it can't count.
    write_backup(
        temp.path(),
        backup_with(
            (NOW - 100, "dd"),
            json!({"survey": survey(("aa", NOW - 3000), &[("bb", NOW - 5000)])}),
        ),
    );
    assert_eq!(state_of(&view(temp.path(), NOW, false)), "waiting");
}

#[test]
fn an_erase_no_backup_predates_is_not_offered() {
    let temp = journal();
    // Backup was turned on after the erase: nothing older ever held it.
    erase(temp.path(), NOW - 9000, "090000_300");
    write_backup(
        temp.path(),
        backup_with(
            (NOW - 100, "aa"),
            json!({"survey": survey(("aa", NOW - 400), &[("bb", NOW - 5000)])}),
        ),
    );
    assert_eq!(state_of(&view(temp.path(), NOW, false)), "hidden");
}

#[test]
fn settled_erases_are_not_offered_again() {
    let temp = journal();
    erase(temp.path(), NOW - 1000, "090000_300");
    write_backup(
        temp.path(),
        backup_with(
            (NOW - 100, "aa"),
            json!({"survey": survey(("aa", NOW - 400), &[("bb", NOW - 5000)]), "settled_through": iso(NOW - 400)}),
        ),
    );
    assert_eq!(state_of(&view(temp.path(), NOW, false)), "hidden");
}

#[test]
fn a_finished_run_shows_done_for_seven_days_then_nothing() {
    let temp = journal();
    let done = json!({"at": NOW - 3600, "kept_newest_time": iso(NOW - 4000), "removed": 3,
        "archives_removed": 0, "archives_mixed": 0, "kept_before_restore_left": 0, "storage": "removes"});
    write_backup(
        temp.path(),
        backup_with(
            (NOW - 100, "aa"),
            json!({"done": done, "settled_through": iso(NOW - 4000)}),
        ),
    );
    assert_eq!(state_of(&view(temp.path(), NOW, false)), "done");
    assert_eq!(
        state_of(&view(temp.path(), NOW + 7 * 86400, false)),
        "hidden"
    );
}

#[test]
fn kept_restore_backups_or_versioned_storage_read_partly_done() {
    for (left, storage, expected) in [
        (2, "removes", "partly_done"),
        (0, "versions", "partly_done"),
        (0, "unknown", "done"),
    ] {
        let temp = journal();
        let done = json!({"at": NOW - 60, "kept_newest_time": iso(NOW - 400), "removed": 3,
            "archives_removed": 0, "archives_mixed": 0, "kept_before_restore_left": left, "storage": storage});
        let mut survey = survey(("aa", NOW - 400), &[]);
        survey["kept_before_restore"] = json!([dated("k1", NOW - 90000), dated("k2", NOW - 80000)]);
        write_backup(
            temp.path(),
            backup_with((NOW - 100, "aa"), json!({"done": done, "survey": survey})),
        );
        let shown = view(temp.path(), NOW, false);
        assert_eq!(state_of(&shown), expected, "{storage} {left}");
    }
}

#[test]
fn a_run_reads_running_only_while_its_lock_is_held() {
    let temp = journal();
    let run = |stopped: Value| {
        json!({"run": {"started": NOW - 7200, "updated": NOW - 60, "keep": "aa", "keep_time": iso(NOW - 400),
            "include_kept": false, "total": 41, "removed": 18, "archives_removed": 0, "stopped": stopped}})
    };
    write_backup(
        temp.path(),
        backup_with((NOW - 100, "aa"), run(Value::Null)),
    );
    // A record nobody holds the lock for is a run that stopped.
    let shown = view(temp.path(), NOW, false);
    assert_eq!(
        (state_of(&shown), shown["reason"].as_str()),
        ("stopped", Some("interrupted"))
    );
    {
        let _held = hold_run_lock(temp.path()).unwrap();
        let shown = view(temp.path(), NOW, false);
        assert_eq!(
            (state_of(&shown), shown["removed"].as_u64()),
            ("running", Some(18))
        );
        assert_eq!(hold_run_lock(temp.path()).err(), Some(Refusal::Running));
    }
    assert!(
        temp.path().join("health/older-backups.lock").exists(),
        "the lock file stays"
    );
    write_backup(
        temp.path(),
        backup_with((NOW - 100, "aa"), run(json!("locked"))),
    );
    let shown = view(temp.path(), NOW, false);
    assert_eq!(
        (state_of(&shown), shown["reason"].as_str()),
        ("stopped", Some("locked"))
    );
    // The page's own slot says running whatever the record says.
    assert_eq!(state_of(&view(temp.path(), NOW, true)), "running");
}

#[test]
fn an_erase_whose_act_predates_the_settled_time_still_counts() {
    let temp = journal();
    // Its name dates the owner's act two days before the settled time; its
    // files were gone after it.
    let at = NOW - 1000;
    let act = DateTime::from_timestamp(NOW - 2 * 86400 - 5000, 0)
        .unwrap()
        .format("%Y%m%dT%H%M%S%3fZ")
        .to_string();
    let dir = temp.path().join("config/removals");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{act}-t1-erased-t1-uabcd.json")),
        serde_json::to_vec(&json!({"schema": 1, "id": format!("{act}-t1-erased-t1-uabcd"), "operation": format!("{act}-t1"),
            "event": "erased", "at": iso(at), "action": "erase", "surface": "policy", "policy": "trash",
            "unit": {"kind": "entity"}}))
        .unwrap(),
    )
    .unwrap();
    write_backup(
        temp.path(),
        backup_with(
            (NOW - 100, "aa"),
            json!({"survey": survey(("aa", NOW - 400), &[("bb", NOW - 5000)]), "settled_through": iso(NOW - 2 * 86400)}),
        ),
    );
    assert_eq!(state_of(&view(temp.path(), NOW, false)), "ready");
}

#[test]
fn times_compare_as_instants_across_offsets() {
    let temp = journal();
    // The newest backup started at 20:00Z, written +09:00; the erase was at
    // 21:00Z. A string compare would put the backup after the erase.
    let erase_at = DateTime::parse_from_rfc3339("2026-11-14T21:00:00.000Z")
        .unwrap()
        .timestamp();
    erase(temp.path(), erase_at, "090000_300");
    let mut listing = survey(("aa", 0), &[("bb", erase_at - 9000)]);
    listing["newest"] = json!({"id": "aa", "time": "2026-11-15T05:00:00.123456789+09:00"});
    write_backup(
        temp.path(),
        backup_with((erase_at + 600, "aa"), json!({"survey": listing})),
    );
    assert_eq!(
        state_of(&view(temp.path(), erase_at + 700, false)),
        "waiting"
    );
}

#[test]
fn an_incomplete_newest_backup_offers_nothing_older() {
    let temp = journal();
    erase(temp.path(), NOW - 1000, "090000_300");
    let mut backup = backup_with(
        (NOW - 100, "aa"),
        json!({"survey": survey(("aa", NOW - 400), &[("bb", NOW - 5000)])}),
    );
    backup["last_backup"]["error_reason"] = json!("incomplete");
    write_backup(temp.path(), backup);
    let shown = view(temp.path(), NOW, false);
    assert_eq!(
        (state_of(&shown), shown["newest_incomplete"].as_bool()),
        ("waiting", Some(true))
    );
}

// ---------------------------------------------------------------- archives

fn segment_dir(journal: &Path, key: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join(DAY)
        .join("macbook")
        .join(key)
}

fn offloaded(journal: &Path, key: &str, snapshot: &str) {
    let dir = segment_dir(journal, key);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("audio.jsonl"), b"{}\n").unwrap();
    crate::ledger::append_offload_event(
        journal,
        DAY,
        "macbook",
        key,
        snapshot,
        &[OffloadFile {
            name: "audio.m4a".into(),
            bytes: 10,
            sha256: "a".repeat(64),
        }],
        1,
    )
    .unwrap();
}

/// The recording went to the trash: its folder now holds only the tombstone.
fn trashed(journal: &Path, key: &str, cid: &str, still_waiting: bool) {
    let dir = segment_dir(journal, key);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(TOMBSTONE_NAME),
        serde_json::to_vec(
            &json!({"deleted_at": iso(NOW), "cid": cid, "reason": "owner_segment_delete",
            "manifest": [format!("chronicle/{DAY}/macbook/{key}/audio.jsonl")]}),
        )
        .unwrap(),
    )
    .unwrap();
    if still_waiting {
        std::fs::create_dir_all(journal.join(".trash").join(cid).join("files")).unwrap();
    }
}

fn archive(id: &str) -> SnapshotFacts {
    SnapshotFacts {
        id: id.into(),
        time: iso(NOW - 50_000),
        tags: vec![OFFLOAD_ARCHIVE_TAG.into()],
        paths: vec!["archive/audio.m4a".into()],
    }
}

#[test]
fn an_archive_goes_only_when_everything_it_holds_was_erased() {
    let temp = journal();
    let root = temp.path();
    let erased_id = "1".repeat(64);
    let live_id = "2".repeat(64);
    let waiting_id = "3".repeat(64);
    let mixed_id = "4".repeat(64);
    offloaded(root, "090000_300", &erased_id);
    offloaded(root, "091000_300", &live_id);
    offloaded(root, "092000_300", &waiting_id);
    offloaded(root, "093000_300", &mixed_id);
    offloaded(root, "094000_300", &mixed_id);
    trashed(root, "090000_300", "t-erased", false);
    trashed(root, "092000_300", "t-waiting", true);
    trashed(root, "093000_300", "t-mixed", false);
    let unknown_id = "5".repeat(64);
    let classes = classify_archives(
        root,
        &[
            archive(&erased_id),
            archive(&live_id),
            archive(&waiting_id),
            archive(&mixed_id),
            archive(&unknown_id),
        ],
    );
    assert_eq!(classes.removable, BTreeSet::from([erased_id]));
    assert_eq!(classes.mixed, BTreeSet::from([mixed_id, unknown_id]));
}

#[test]
fn a_short_historical_id_matches_its_archive() {
    let temp = journal();
    let full = format!("abcdef12{}", "0".repeat(56));
    offloaded(temp.path(), "090000_300", "abcdef12");
    trashed(temp.path(), "090000_300", "t1", false);
    let classes = classify_archives(temp.path(), &[archive(&full)]);
    assert_eq!(classes.removable, BTreeSet::from([full]));
}

#[test]
fn an_unreadable_offload_record_keeps_every_archive() {
    let temp = journal();
    let id = "1".repeat(64);
    offloaded(temp.path(), "090000_300", &id);
    trashed(temp.path(), "090000_300", "t1", false);
    std::fs::write(
        temp.path()
            .join("health/offload")
            .join(format!("{DAY}.jsonl")),
        b"{not json\n",
    )
    .unwrap();
    let classes = classify_archives(temp.path(), &[archive(&id)]);
    assert!(classes.removable.is_empty());
    assert_eq!(classes.mixed, BTreeSet::from([id]));
}

#[test]
fn a_listing_sorts_into_newest_older_kept_and_archives() {
    let temp = journal();
    let at = |offset: i64, tags: &[&str], id: &str| SnapshotFacts {
        id: id.into(),
        time: iso(NOW - offset),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        paths: vec!["/j".into()],
    };
    let survey = sort_listing(
        temp.path(),
        vec![
            at(100, &[], "n1"),
            at(9000, &[], "o1"),
            at(5000, &[], "o2"),
            at(99_000, &[KEPT_BEFORE_RESTORE_TAG], "k1"),
            at(70_000, &[ARCHIVE_TAG], "a1"),
        ],
        "removes",
        false,
        NOW,
    );
    assert_eq!(survey.newest.unwrap().id, "n1");
    assert_eq!(
        survey
            .older
            .iter()
            .map(|d| d.id.as_str())
            .collect::<Vec<_>>(),
        ["o1", "o2"]
    );
    assert_eq!(survey.kept_before_restore[0].id, "k1");
    // No offload record names it, so it may hold anything: it stays, counted
    // as possibly holding something erased.
    assert_eq!((survey.archives_kept, survey.archives_mixed.len()), (0, 1));
}

#[test]
fn a_tombstone_beside_media_still_present_does_not_count_as_erased() {
    let temp = journal();
    let id = "6".repeat(64);
    offloaded(temp.path(), "090000_300", &id);
    trashed(temp.path(), "090000_300", "t1", false);
    // The witnessed file is back in the folder beside the tombstone.
    std::fs::write(
        segment_dir(temp.path(), "090000_300").join("audio.m4a"),
        b"0123456789",
    )
    .unwrap();
    let classes = classify_archives(temp.path(), &[archive(&id)]);
    assert!(classes.removable.is_empty());
}
