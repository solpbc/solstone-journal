// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Live-journal snapshot classification for concurrent recording and appends.
//!
//! Offline strict exports freeze every file in the journal and refuse any
//! change anywhere in the tree. Prepared live exports (`SnapshotMode::LiveJournal`)
//! allow normal live journal activity to continue while preserving a well-defined
//! `t=0` archive:
//!
//! - Small live-updated metadata files (`streams/*.json`, `stream.updated`,
//!   `link/authorized_clients.json`, etc.) are captured into memory at `t=0`
//!   within a bounded budget so atomic rewrites after `t=0` do not abort export.
//! - Identified append-only `.jsonl` logs may grow after `t=0` while retaining
//!   the exact same file identity; encoding copies only the `t=0` prefix
//!   (`0..expected_size`) and requires a clean newline boundary when the file grew.
//! - New sibling directories/files created after `t=0` are outside the frozen
//!   `t=0` inventory and do not invalidate it.
//! - Unexpected mutations (truncation, deletion, identity replacement, or edits
//!   to immutable owner files) still fail cleanly with `SourceChanged`.

/// How an [`crate::ArchiveSource`] tolerates concurrent live-journal writes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SnapshotMode {
    /// Strict offline freeze: any mutation anywhere in the inventory fails.
    #[default]
    Strict,
    /// Live-journal snapshot: tolerates normal live metadata replacement,
    /// append-only `.jsonl` growth, and newly created sibling segments while
    /// failing cleanly on unexpected mutations.
    LiveJournal,
}

/// Per-file memory cap for snapshotting live-updated metadata at `t=0`.
pub(crate) const LIVE_METADATA_MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Aggregate memory cap across all snapshotted live metadata files at `t=0`.
pub(crate) const LIVE_METADATA_TOTAL_BUDGET_BYTES: u64 = 16 * 1024 * 1024;

fn normalized_member(member: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(windows) {
        std::borrow::Cow::Owned(member.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(member)
    }
}

fn is_eight_digit_day(day: &str) -> bool {
    day.len() == 8 && day.bytes().all(|b| b.is_ascii_digit())
}

/// Small operational/state files atomically replaced during normal live operation.
pub(crate) fn is_live_snapshottable_metadata(member: &str) -> bool {
    let member = normalized_member(member);
    let m = member.as_ref();
    if matches!(
        m,
        "link/authorized_clients.json"
            | "link/client-descriptions.json"
            | "awareness/current.json"
            | "awareness/speaker_candidates.json"
            | "awareness/speaker_candidates_backfill.json"
            | "config/journal.json"
            | "config/schedules.json"
            | "config/convey.json"
            | "health/work-inventory.json"
            | "health/work-intent.json"
            | "health/cortex-use-id.json"
            | "health/owner-asks.json"
            | "health/owner-notices.json"
            | "health/maintenance/daily_schedule.json"
    ) {
        return true;
    }
    let parts: Vec<&str> = m.split('/').collect();
    match parts.as_slice() {
        ["streams", leaf] if leaf.ends_with(".json") => true,
        ["chronicle", day, "health", leaf]
            if is_eight_digit_day(day)
                && matches!(
                    *leaf,
                    "stream.updated" | "daily.updated" | "body.updated" | "location.json"
                ) =>
        {
            true
        }
        ["chronicle", day, "health", "daily-units", leaf]
            if is_eight_digit_day(day) && leaf.ends_with(".json") =>
        {
            true
        }
        _ => false,
    }
}

/// Journal-managed derived and operational state files routinely replaced
/// atomically (temp-file + rename) by live processing (speaker resolution,
/// thinking, indexing, health markers, entity/facet updates, stream/config
/// updates, import metadata, and per-day derived JSONs).
///
/// In `SnapshotMode::LiveJournal`, opening one of these files binds to a single
/// complete current regular-file version verified through its `t=0` ancestor
/// directories, and final revalidation requires only that the verified route
/// still resolves to a regular file (never a symlink or deleted file).
pub(crate) fn is_live_atomic_rewritable(member: &str) -> bool {
    if is_live_append_only_jsonl(member) {
        return false;
    }
    if is_live_snapshottable_metadata(member) {
        return true;
    }
    let member = normalized_member(member);
    let m = member.as_ref();
    let parts: Vec<&str> = m.split('/').collect();
    match parts.as_slice() {
        [
            "awareness" | "health" | "streams" | "config" | "link" | "backup" | "solstone"
            | "entities" | "facets" | "speakers",
            ..,
        ] => true,
        ["imports", .., leaf] if leaf.ends_with(".json") => true,
        ["chronicle", day, "health", ..] if is_eight_digit_day(day) => true,
        ["chronicle", day, .., leaf]
            if is_eight_digit_day(day)
                && (leaf.ends_with(".json") || leaf.ends_with(".updated")) =>
        {
            true
        }
        ["chronicle", day, _, _, "talents", ..] if is_eight_digit_day(day) => true,
        _ => false,
    }
}

/// Identified append-only `.jsonl` logs whose `0..expected_size` prefix stays
/// intact as new lines are appended during live capture or thinking.
pub(crate) fn is_live_append_only_jsonl(member: &str) -> bool {
    let member = normalized_member(member);
    let m = member.as_ref();
    if !m.ends_with(".jsonl") {
        return false;
    }
    if m == "events.jsonl" {
        return true;
    }
    let parts: Vec<&str> = m.split('/').collect();
    match parts.as_slice() {
        ["awareness" | "tokens" | "logs" | "speakers", _] => true,
        ["config", "actions", _] => true,
        ["health", ..] => true,
        ["chronicle", day, ..] if is_eight_digit_day(day) => true,
        ["facets", _, "activities" | "experiences" | "logs", _] => true,
        ["entities", ..] => true,
        _ => false,
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "unit tests for live-journal snapshot encoding"
)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs::{self, OpenOptions};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use zip::ZipArchive;

    use crate::{
        ArchiveError, ArchiveSource, EncodeArchiveError, EncodeArchiveRequest, EncodeOptions,
        encode_archive_with_options,
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-archive-live-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn live_journal_tolerates_normal_recording_appends_and_metadata_replacement() {
        let work = TempDir::new("normal-live");
        let root = work.path().join("journal");
        fs::create_dir_all(&root).unwrap();

        let write = |rel: &str, bytes: &[u8]| {
            let full = root.join(rel);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, bytes).unwrap();
        };

        write("config/journal.json", br#"{"schema":1}"#);
        write("streams/mic.json", br#"{"name":"mic","seq":1}"#);
        write("link/authorized_clients.json", br#"{"clients":["c1"]}"#);
        write("chronicle/20261126/health/stream.updated", b"1000");
        write(
            "chronicle/20261126/mic/090000_60/audio.flac",
            b"immutable-audio-bytes",
        );
        write(
            "chronicle/20261126/mic/090000_60/audio.jsonl",
            b"{\"_solstone_processing\":{\"schema\":1}}\n{\"text\":\"line1\"}\n",
        );
        write("events.jsonl", b"{\"event\":\"boot\"}\n");

        let source = ArchiveSource::open_live(&root).expect("open live source");

        // Simulate normal live activity AFTER t=0 inventory:
        // 1. New capture segment created.
        write(
            "chronicle/20261126/mic/090100_60/audio.flac",
            b"new-segment-after-t0",
        );
        // 2. Small live metadata atomically replaced via rename.
        let tmp_stream = root.join("streams/mic.json.tmp");
        fs::write(&tmp_stream, br#"{"name":"mic","seq":2,"updated":true}"#).unwrap();
        fs::rename(&tmp_stream, root.join("streams/mic.json")).unwrap();
        let tmp_marker = root.join("chronicle/20261126/health/stream.updated.tmp");
        fs::write(&tmp_marker, b"2000").unwrap();
        fs::rename(
            &tmp_marker,
            root.join("chronicle/20261126/health/stream.updated"),
        )
        .unwrap();
        // 3. Append-only JSONL files grow with complete lines (same inode).
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(root.join("events.jsonl"))
                .unwrap();
            f.write_all(b"{\"event\":\"later\"}\n").unwrap();
        }
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(root.join("chronicle/20261126/mic/090000_60/audio.jsonl"))
                .unwrap();
            f.write_all(b"{\"text\":\"line2-after-t0\"}\n").unwrap();
        }

        let out_path = work.path().join("live.zip");
        let mut out_file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&out_path)
            .unwrap();
        let request = EncodeArchiveRequest {
            source: &source,
            solstone_version: "2.0.93",
            exported_at: "2026-11-26T12:00:00Z",
            day_window: None,
        };
        let mut omitted = BTreeSet::new();
        omitted.insert("chronicle/20261126/mic/090000_60/audio.flac".to_owned());
        let mut progress_ticks = 0u64;
        let mut on_progress = |_p| {
            progress_ticks += 1;
        };
        encode_archive_with_options(
            &request,
            &mut out_file,
            EncodeOptions {
                omitted_members: Some(&omitted),
                cancel: None,
                on_progress: Some(&mut on_progress),
                check_abort: None,
            },
        )
        .expect("live journal encode succeeds");
        assert!(progress_ticks > 0);

        let reader = fs::File::open(&out_path).unwrap();
        let mut archive = ZipArchive::new(reader).expect("valid zip");
        // Omitted processed media is absent; post-t=0 segment is absent.
        assert!(
            archive
                .by_name("chronicle/20261126/mic/090000_60/audio.flac")
                .is_err()
        );
        assert!(
            archive
                .by_name("chronicle/20261126/mic/090100_60/audio.flac")
                .is_err()
        );
        // Snapshotted metadata holds exact t=0 bytes.
        let mut stream_json = String::new();
        archive
            .by_name("streams/mic.json")
            .unwrap()
            .read_to_string(&mut stream_json)
            .unwrap();
        assert_eq!(stream_json, r#"{"name":"mic","seq":1}"#);
        // Appended JSONL holds exact t=0 prefix without post-t=0 lines.
        let mut events = String::new();
        archive
            .by_name("events.jsonl")
            .unwrap()
            .read_to_string(&mut events)
            .unwrap();
        assert_eq!(events, "{\"event\":\"boot\"}\n");
    }

    #[test]
    fn live_journal_fails_cleanly_on_unexpected_mutation_and_cancel_and_abort() {
        let work = TempDir::new("unexpected");
        let root = work.path().join("journal");
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(root.join("notes/important.txt"), b"original-bytes").unwrap();
        fs::write(root.join("events.jsonl"), b"{\"a\":1}\n{\"b\":2}\n").unwrap();

        // 1. Unexpected deletion of an immutable owner file fails with SourceChanged.
        let source = ArchiveSource::open_live(&root).unwrap();
        fs::remove_file(root.join("notes/important.txt")).unwrap();
        let mut out1 = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(work.path().join("fail1.zip"))
            .unwrap();
        let req1 = EncodeArchiveRequest {
            source: &source,
            solstone_version: "2.0.93",
            exported_at: "2026-11-26T12:00:00Z",
            day_window: None,
        };
        let err1 = encode_archive_with_options(&req1, &mut out1, EncodeOptions::default())
            .expect_err("deleted owner file fails");
        assert!(matches!(
            err1,
            EncodeArchiveError::Source(ArchiveError::SourceChanged { .. })
        ));

        // 2. Truncation of an append-only JSONL fails with SourceChanged.
        fs::write(root.join("notes/important.txt"), b"original-bytes").unwrap();
        let source2 = ArchiveSource::open_live(&root).unwrap();
        fs::write(root.join("events.jsonl"), b"{\"a\":1}\n").unwrap();
        let mut out2 = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(work.path().join("fail2.zip"))
            .unwrap();
        let req2 = EncodeArchiveRequest {
            source: &source2,
            solstone_version: "2.0.93",
            exported_at: "2026-11-26T12:00:00Z",
            day_window: None,
        };
        let err2 = encode_archive_with_options(&req2, &mut out2, EncodeOptions::default())
            .expect_err("truncated jsonl fails");
        assert!(matches!(
            err2,
            EncodeArchiveError::Source(ArchiveError::SourceChanged { .. })
        ));

        // 3. Cooperative cancel and external abort both halt cleanly.
        let source3 = ArchiveSource::open_live(&root).unwrap();
        let cancel = AtomicBool::new(true);
        let mut out3 = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(work.path().join("fail3.zip"))
            .unwrap();
        let req3 = EncodeArchiveRequest {
            source: &source3,
            solstone_version: "2.0.93",
            exported_at: "2026-11-26T12:00:00Z",
            day_window: None,
        };
        let err3 = encode_archive_with_options(
            &req3,
            &mut out3,
            EncodeOptions {
                cancel: Some(&cancel),
                ..EncodeOptions::default()
            },
        )
        .expect_err("cancelled");
        assert!(matches!(err3, EncodeArchiveError::Cancelled));

        let mut out4 = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(work.path().join("fail4.zip"))
            .unwrap();
        let mut abort_check = || Err("trash_erased".to_owned());
        let err4 = encode_archive_with_options(
            &req3,
            &mut out4,
            EncodeOptions {
                check_abort: Some(&mut abort_check),
                ..EncodeOptions::default()
            },
        )
        .expect_err("aborted");
        assert!(matches!(err4, EncodeArchiveError::Aborted(ref msg) if msg == "trash_erased"));
    }

    #[test]
    fn live_journal_tolerates_atomic_rewrite_of_derived_files_like_speaker_candidates() {
        let work = TempDir::new("derived-atomic");
        let root = work.path().join("journal");
        fs::create_dir_all(&root).unwrap();

        let write = |rel: &str, bytes: &[u8]| {
            let full = root.join(rel);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, bytes).unwrap();
        };

        // 1. Small speaker_candidates.json (covered by snapshot) AND a >4 MiB
        //    derived file under awareness/ + entities/alice/entity.json (covered
        //    by current-whole-version atomic replacement without snapshot).
        write(
            "awareness/speaker_candidates.json",
            br#"{"version":1,"candidates":["a"]}"#,
        );
        let large_v1 = vec![b'1'; 5 * 1024 * 1024];
        write("awareness/speaker_candidates_large.json", &large_v1);
        write(
            "entities/alice/entity.json",
            br#"{"id":"alice","revision":1}"#,
        );

        let source = ArchiveSource::open_live(&root).expect("open live source");

        // Atomically replace all three via temp-file + rename after t=0.
        let atomic_swap = |rel: &str, bytes: &[u8]| {
            let target = root.join(rel);
            let tmp = root.join(format!("{rel}.tmp"));
            fs::write(&tmp, bytes).unwrap();
            fs::rename(&tmp, &target).unwrap();
        };
        atomic_swap(
            "awareness/speaker_candidates.json",
            br#"{"version":2,"candidates":["a","b"]}"#,
        );
        let large_v2 = vec![b'2'; 5 * 1024 * 1024 + 123];
        atomic_swap("awareness/speaker_candidates_large.json", &large_v2);
        atomic_swap(
            "entities/alice/entity.json",
            br#"{"id":"alice","revision":2,"extra":true}"#,
        );

        let out_path = work.path().join("derived.zip");
        let mut out_file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&out_path)
            .unwrap();
        let req = EncodeArchiveRequest {
            source: &source,
            solstone_version: "2.0.93",
            exported_at: "2026-11-26T12:00:00Z",
            day_window: None,
        };
        encode_archive_with_options(&req, &mut out_file, EncodeOptions::default())
            .expect("live journal encode tolerates atomic derived rewrites");

        // Another atomic replace after encode must still revalidate cleanly.
        atomic_swap(
            "entities/alice/entity.json",
            br#"{"id":"alice","revision":3}"#,
        );
        source
            .revalidate()
            .expect("revalidate tolerates subsequent atomic derived rewrite");

        let mut archive = ZipArchive::new(fs::File::open(&out_path).unwrap()).unwrap();
        let mut sc = String::new();
        archive
            .by_name("awareness/speaker_candidates.json")
            .unwrap()
            .read_to_string(&mut sc)
            .unwrap();
        assert_eq!(sc, r#"{"version":1,"candidates":["a"]}"#);

        let mut large_out = Vec::new();
        archive
            .by_name("awareness/speaker_candidates_large.json")
            .unwrap()
            .read_to_end(&mut large_out)
            .unwrap();
        assert_eq!(large_out, large_v2);

        let mut entity_out = String::new();
        archive
            .by_name("entities/alice/entity.json")
            .unwrap()
            .read_to_string(&mut entity_out)
            .unwrap();
        assert_eq!(entity_out, r#"{"id":"alice","revision":2,"extra":true}"#);

        // Deleting a derived file still fails cleanly with SourceChanged.
        fs::remove_file(root.join("entities/alice/entity.json")).unwrap();
        assert!(matches!(
            source.revalidate(),
            Err(ArchiveError::SourceChanged { .. })
        ));
    }
}
