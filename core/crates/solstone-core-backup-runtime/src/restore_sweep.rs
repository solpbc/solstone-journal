// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What finishes a restore during setup: afterwards the journal is the snapshot.
//!
//! A restore lands on a journal that has only just been set up, so what the
//! snapshot doesn't hold is setup's own files. restic's dry-run report names
//! them after a clean restore and check, and this removes them. What backups
//! don't carry stays where it is. Every step can run again.

use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::atomic::{AtomicWriteOptions, atomic_replace};
use solstone_core_journal_io::copy_excludes::INIT_RESTORE_FILE;
use solstone_core_journal_io::copy_excludes::{copy_excluded, copy_file_excluded};

/// Room kept free beyond the restored bytes.
const FREE_SPACE_MARGIN: u64 = 1_000_000_000;

/// Files a running journal holds or needs in place, beyond what backups omit.
/// Left alone by the sweep after the restore, and not counted as a change.
const LIVE_FILES: &[&str] = &[
    "health/service.log",
    "health/direct-door.json",
    "backup/teardown-recovery.json",
];
const LIVE_PREFIXES: &[&str] = &[
    "health/launchd-",
    "health/providers/",
    "backup/teardown-preparations/",
];

/// Mark this journal as restoring during setup, before the restore starts.
pub fn mark_init_restore(journal: &Path) -> io::Result<()> {
    let path = journal.join(INIT_RESTORE_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_replace(&path, b"{}", AtomicWriteOptions { mode: Some(0o600) }).map_err(io::Error::other)
}

pub fn is_init_restore(journal: &Path) -> bool {
    journal.join(INIT_RESTORE_FILE).exists()
}

/// Whether setup has finished here. A restored config says so too, which is
/// why a retry of a restore already recorded doesn't ask.
pub fn setup_finished(journal: &Path) -> bool {
    fs::read(journal.join("config/journal.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|config| config.get("setup")?.get("completed_at")?.as_f64())
        .is_some_and(|completed_at| completed_at > 0.0)
}

/// Whether this restore ends by removing what the snapshot doesn't hold. A
/// marker a stopped attempt left behind can't reach a journal in use.
pub(crate) fn sweeps(journal: &Path, retrying: bool) -> bool {
    is_init_restore(journal) && (retrying || !setup_finished(journal))
}

/// Remove what a restore during setup left, so the journal sets up as new.
/// What backups don't carry stays.
pub fn abandon_init_restore(journal: &Path) -> io::Result<()> {
    if !is_init_restore(journal) {
        return Err(io::Error::other("no restore during setup to abandon"));
    }
    remove_journal_files(journal, journal)?;
    use solstone_core_journal_io::whole_journal_restore::{
        WholeJournalRestoreReadiness, clear_whole_journal_restore, read_whole_journal_restore,
    };
    if let WholeJournalRestoreReadiness::Pending(record) = read_whole_journal_restore(journal) {
        clear_whole_journal_restore(journal, &record)
            .map_err(|_| io::Error::other("not cleared"))?;
    }
    finish_init_restore(journal);
    Ok(())
}

fn remove_journal_files(journal: &Path, directory: &Path) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path
            .strip_prefix(journal)
            .map_err(io::Error::other)?
            .to_string_lossy()
            .replace('\\', "/");
        if kept(&rel) || rel == "backup-activity.lock" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            remove_journal_files(journal, &path)?;
            // Not empty means it holds something a restore leaves alone.
            let _ = fs::remove_dir(&path);
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// A swept journal is the snapshot: nothing is left for a retry to remove.
pub(crate) fn finish_init_restore(journal: &Path) {
    if let Err(error) = fs::remove_file(journal.join(INIT_RESTORE_FILE))
        && error.kind() != io::ErrorKind::NotFound
    {
        log::warn!("restore: the setup restore marker was not cleared: {error}");
    }
}

/// What restic says a restore would do, journal-relative.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Report {
    pub updated: Vec<String>,
    pub restored: Vec<String>,
    pub deleted: Vec<String>,
    pub bytes_to_restore: u64,
    /// Names restic could only print with replacement characters.
    pub unreadable_names: usize,
}

/// Parse a `restic restore --dry-run --json -v -v` report. `None` unless the
/// whole report is there: an empty list must never read as nothing to do.
pub(crate) fn parse_report(stdout: &str, target: &Path) -> Option<Report> {
    let target = normalize(&target.display().to_string());
    let target = target.trim_end_matches('/');
    let mut report = Report::default();
    let mut summary = false;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line).ok()?;
        match value.get("message_type").and_then(Value::as_str) {
            Some("verbose_status") => {
                let action = value.get("action").and_then(Value::as_str)?;
                let item = normalize(value.get("item").and_then(Value::as_str)?);
                let rel = match action {
                    "deleted" => {
                        let rest = item.strip_prefix(target)?;
                        if !rest.is_empty() && !rest.starts_with('/') {
                            return None;
                        }
                        rest.trim_start_matches('/').to_owned()
                    }
                    _ => item.trim_start_matches('/').to_owned(),
                };
                if rel.is_empty() {
                    continue;
                }
                if !stays_inside(&rel) {
                    return None;
                }
                if rel.contains('\u{fffd}') {
                    report.unreadable_names += 1;
                    continue;
                }
                match action {
                    "updated" => report.updated.push(rel),
                    "restored" => report.restored.push(rel),
                    "deleted" => report.deleted.push(rel),
                    _ => {}
                }
            }
            Some("summary") => {
                summary = true;
                report.bytes_to_restore = value
                    .get("bytes_restored")
                    .or_else(|| value.get("total_bytes"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
            }
            _ => {}
        }
    }
    summary.then_some(report)
}

/// A path as restic prints it, in the form these reports compare. Only Windows
/// has another separator: on Unix a backslash is part of the name.
fn normalize(path: &str) -> String {
    if cfg!(windows) {
        let path = path.replace('\\', "/");
        path.strip_prefix("//?/").unwrap_or(&path).to_owned()
    } else {
        path.to_owned()
    }
}

/// A name in a report is the snapshot's, and only ever lands under the journal.
fn stays_inside(rel: &str) -> bool {
    rel.split('/').all(|component| component != "..")
}

/// Whether a backup carries this path at all.
fn not_backed_up(rel: &str) -> bool {
    copy_excluded(rel) || copy_file_excluded(rel)
}

/// Whether the sweep after a restore leaves this path where it is.
pub(crate) fn kept(rel: &str) -> bool {
    not_backed_up(rel)
        || LIVE_FILES.contains(&rel)
        || LIVE_PREFIXES.iter().any(|prefix| rel.starts_with(prefix))
}

/// Whether the restored bytes fit, with the margin. Free space that can't be
/// read doesn't refuse: nothing else about the restore depends on it.
pub(crate) fn fits(free: Option<u64>, restoring: u64) -> bool {
    free.is_none_or(|free| free >= restoring.saturating_add(FREE_SPACE_MARGIN))
}

/// Remove what the snapshot doesn't hold, then the directories that leaves
/// empty. Returns how many files stayed behind.
pub(crate) fn remove_extras(journal: &Path, report: &Report) -> usize {
    let mut failed = 0;
    let mut directories = Vec::new();
    for rel in report.deleted.iter().filter(|rel| !kept(rel)) {
        let path = journal.join(rel);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_dir() {
            directories.push(path);
            continue;
        }
        if let Err(error) = fs::remove_file(&path) {
            log::warn!("restore: setup's file could not be removed: {error}");
            failed += 1;
        }
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        // Not empty means it holds something a restore leaves alone.
        let _ = fs::remove_dir(directory);
    }
    failed
}

/// Whether a report taken after the restore shows the journal is the snapshot.
/// restic lists every directory as `restored` when it would only reset its
/// times, so a directory there says nothing.
pub(crate) fn matches_snapshot(journal: &Path, report: &Report) -> bool {
    let is_directory = |rel: &String| {
        fs::symlink_metadata(journal.join(rel)).is_ok_and(|meta| meta.file_type().is_dir())
    };
    report.unreadable_names == 0
        && report.updated.iter().all(|rel| kept(rel))
        && report
            .restored
            .iter()
            .all(|rel| kept(rel) || is_directory(rel))
}

/// Free bytes on the journal's volume, when that can be read.
pub(crate) fn free_bytes(journal: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let stat = nix::sys::statvfs::statvfs(journal).ok()?;
        #[allow(clippy::unnecessary_cast)]
        let blocks = stat.blocks_available() as u64;
        Some(blocks.saturating_mul(stat.fragment_size() as u64))
    }
    #[cfg(windows)]
    {
        solstone_core_journal_io::windows_available_disk_bytes(journal).ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = journal;
        None
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]
mod tests {
    use super::*;
    use std::fs;

    fn report(lines: &[&str]) -> String {
        let mut text = lines.join("\n");
        text.push_str("\n{\"message_type\":\"summary\",\"bytes_restored\":5}\n");
        text
    }

    #[test]
    fn a_report_needs_its_summary_and_every_line() {
        let target = Path::new("/j");
        let whole = report(&[
            r#"{"message_type":"verbose_status","action":"updated","item":"/a"}"#,
            r#"{"message_type":"verbose_status","action":"deleted","item":"/j/sub/b"}"#,
            r#"{"message_type":"verbose_status","action":"unchanged","item":"/c"}"#,
        ]);
        let parsed = parse_report(&whole, target).unwrap();
        assert_eq!(parsed.updated, ["a"]);
        assert_eq!(parsed.deleted, ["sub/b"]);
        assert_eq!(parsed.bytes_to_restore, 5);
        assert_eq!(
            parse_report(
                r#"{"message_type":"verbose_status","action":"updated","item":"/a"}"#,
                target
            ),
            None,
            "no summary, no report"
        );
        assert_eq!(parse_report("not json\n", target), None);
        let outside =
            report(&[r#"{"message_type":"verbose_status","action":"deleted","item":"/other/b"}"#]);
        assert_eq!(parse_report(&outside, target), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_backslash_is_part_of_a_name_on_unix() {
        let text =
            report(&[r#"{"message_type":"verbose_status","action":"updated","item":"/a\\b"}"#]);
        assert_eq!(
            parse_report(&text, Path::new("/j")).unwrap().updated,
            ["a\\b"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_verbatim_target_still_reads_as_the_journal() {
        let text = report(&[
            r#"{"message_type":"verbose_status","action":"deleted","item":"C:\\j\\sub\\b"}"#,
        ]);
        let parsed = parse_report(&text, Path::new(r"\\?\C:\j")).unwrap();
        assert_eq!(parsed.deleted, ["sub/b"]);
    }

    #[test]
    fn a_name_that_leaves_the_journal_is_no_report() {
        let text = report(&[
            r#"{"message_type":"verbose_status","action":"updated","item":"/../outside"}"#,
        ]);
        assert_eq!(parse_report(&text, Path::new("/j")), None);
    }

    #[test]
    fn a_name_restic_cannot_print_blocks_the_success_claim() {
        let text = report(&[
            "{\"message_type\":\"verbose_status\",\"action\":\"updated\",\"item\":\"/bad\u{fffd}name\"}",
        ]);
        let parsed = parse_report(&text, Path::new("/j")).unwrap();
        assert_eq!(parsed.unreadable_names, 1);
        assert!(!matches_snapshot(Path::new("/j"), &parsed));
    }

    #[test]
    fn what_backups_do_not_carry_stays_and_owner_paths_named_alike_do_not() {
        for path in [
            "cache/models/x",
            "indexer/journal.sqlite",
            "config/chatgpt-sign-in.json",
            "health/service.log",
            "health/providers/local.json",
            "health/init-restore.json",
        ] {
            assert!(kept(path), "{path}");
        }
        for path in [
            "entities/cache/entity.json",
            "chronicle/20260101/x/audio.flac",
            "config/removals/a.json",
            ".trash/abc/item.json",
            "backup/hosted/binding.json",
        ] {
            assert!(!kept(path), "{path}");
        }
    }

    #[test]
    fn what_the_snapshot_lacks_goes_and_emptied_folders_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("sub/deep")).unwrap();
        fs::create_dir_all(root.join("cache")).unwrap();
        fs::write(root.join("sub/deep/new"), b"new").unwrap();
        fs::write(root.join("cache/model"), b"kept").unwrap();
        let plan = Report {
            deleted: vec![
                "sub/deep/new".into(),
                "sub/deep".into(),
                "sub".into(),
                "cache/model".into(),
                "cache".into(),
            ],
            ..Report::default()
        };
        assert_eq!(remove_extras(root, &plan), 0);
        assert!(!root.join("sub").exists());
        assert_eq!(fs::read(root.join("cache/model")).unwrap(), b"kept");
    }

    #[test]
    fn free_space_covers_the_restore() {
        assert!(fits(None, 10));
        assert!(fits(Some(FREE_SPACE_MARGIN + 10), 10));
        assert!(!fits(Some(FREE_SPACE_MARGIN + 9), 10));
    }

    #[test]
    fn a_change_left_after_the_restore_is_not_a_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(root.join("left"), b"x").unwrap();
        let fine = Report {
            updated: vec!["health/service.log".into()],
            restored: vec!["notes".into()],
            ..Report::default()
        };
        assert!(matches_snapshot(root, &fine));
        let changed = Report {
            updated: vec!["config/journal.json".into()],
            ..Report::default()
        };
        assert!(!matches_snapshot(root, &changed));
        let missing = Report {
            restored: vec!["left".into()],
            ..Report::default()
        };
        assert!(!matches_snapshot(root, &missing));
    }

    #[test]
    fn a_marker_left_on_a_finished_journal_never_sweeps() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        mark_init_restore(root).unwrap();
        assert!(sweeps(root, false));
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":5}}"#,
        )
        .unwrap();
        assert!(!sweeps(root, false));
        assert!(
            sweeps(root, true),
            "a recorded retry sees the snapshot's own config"
        );
    }

    #[test]
    fn abandoning_leaves_a_journal_with_only_what_backups_omit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        mark_init_restore(root).unwrap();
        fs::create_dir_all(root.join("notes/deep")).unwrap();
        fs::create_dir_all(root.join("cache")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("notes/deep/a"), b"a").unwrap();
        fs::write(root.join("cache/model"), b"m").unwrap();
        fs::write(root.join("config/journal.json"), b"{}").unwrap();
        abandon_init_restore(root).unwrap();
        assert!(!root.join("notes").exists());
        assert!(!root.join("config/journal.json").exists());
        assert_eq!(fs::read(root.join("cache/model")).unwrap(), b"m");
        assert!(!is_init_restore(root));
    }

    #[test]
    fn the_marker_says_whether_this_restore_may_sweep() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_init_restore(dir.path()));
        mark_init_restore(dir.path()).unwrap();
        assert!(is_init_restore(dir.path()));
        finish_init_restore(dir.path());
        assert!(!is_init_restore(dir.path()));
    }
}
