// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a whole-journal restore sets aside so that it never silently destroys
//! the journal it replaces, and so that afterwards the journal is the snapshot.
//!
//! The restore itself stays one restic restore over the journal. Around it,
//! restic's own dry-run report names what it would change and what the snapshot
//! doesn't hold:
//!
//! - **before**, each file the restore will change is copied into the
//!   set-aside folder;
//! - **after** a clean restore and check, each file the snapshot doesn't hold
//!   is moved there.
//!
//! What backups don't carry a restore leaves alone. Every step can run again.
//! A copy never replaces another: a file already set aside with the same bytes
//! is skipped, and different bytes are kept beside it under a numbered name.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal::BEFORE_RESTORE_PREFIX;
use solstone_core_journal_io::atomic::{AtomicWriteOptions, atomic_replace};
use solstone_core_journal_io::copy_excludes::{copy_excluded, copy_file_excluded};

/// Names the current attempt's folder across a retry.
pub(crate) const ATTEMPT_FILE: &str = solstone_core_journal::RESTORE_ATTEMPT_FILE;

/// Room kept free beyond the copies and the restored bytes.
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

/// One attempt at a restore, and the folder it sets aside into.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Attempt {
    pub stamp: String,
    pub snapshot_id: String,
}

impl Attempt {
    pub(crate) fn folder(&self) -> String {
        format!("{BEFORE_RESTORE_PREFIX}{}", self.stamp)
    }
}

/// The attempt in progress for this snapshot, or a new one.
pub(crate) fn begin_attempt(journal: &Path, snapshot_id: &str, stamp: &str) -> io::Result<Attempt> {
    let path = journal.join(ATTEMPT_FILE);
    if let Ok(bytes) = fs::read(&path)
        && let Ok(attempt) = serde_json::from_slice::<Attempt>(&bytes)
        && attempt.snapshot_id == snapshot_id
    {
        return Ok(attempt);
    }
    let attempt = Attempt {
        stamp: stamp.to_owned(),
        snapshot_id: snapshot_id.to_owned(),
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(&attempt).map_err(io::Error::other)?;
    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(io::Error::other)?;
    Ok(attempt)
}

/// A finished restore's next attempt starts a new folder.
pub(crate) fn finish_attempt(journal: &Path) {
    if let Err(error) = fs::remove_file(journal.join(ATTEMPT_FILE))
        && error.kind() != io::ErrorKind::NotFound
    {
        log::warn!("restore: the attempt marker was not cleared: {error}");
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
    let first = rel.split('/').next().unwrap_or(rel);
    let first = std::ffi::OsStr::new(first);
    solstone_core_journal::is_hidden_root_name(first)
        && !solstone_core_journal::is_trash_root_name(first)
        || copy_excluded(rel)
        || copy_file_excluded(rel)
}

/// Whether the sweep after a restore leaves this path where it is.
pub(crate) fn kept(rel: &str) -> bool {
    not_backed_up(rel)
        || LIVE_FILES.contains(&rel)
        || LIVE_PREFIXES.iter().any(|prefix| rel.starts_with(prefix))
}

/// The files a restore will change that exist now: what is `updated`, and a
/// file or link where the snapshot holds another kind of thing.
pub(crate) fn files_to_copy(journal: &Path, report: &Report) -> Vec<String> {
    let mut files: Vec<String> = report
        .updated
        .iter()
        .chain(report.restored.iter())
        .filter(|rel| !not_backed_up(rel))
        .filter(|rel| {
            fs::symlink_metadata(journal.join(rel)).is_ok_and(|meta| !meta.file_type().is_dir())
        })
        .cloned()
        .collect();
    files.sort();
    files.dedup();
    files
}

/// The bytes those copies take.
pub(crate) fn copies_bytes(journal: &Path, files: &[String]) -> u64 {
    files
        .iter()
        .filter_map(|rel| fs::symlink_metadata(journal.join(rel)).ok())
        .map(|meta| meta.len())
        .fold(0, u64::saturating_add)
}

/// Whether the copies and the restored bytes fit, with the margin. Free space
/// that can't be read doesn't refuse: a restore today doesn't ask.
pub(crate) fn fits(free: Option<u64>, copies: u64, restoring: u64) -> bool {
    free.is_none_or(|free| {
        free >= copies
            .saturating_add(restoring)
            .saturating_add(FREE_SPACE_MARGIN)
    })
}

/// Copy each file into the folder before the restore overwrites it.
pub(crate) fn copy_originals(journal: &Path, folder: &str, files: &[String]) -> io::Result<()> {
    for rel in files {
        copy_one(journal, folder, rel)?;
    }
    Ok(())
}

fn copy_one(journal: &Path, folder: &str, rel: &str) -> io::Result<()> {
    let source = journal.join(rel);
    let Ok(meta) = fs::symlink_metadata(&source) else {
        return Ok(());
    };
    // A pipe or socket holds nothing to keep, and opening a pipe would wait.
    if !meta.is_file() && !meta.file_type().is_symlink() {
        return Ok(());
    }
    let destination = journal.join(folder).join(rel);
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("no parent"))?
        .to_path_buf();
    fs::create_dir_all(&parent)?;
    let Some(free_name) = free_name_for(&source, &destination, &meta)? else {
        return Ok(());
    };
    let partial = with_suffix(&free_name, ".partial");
    if fs::symlink_metadata(&partial).is_ok() {
        fs::remove_file(&partial)?;
    }
    if meta.file_type().is_symlink() {
        copy_link(&source, &partial)?;
    } else {
        fs::copy(&source, &partial)?;
        fs::File::open(&partial)?.sync_all()?;
    }
    fs::rename(&partial, &free_name)?;
    flush_dir(&parent);
    Ok(())
}

/// Where to put a copy of `source`: `destination` if free, nothing if a copy
/// with the same bytes is already there, otherwise the next numbered name.
fn free_name_for(
    source: &Path,
    destination: &Path,
    meta: &fs::Metadata,
) -> io::Result<Option<PathBuf>> {
    let mut candidate = destination.to_path_buf();
    let mut number = 1_u32;
    loop {
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Some(candidate)),
            Err(error) => return Err(error),
            Ok(existing) => {
                if same_content(source, &candidate, meta, &existing)? {
                    return Ok(None);
                }
            }
        }
        number = number.saturating_add(1);
        candidate = with_suffix(destination, &format!("~{number}"));
    }
}

fn same_content(
    source: &Path,
    copy: &Path,
    meta: &fs::Metadata,
    existing: &fs::Metadata,
) -> io::Result<bool> {
    if meta.file_type().is_symlink() || existing.file_type().is_symlink() {
        return Ok(meta.file_type().is_symlink()
            && existing.file_type().is_symlink()
            && fs::read_link(source)? == fs::read_link(copy)?);
    }
    if !existing.is_file() || meta.len() != existing.len() {
        return Ok(false);
    }
    use std::io::Read;
    let mut left = io::BufReader::new(fs::File::open(source)?);
    let mut right = io::BufReader::new(fs::File::open(copy)?);
    let (mut a, mut b) = ([0_u8; 65536], [0_u8; 65536]);
    loop {
        let read = left.read(&mut a)?;
        if read == 0 {
            return Ok(true);
        }
        right.read_exact(&mut b[..read])?;
        if a[..read] != b[..read] {
            return Ok(false);
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(unix)]
fn copy_link(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)
}

#[cfg(windows)]
fn copy_link(source: &Path, destination: &Path) -> io::Result<()> {
    let target = fs::read_link(source)?;
    if fs::metadata(source).is_ok_and(|meta| meta.is_dir()) {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

#[cfg(not(any(unix, windows)))]
fn copy_link(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

fn flush_dir(path: &Path) {
    #[cfg(not(windows))]
    if let Err(error) = fs::File::open(path).and_then(|dir| dir.sync_all()) {
        log::warn!("restore: a set-aside folder was not flushed: {error}");
    }
    #[cfg(windows)]
    let _ = path;
}

/// Move what the snapshot doesn't hold into the folder, then remove the
/// directories that leaves empty. Returns how many files stayed behind.
pub(crate) fn move_removed(journal: &Path, folder: &str, report: &Report) -> usize {
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
        if let Err(error) = move_one(journal, folder, rel, &path, &meta) {
            log::warn!("restore: a file could not be set aside: {error}");
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

fn move_one(
    journal: &Path,
    folder: &str,
    rel: &str,
    path: &Path,
    meta: &fs::Metadata,
) -> io::Result<()> {
    let destination = journal.join(folder).join(rel);
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("no parent"))?
        .to_path_buf();
    fs::create_dir_all(&parent)?;
    let Some(free_name) = free_name_for(path, &destination, meta)? else {
        // Already set aside with these bytes: only the live copy is left.
        return fs::remove_file(path);
    };
    fs::rename(path, &free_name)?;
    flush_dir(&parent);
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => Err(io::Error::other("still in place after the move")),
    }
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

/// The most the set-aside folder name can say about when it was made.
pub(crate) fn stamp(now_unix: i64) -> String {
    chrono::DateTime::from_timestamp(now_unix, 0)
        .map(|at| at.format("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_else(|| now_unix.to_string())
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

    const FOLDER: &str = ".before-restore-1";

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
        // A removal outside the target is no report at all.
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
    fn a_log_a_restore_overwrites_is_still_copied_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("backup")).unwrap();
        fs::write(root.join("backup/teardown-recovery.json"), b"local").unwrap();
        let plan = Report {
            updated: vec!["backup/teardown-recovery.json".into()],
            ..Report::default()
        };
        assert_eq!(
            files_to_copy(root, &plan),
            ["backup/teardown-recovery.json"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_is_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        nix::unistd::mkfifo(&root.join("pipe"), nix::sys::stat::Mode::S_IRWXU).unwrap();
        copy_originals(root, FOLDER, &["pipe".to_owned()]).unwrap();
        assert!(!root.join(FOLDER).join("pipe").exists());
    }

    #[test]
    fn a_name_restic_cannot_print_is_counted_and_blocks_the_success_claim() {
        let text = report(&[
            "{\"message_type\":\"verbose_status\",\"action\":\"updated\",\"item\":\"/bad\u{fffd}name\"}",
        ]);
        let parsed = parse_report(&text, Path::new("/j")).unwrap();
        assert!(parsed.updated.is_empty());
        assert_eq!(parsed.unreadable_names, 1);
        assert!(!matches_snapshot(Path::new("/j"), &parsed));
    }

    #[test]
    fn what_backups_do_not_carry_stays_and_owner_paths_named_alike_do_not() {
        for path in [
            "cache/models/x",
            "indexer/journal.sqlite",
            "config/chatgpt-sign-in.json",
            ".before-restore-9/a",
            "health/service.log",
            "health/providers/local.json",
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
    fn a_change_and_a_type_change_are_copied_and_a_directory_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("changed"), b"now").unwrap();
        fs::write(root.join("becomes-link"), b"file").unwrap();
        let plan = Report {
            updated: vec!["changed".into(), "gone".into()],
            restored: vec!["becomes-link".into(), "sub".into(), "new".into()],
            ..Report::default()
        };
        assert_eq!(files_to_copy(root, &plan), ["becomes-link", "changed"]);
    }

    #[test]
    fn a_copy_is_whole_and_a_rerun_never_replaces_an_original() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a"), b"original").unwrap();
        copy_originals(root, FOLDER, &["a".to_owned()]).unwrap();
        assert_eq!(fs::read(root.join(FOLDER).join("a")).unwrap(), b"original");
        // A rerun with the same bytes changes nothing.
        copy_originals(root, FOLDER, &["a".to_owned()]).unwrap();
        assert!(!root.join(FOLDER).join("a~2").exists());
        // The journal moved on between attempts: both versions survive.
        fs::write(root.join("a"), b"newer").unwrap();
        copy_originals(root, FOLDER, &["a".to_owned()]).unwrap();
        assert_eq!(fs::read(root.join(FOLDER).join("a")).unwrap(), b"original");
        assert_eq!(fs::read(root.join(FOLDER).join("a~2")).unwrap(), b"newer");
        // A stop mid-copy leaves a partial that the next run replaces.
        fs::write(root.join(FOLDER).join("b.partial"), b"torn").unwrap();
        fs::write(root.join("b"), b"whole").unwrap();
        copy_originals(root, FOLDER, &["b".to_owned()]).unwrap();
        assert_eq!(fs::read(root.join(FOLDER).join("b")).unwrap(), b"whole");
        assert!(!root.join(FOLDER).join("b.partial").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_set_aside_as_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::os::unix::fs::symlink("target", root.join("link")).unwrap();
        copy_originals(root, FOLDER, &["link".to_owned()]).unwrap();
        assert_eq!(
            fs::read_link(root.join(FOLDER).join("link")).unwrap(),
            Path::new("target")
        );
    }

    #[test]
    fn what_the_snapshot_lacks_moves_and_emptied_folders_go() {
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
        assert_eq!(move_removed(root, FOLDER, &plan), 0);
        assert_eq!(
            fs::read(root.join(FOLDER).join("sub/deep/new")).unwrap(),
            b"new"
        );
        assert!(!root.join("sub").exists());
        assert_eq!(fs::read(root.join("cache/model")).unwrap(), b"kept");
    }

    #[cfg(windows)]
    #[test]
    fn a_file_held_open_stays_and_is_counted() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("held"), b"open").unwrap();
        let _open = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.join("held"))
            .unwrap();
        let plan = Report {
            deleted: vec!["held".into()],
            ..Report::default()
        };
        assert_eq!(move_removed(root, FOLDER, &plan), 1);
        assert!(root.join("held").exists());
    }

    #[test]
    fn an_attempt_lasts_for_its_snapshot_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let first = begin_attempt(root, "aaa", "1").unwrap();
        assert_eq!(begin_attempt(root, "aaa", "2").unwrap(), first);
        assert_eq!(begin_attempt(root, "bbb", "3").unwrap().stamp, "3");
        finish_attempt(root);
        assert_eq!(begin_attempt(root, "bbb", "4").unwrap().stamp, "4");
    }

    #[test]
    fn free_space_covers_the_copies_and_the_restore() {
        assert!(fits(None, 10, 10));
        assert!(fits(Some(FREE_SPACE_MARGIN + 20), 10, 10));
        assert!(!fits(Some(FREE_SPACE_MARGIN + 19), 10, 10));
    }

    #[test]
    fn a_change_left_after_the_restore_is_not_a_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(root.join("left"), b"x").unwrap();
        let kept_change = Report {
            updated: vec!["health/service.log".into()],
            restored: vec!["notes".into()],
            ..Report::default()
        };
        assert!(matches_snapshot(root, &kept_change));
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
}
