// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    reason = "test code: fixtures remove files to set up states; the crate-level denials exist to constrain the verbs"
)]
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::{JournalRoot, LockOptions};

use super::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);
const AUDIO: &[u8] = b"synthetic audio bytes";

fn at() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(1_772_614_800_000).unwrap()
}

fn journal() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "retention-owner-voice-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

struct Fixture {
    journal: std::path::PathBuf,
    segment: std::path::PathBuf,
    jsonl: std::path::PathBuf,
    original: std::path::PathBuf,
}

fn record(state: &str, size: u64, digest: &str) -> Value {
    json!({
        "schema": vocab::SCHEMA,
        "state": state,
        "reason_code": if state == "analyzed" { "ok" } else { "no_speech" },
        "handler": vocab::HANDLER_TRANSCRIBE,
        "attempted_at": "2026-10-20T10:00:00Z",
        "input_size": size,
        "owner_voice_filter": true,
        "owner_voice": {
            "version": 1,
            "speech_s": 10.0,
            "kept_s": 6.0,
            "left_out_s": 4.0,
            "windows_scored": 5,
            "windows_kept": 3,
            "original": {"name": "120000_300.m4a", "size": size, "sha256": digest},
            "print": {"sha256": "0".repeat(64)},
        },
    })
}

fn fixture_in(segment_rel: &str, state: &str) -> Fixture {
    let journal = journal();
    let segment = journal.join(segment_rel);
    fs::create_dir_all(&segment).unwrap();
    let original = segment.join("120000_300.m4a");
    fs::write(&original, AUDIO).unwrap();
    let jsonl = segment.join("120000_300.jsonl");
    let header = json!({"raw": "120000_300.m4a", "_solstone_processing": record(state, AUDIO.len() as u64, &sha(AUDIO))});
    let mut body = format!("{header}\n");
    if state == "analyzed" {
        body.push_str(&format!(
            "{}\n",
            json!({"start": "12:00:01", "text": "kept words"})
        ));
    }
    fs::write(&jsonl, body).unwrap();
    Fixture {
        journal,
        segment,
        jsonl,
        original,
    }
}

fn fixture(state: &str) -> Fixture {
    fixture_in("chronicle/20261020/watch/120000_300", state)
}

fn finish(fixture: &Fixture) -> OwnerVoiceDeletion {
    let claims = OutputClaims::acquire(
        [fixture.jsonl.clone()],
        LockOptions {
            timeout: Duration::ZERO,
            ..LockOptions::default()
        },
    )
    .unwrap();
    let commit = claims.commit(&fixture.segment).unwrap();
    finish_owner_voice_deletion(&fixture.journal, &fixture.jsonl, &claims, &commit, at())
}

#[test]
fn events_scan_reads_presence_with_its_failure_polarity() {
    let fixture = fixture("analyzed");
    assert_eq!(
        retention_events_for_name(&fixture.segment, "120000_300.m4a").unwrap(),
        RetentionEventsForName::default()
    );
    fs::write(
        fixture.segment.join(EVENTS_FILE),
        "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"120000_300.m4a\",\"class\":\"some_future_class\"}\n{torn\n{\"tract\":\"retention\",\"event\":\"original_release_refused\",\"name\":\"other.m4a\",\"reason\":\"unknown\"}\n",
    )
    .unwrap();
    let mine = retention_events_for_name(&fixture.segment, "120000_300.m4a").unwrap();
    assert!(mine.deleted && !mine.refused);
    let other = retention_events_for_name(&fixture.segment, "other.m4a").unwrap();
    assert!(!other.deleted && other.refused);

    fs::remove_file(fixture.segment.join(EVENTS_FILE)).unwrap();
    fs::create_dir(fixture.segment.join(EVENTS_FILE)).unwrap();
    assert!(retention_events_for_name(&fixture.segment, "120000_300.m4a").is_err());
}

#[test]
fn refusal_record_round_trips() {
    let fixture = fixture("analyzed");
    record_owner_voice_release_refused(
        &fixture.journal,
        "chronicle/20261020/watch/120000_300",
        "120000_300.m4a",
        OwnerVoiceRefusal::BytesChanged,
        at(),
    )
    .unwrap();
    let line = fs::read_to_string(fixture.segment.join(EVENTS_FILE)).unwrap();
    let row: Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
    assert_eq!(row["event"], "original_release_refused");
    assert_eq!(row["reason"], "bytes_changed");
    assert_eq!(row["class"], "owner_voice_raw_release");
    assert_eq!(row["ts"], at().timestamp_millis());
    assert!(
        retention_events_for_name(&fixture.segment, "120000_300.m4a")
            .unwrap()
            .refused
    );
}

#[test]
fn deletion_state_truth_table() {
    let owed = fixture("analyzed");
    assert_eq!(
        owner_voice_deletion_state(&owed.segment, &owed.jsonl),
        OwedState::Owed
    );
    let empty = fixture("empty");
    assert_eq!(
        owner_voice_deletion_state(&empty.segment, &empty.jsonl),
        OwedState::Owed
    );

    // The predicate never reads the original's bytes.
    fs::write(&owed.original, b"different").unwrap();
    assert_eq!(
        owner_voice_deletion_state(&owed.segment, &owed.jsonl),
        OwedState::Owed
    );

    let gone = fixture("analyzed");
    fs::remove_file(&gone.original).unwrap();
    assert_eq!(
        owner_voice_deletion_state(&gone.segment, &gone.jsonl),
        OwedState::NotOwed
    );

    let failed = fixture("failed");
    assert_eq!(
        owner_voice_deletion_state(&failed.segment, &failed.jsonl),
        OwedState::NotOwed
    );

    let rows_gone = fixture("analyzed");
    let header = fs::read_to_string(&rows_gone.jsonl).unwrap();
    fs::write(
        &rows_gone.jsonl,
        format!("{}\n", header.lines().next().unwrap()),
    )
    .unwrap();
    assert_eq!(
        owner_voice_deletion_state(&rows_gone.segment, &rows_gone.jsonl),
        OwedState::NotOwed
    );

    let other_raw = fixture("empty");
    let text = fs::read_to_string(&other_raw.jsonl).unwrap();
    fs::write(
        &other_raw.jsonl,
        text.replacen("\"raw\":\"120000_300.m4a\"", "\"raw\":\"other.m4a\"", 1),
    )
    .unwrap();
    assert_eq!(
        owner_voice_deletion_state(&other_raw.segment, &other_raw.jsonl),
        OwedState::NotOwed
    );

    let unfiltered = fixture("empty");
    let text = fs::read_to_string(&unfiltered.jsonl).unwrap();
    let mut header: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    header["_solstone_processing"]
        .as_object_mut()
        .unwrap()
        .remove("owner_voice");
    fs::write(&unfiltered.jsonl, format!("{header}\n")).unwrap();
    assert_eq!(
        owner_voice_deletion_state(&unfiltered.segment, &unfiltered.jsonl),
        OwedState::NotOwed
    );

    let invalid = fixture("empty");
    let text = fs::read_to_string(&invalid.jsonl).unwrap();
    fs::write(
        &invalid.jsonl,
        text.replacen("\"version\":1", "\"version\":2", 1),
    )
    .unwrap();
    assert_eq!(
        owner_voice_deletion_state(&invalid.segment, &invalid.jsonl),
        OwedState::NotOwed
    );

    for (event, reason) in [
        ("original_deleted", "x"),
        ("original_release_refused", "never_heard_of"),
    ] {
        let recorded = fixture("analyzed");
        fs::write(
            recorded.segment.join(EVENTS_FILE),
            format!("{}\n", json!({"tract": "retention", "event": event, "name": "120000_300.m4a", "class": "owner_segment_removal", "reason": reason})),
        )
        .unwrap();
        assert_eq!(
            owner_voice_deletion_state(&recorded.segment, &recorded.jsonl),
            OwedState::NotOwed
        );
    }

    let unreadable = fixture("analyzed");
    fs::create_dir(unreadable.segment.join(EVENTS_FILE)).unwrap();
    assert_eq!(
        owner_voice_deletion_state(&unreadable.segment, &unreadable.jsonl),
        OwedState::Unknown
    );
    assert!(!owner_voice_deletion_owed(
        &unreadable.segment,
        &unreadable.jsonl
    ));
}

#[test]
fn owed_days_cover_today_and_the_29_before() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
    let days: Vec<_> = owner_voice_owed_days(today).collect();
    assert_eq!(days.len(), 30);
    assert_eq!(days.first(), Some(&today));
    assert!(days.contains(&NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()));
    assert!(!days.contains(&NaiveDate::from_ymd_opt(2026, 9, 20).unwrap()));
}

#[test]
fn finishing_deletes_and_records_in_both_segment_layouts() {
    for segment_rel in [
        "chronicle/20261020/watch/120000_300",
        "chronicle/20261020/120000_300",
    ] {
        let fixture = fixture_in(segment_rel, "analyzed");
        assert_eq!(
            finish(&fixture),
            OwnerVoiceDeletion::Deleted,
            "{segment_rel}"
        );
        assert!(!fixture.original.exists());
        let events = fs::read_to_string(fixture.segment.join(EVENTS_FILE)).unwrap();
        assert_eq!(events.matches("owner_voice_raw_release").count(), 1);
        assert_eq!(finish(&fixture), OwnerVoiceDeletion::NotOwed);
    }
}

#[test]
fn a_changed_original_is_refused_once_and_kept() {
    for (bytes, reason) in [
        (
            &b"synthetic audio bytez"[..],
            OwnerVoiceRefusal::BytesChanged,
        ),
        (&b"shorter"[..], OwnerVoiceRefusal::SizeChanged),
    ] {
        let fixture = fixture("analyzed");
        fs::write(&fixture.original, bytes).unwrap();
        assert_eq!(finish(&fixture), OwnerVoiceDeletion::Refused(reason));
        assert_eq!(fs::read(&fixture.original).unwrap(), bytes);
        assert_eq!(finish(&fixture), OwnerVoiceDeletion::NotOwed);
        let events = fs::read_to_string(fixture.segment.join(EVENTS_FILE)).unwrap();
        assert_eq!(events.matches("original_release_refused").count(), 1);
        assert!(!events.contains("original_deleted"));
    }
}

#[test]
fn a_commit_from_other_claims_proves_nothing() {
    let fixture = fixture("analyzed");
    let other = fixture_in("chronicle/20261020/watch/130000_300", "analyzed");
    let options = LockOptions {
        timeout: Duration::ZERO,
        ..LockOptions::default()
    };
    let claims = OutputClaims::acquire([fixture.jsonl.clone()], options).unwrap();
    let other_claims = OutputClaims::acquire([other.jsonl.clone()], options).unwrap();
    let other_commit = other_claims.commit(&other.segment).unwrap();
    let outcome = finish_owner_voice_deletion(
        &fixture.journal,
        &fixture.jsonl,
        &claims,
        &other_commit,
        at(),
    );
    assert!(matches!(outcome, OwnerVoiceDeletion::NotProvable(_)));
    assert_eq!(fs::read(&fixture.original).unwrap(), AUDIO);
    assert!(!fixture.segment.join(EVENTS_FILE).exists());
}

#[test]
fn a_busy_journal_leaves_the_deletion_owed() {
    let fixture = fixture("analyzed");
    let root = JournalRoot::open(&fixture.journal).unwrap();
    let lease = ActivityLease::try_acquire(Arc::new(root)).unwrap();
    assert!(matches!(finish(&fixture), OwnerVoiceDeletion::Owed(_)));
    assert_eq!(fs::read(&fixture.original).unwrap(), AUDIO);
    assert!(owner_voice_deletion_owed(&fixture.segment, &fixture.jsonl));
    drop(lease);
    assert_eq!(finish(&fixture), OwnerVoiceDeletion::Deleted);
}

#[test]
fn the_daily_walk_finishes_owed_deletions_in_its_window_only() {
    let fixture = fixture_in("chronicle/20261020/watch/120000_300", "analyzed");
    let journal = &fixture.journal;
    let old = journal.join("chronicle/20260920/watch/120000_300");
    fs::create_dir_all(&old).unwrap();
    fs::copy(&fixture.original, old.join("120000_300.m4a")).unwrap();
    fs::copy(&fixture.jsonl, old.join("120000_300.jsonl")).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();

    // Busy at first: the deletion stays owed and is retried within the run.
    let root = JournalRoot::open(journal).unwrap();
    let lease = std::cell::RefCell::new(Some(ActivityLease::try_acquire(Arc::new(root)).unwrap()));
    let calls = std::cell::Cell::new(0);
    let counts = finish_owed_owner_voice_deletions(journal, today, 2, Duration::ZERO, &|| {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            lease.borrow_mut().take();
        }
        at()
    });
    assert_eq!(
        calls.get(),
        2,
        "the first attempt was busy and the retry deleted it"
    );
    assert_eq!(counts.deleted, 1);
    assert_eq!(counts.owed, 0);
    assert!(!fixture.original.exists());
    // 2026-09-20 is 30 days back: outside the window, left alone.
    assert!(old.join("120000_300.m4a").exists());

    let again = finish_owed_owner_voice_deletions(journal, today, 0, Duration::ZERO, &at);
    assert_eq!(again, OwedRunCounts::default());
}

#[test]
fn an_absent_original_is_not_owed() {
    let fixture = fixture("analyzed");
    fs::remove_file(&fixture.original).unwrap();
    assert_eq!(finish(&fixture), OwnerVoiceDeletion::NotOwed);
}
