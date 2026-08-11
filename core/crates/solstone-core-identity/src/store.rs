// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockOptions, append_text, atomic_replace, hold_lock,
};

use crate::IdentityError;
use crate::fixture::{HEALTH_MD, PARTNER_MD};
use crate::section::{prune_partner_getting_started, replace_section};

const LOCK_SENTINEL: &str = ".identity";
const FILE_MODE: u32 = 0o600;

/// Write one identity file under the identity sentinel lock.
pub fn write_identity(
    journal_path: &Path,
    file: &str,
    actor: &str,
    op: &str,
    section: Option<&str>,
    content: &str,
    reason: &str,
) -> Result<(), IdentityError> {
    write_identity_with_lock_options(
        journal_path,
        file,
        actor,
        op,
        section,
        content,
        reason,
        LockOptions::default(),
    )
}

/// Update one top-level identity section under the identity sentinel lock.
pub fn update_identity_section(
    journal_path: &Path,
    file: &str,
    section: &str,
    new_value: &str,
    actor: &str,
    reason: &str,
) -> Result<bool, IdentityError> {
    update_identity_section_with_lock_options(
        journal_path,
        file,
        section,
        new_value,
        actor,
        reason,
        LockOptions::default(),
    )
}

/// Create missing identity seed files and return the identity directory.
pub fn ensure_identity_directory(journal_path: &Path) -> Result<PathBuf, IdentityError> {
    let identity_dir = identity_dir(journal_path);
    for (file, content) in [("partner.md", PARTNER_MD), ("health.md", HEALTH_MD)] {
        if identity_dir.join(file).exists() {
            continue;
        }
        write_identity(
            journal_path,
            file,
            "ensure_identity_directory",
            "create",
            None,
            content,
            "bootstrap",
        )?;
    }
    Ok(identity_dir)
}

#[allow(clippy::too_many_arguments)] // Public parity arguments plus the crate-test lock seam.
pub(crate) fn write_identity_with_lock_options(
    journal_path: &Path,
    file: &str,
    actor: &str,
    op: &str,
    section: Option<&str>,
    content: &str,
    reason: &str,
    lock_options: LockOptions,
) -> Result<(), IdentityError> {
    let identity_dir = identity_dir(journal_path);
    let _lock = identity_lock(&identity_dir, lock_options)?;
    write_identity_locked(&identity_dir, file, actor, op, section, content, reason)
}

pub(crate) fn update_identity_section_with_lock_options(
    journal_path: &Path,
    file: &str,
    section: &str,
    new_value: &str,
    actor: &str,
    reason: &str,
    lock_options: LockOptions,
) -> Result<bool, IdentityError> {
    let identity_dir = identity_dir(journal_path);
    let file_name = identity_file_name(file);
    let target = identity_dir.join(&file_name);
    let _lock = identity_lock(&identity_dir, lock_options)?;
    if !target.exists() {
        return Ok(false);
    }
    let existing = read_existing(&target)?;
    let Some(mut new_content) = replace_section(&existing, section, new_value) else {
        return Ok(false);
    };
    if file_name == "partner.md" {
        new_content = prune_partner_getting_started(&new_content);
    }
    if new_content == existing {
        return Ok(false);
    }
    write_identity_locked(
        &identity_dir,
        &file_name,
        actor,
        "update_section",
        Some(section),
        &new_content,
        reason,
    )?;
    Ok(true)
}

fn write_identity_locked(
    identity_dir: &Path,
    file: &str,
    actor: &str,
    op: &str,
    section: Option<&str>,
    content: &str,
    reason: &str,
) -> Result<(), IdentityError> {
    let file_name = identity_file_name(file);
    let target = identity_dir.join(&file_name);
    let had_existing = target.exists();
    let before_content = if had_existing {
        read_existing(&target)?
    } else {
        String::new()
    };
    atomic_replace(
        &target,
        content.as_bytes(),
        AtomicWriteOptions {
            mode: Some(FILE_MODE),
        },
    )?;
    let record = history_record(
        &file_name,
        actor,
        op,
        section,
        reason,
        &before_content,
        content,
    );
    match append_text(history_path(identity_dir), &record) {
        Ok(()) => Ok(()),
        Err(error) => {
            rollback_after_append_failure(&target, had_existing, &before_content);
            Err(IdentityError::Append(error))
        }
    }
}

fn identity_dir(journal_path: &Path) -> PathBuf {
    journal_path.join("identity")
}

fn identity_file_name(file: &str) -> String {
    Path::new(file)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned()
}

fn history_path(identity_dir: &Path) -> PathBuf {
    identity_dir.join("history.jsonl")
}

fn identity_lock(
    identity_dir: &Path,
    options: LockOptions,
) -> Result<solstone_core_journal_io::FileLock, IdentityError> {
    Ok(hold_lock(identity_dir.join(LOCK_SENTINEL), options)?)
}

fn read_existing(target: &Path) -> Result<String, IdentityError> {
    fs::read_to_string(target).map_err(|source| IdentityError::Io {
        path: target.to_path_buf(),
        source,
    })
}

fn rollback_after_append_failure(target: &Path, had_existing: bool, before_content: &str) {
    if had_existing {
        if let Err(error) = atomic_replace(
            target,
            before_content.as_bytes(),
            AtomicWriteOptions {
                mode: Some(FILE_MODE),
            },
        ) {
            log::error!(
                "Failed to restore {} after history append failure: {error}",
                target.display()
            );
        }
        return;
    }
    if let Err(error) = fs::remove_file(target)
        && error.kind() != io::ErrorKind::NotFound
    {
        log::error!(
            "Failed to remove {} after history append failure: {error}",
            target.display()
        );
    }
}

fn history_record(
    file: &str,
    actor: &str,
    op: &str,
    section: Option<&str>,
    reason: &str,
    before_content: &str,
    content: &str,
) -> String {
    let section = section
        .map(json_string)
        .unwrap_or_else(|| "null".to_owned());
    format!(
        concat!(
            "{{\"ts\":{},\"file\":{},\"actor\":{},\"op\":{},\"section\":{},",
            "\"reason\":{},\"before_hash\":{},\"after_hash\":{},",
            "\"bytes_before\":{},\"bytes_after\":{}}}"
        ),
        json_string(&history_ts()),
        json_string(file),
        json_string(actor),
        json_string(op),
        section,
        json_string(reason),
        json_string(&hash_content(before_content)),
        json_string(&hash_content(content)),
        before_content.len(),
        content.len(),
    )
}

fn history_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn hash_content(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn json_string(value: &str) -> String {
    format!("\"{}\"", json_escape_ascii(value))
}

fn json_escape_ascii(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{0008}' => escaped.push_str("\\b"),
            '\u{000c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character <= '\u{001f}' => {
                let _ = write!(escaped, "\\u{:04x}", character as u32);
            }
            character if ('\u{007f}'..='\u{ffff}').contains(&character) => {
                let _ = write!(escaped, "\\u{:04x}", character as u32);
            }
            character if character > '\u{ffff}' => {
                let scalar = character as u32 - 0x1_0000;
                let high = 0xd800 + (scalar >> 10);
                let low = 0xdc00 + (scalar & 0x03ff);
                let _ = write!(escaped, "\\u{high:04x}\\u{low:04x}");
            }
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use solstone_core_journal_io::{LockError, LockOptions, hold_lock};

    use super::{
        FILE_MODE, ensure_identity_directory, hash_content, history_path, json_escape_ascii,
        update_identity_section, update_identity_section_with_lock_options, write_identity,
        write_identity_with_lock_options,
    };
    use crate::IdentityError;
    use crate::fixture::{HEALTH_MD, PARTNER_MD};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestJournal(PathBuf);

    impl TestJournal {
        fn new() -> Self {
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "solstone-core-identity-{}-{}-{sequence}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time after epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("journal directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn identity_dir(&self) -> PathBuf {
            self.path().join("identity")
        }
    }

    impl Drop for TestJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn short_lock_options() -> LockOptions {
        LockOptions {
            timeout: Duration::from_millis(50),
            ..LockOptions::default()
        }
    }

    fn history_lines(journal: &TestJournal) -> Vec<String> {
        fs::read_to_string(history_path(&journal.identity_dir()))
            .expect("history")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn assert_no_atomic_temps(identity_dir: &Path) {
        assert!(
            fs::read_dir(identity_dir)
                .expect("identity directory")
                .all(|entry| {
                    !entry
                        .expect("directory entry")
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".tmp_")
                })
        );
    }

    #[test]
    fn identity_writes_use_only_the_sentinel_sidecar_and_report_short_timeouts() {
        let journal = TestJournal::new();
        write_identity(
            journal.path(),
            "partner.md",
            "writer",
            "replace",
            None,
            "content\n",
            "test",
        )
        .expect("write");
        let lock_files = fs::read_dir(journal.identity_dir())
            .expect("identity directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().ends_with(".lock"))
            .collect::<Vec<_>>();
        assert_eq!(lock_files.len(), 1);
        assert_eq!(lock_files[0].to_string_lossy(), ".identity.lock");

        let protected = journal.identity_dir().join(".identity");
        let _holder = hold_lock(&protected, LockOptions::default()).expect("holder lock");
        let error = write_identity_with_lock_options(
            journal.path(),
            "health.md",
            "writer",
            "replace",
            None,
            "content\n",
            "test",
            short_lock_options(),
        )
        .expect_err("lock timeout");
        match error {
            IdentityError::Lock(LockError::Timeout(timeout)) => {
                assert_eq!(timeout.path, protected);
                assert_eq!(timeout.timeout, Duration::from_millis(50));
            }
            error => panic!("expected lock timeout, got {error:?}"),
        }
        let error = update_identity_section_with_lock_options(
            journal.path(),
            "partner.md",
            "missing",
            "value",
            "writer",
            "test",
            short_lock_options(),
        )
        .expect_err("lock timeout");
        assert!(matches!(error, IdentityError::Lock(LockError::Timeout(_))));
    }

    #[test]
    fn history_line_has_python_order_hashes_and_compact_separators() {
        let journal = TestJournal::new();
        write_identity(
            journal.path(),
            "partner.md",
            "journal identity partner --write",
            "replace",
            None,
            "after — content\n",
            "test",
        )
        .expect("write");
        let line = history_lines(&journal).pop().expect("history line");
        let keys = [
            "\"ts\":",
            "\"file\":",
            "\"actor\":",
            "\"op\":",
            "\"section\":",
            "\"reason\":",
            "\"before_hash\":",
            "\"after_hash\":",
            "\"bytes_before\":",
            "\"bytes_after\":",
        ];
        let mut offset = 0;
        for key in keys {
            let index = line[offset..].find(key).expect("ordered history key") + offset;
            offset = index + key.len();
        }
        assert!(line.starts_with("{\"ts\":\""));
        assert!(line.contains("\"file\":\"partner.md\""));
        assert!(line.contains("\"op\":\"replace\""));
        assert!(line.contains("\"section\":null"));
        assert!(line.contains(&format!("\"before_hash\":\"{}\"", hash_content(""))));
        assert!(line.contains(&format!(
            "\"after_hash\":\"{}\"",
            hash_content("after — content\n")
        )));
        assert!(line.contains("\"bytes_before\":0"));
        assert!(line.contains("\"bytes_after\":18"));
        assert!(!line.contains(", "));
        assert!(!line.contains(": "));
        assert!(line.ends_with('}'));
        let timestamp = &line[7..31];
        assert_eq!(timestamp.len(), 24);
        assert_eq!(&timestamp[4..5], "-");
        assert_eq!(&timestamp[10..11], "T");
        assert_eq!(&timestamp[19..20], ".");
        assert!(timestamp.ends_with('Z'));
        assert!(timestamp.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 23) || byte.is_ascii_digit()
        }));
    }

    #[test]
    fn actor_is_opaque_across_write_and_section_update() {
        let journal = TestJournal::new();
        write_identity(
            journal.path(),
            "partner.md",
            "journal identity partner --write",
            "replace",
            None,
            "## heading\nold\n",
            "test",
        )
        .expect("write");
        update_identity_section(
            journal.path(),
            "partner.md",
            "heading",
            "new",
            "journal identity partner --update-section <heading>",
            "test",
        )
        .expect("update");
        let lines = history_lines(&journal);
        assert!(lines[0].contains("\"actor\":\"journal identity partner --write\""));
        assert!(
            lines[1].contains("\"actor\":\"journal identity partner --update-section <heading>\"")
        );
    }

    #[test]
    fn append_failure_restores_existing_or_removes_new_target_without_temp_files() {
        let journal = TestJournal::new();
        let identity_dir = journal.identity_dir();
        fs::create_dir_all(&identity_dir).expect("identity directory");
        let target = identity_dir.join("partner.md");
        fs::write(&target, "original\n").expect("target");
        fs::create_dir_all(history_path(&identity_dir)).expect("history directory");

        let error = write_identity(
            journal.path(),
            "partner.md",
            "writer",
            "replace",
            None,
            "updated\n",
            "test",
        )
        .expect_err("append fails");
        assert!(matches!(error, IdentityError::Append(_)));
        assert_eq!(
            fs::read_to_string(&target).expect("restored target"),
            "original\n"
        );
        assert_no_atomic_temps(&identity_dir);

        let new_target = identity_dir.join("health.md");
        let error = write_identity(
            journal.path(),
            "health.md",
            "writer",
            "replace",
            None,
            "updated\n",
            "test",
        )
        .expect_err("append fails");
        assert!(matches!(error, IdentityError::Append(_)));
        assert!(!new_target.exists());
        assert_no_atomic_temps(&identity_dir);
    }

    #[test]
    fn consecutive_writes_append_two_single_newline_records() {
        let journal = TestJournal::new();
        for content in ["first\n", "second\n"] {
            write_identity(
                journal.path(),
                "partner.md",
                "writer",
                "replace",
                None,
                content,
                "test",
            )
            .expect("write");
        }
        let bytes = fs::read(history_path(&journal.identity_dir())).expect("history bytes");
        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 2);
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes.ends_with(b"\n\n"));
    }

    #[test]
    fn bootstrap_uses_byte_exact_fixtures_and_never_overwrites_owner_content() {
        let journal = TestJournal::new();
        let identity_dir = ensure_identity_directory(journal.path()).expect("bootstrap");
        assert_eq!(
            fs::read_to_string(identity_dir.join("partner.md")).unwrap(),
            PARTNER_MD
        );
        assert_eq!(
            fs::read_to_string(identity_dir.join("health.md")).unwrap(),
            HEALTH_MD
        );
        let lines = history_lines(&journal);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| {
            line.contains("\"actor\":\"ensure_identity_directory\"")
                && line.contains("\"op\":\"create\"")
        }));

        let journal = TestJournal::new();
        let identity_dir = journal.identity_dir();
        fs::create_dir_all(&identity_dir).expect("identity directory");
        fs::write(identity_dir.join("partner.md"), "owner content\n").expect("partner");
        ensure_identity_directory(journal.path()).expect("bootstrap");
        assert_eq!(
            fs::read_to_string(identity_dir.join("partner.md")).unwrap(),
            "owner content\n"
        );
        let lines = history_lines(&journal);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("\"file\":\"health.md\""));
    }

    #[cfg(unix)]
    #[test]
    fn identity_target_mode_is_private() {
        let journal = TestJournal::new();
        write_identity(
            journal.path(),
            "partner.md",
            "writer",
            "replace",
            None,
            "content\n",
            "test",
        )
        .expect("write");
        assert_eq!(
            fs::metadata(journal.identity_dir().join("partner.md"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            FILE_MODE
        );
    }

    #[test]
    fn absent_heading_and_equal_value_are_noop_updates_without_history() {
        let journal = TestJournal::new();
        let identity_dir = journal.identity_dir();
        fs::create_dir_all(&identity_dir).expect("identity directory");
        fs::write(identity_dir.join("other.md"), "## present\nvalue\n").expect("other");
        assert!(
            !update_identity_section(
                journal.path(),
                "other.md",
                "missing",
                "value",
                "writer",
                "test"
            )
            .expect("absent heading")
        );
        assert!(!history_path(&identity_dir).exists());
        assert!(
            !update_identity_section(
                journal.path(),
                "other.md",
                "present",
                "value",
                "writer",
                "test"
            )
            .expect("equal value")
        );
        assert!(!history_path(&identity_dir).exists());
    }

    #[test]
    fn partner_updates_prune_getting_started_but_other_writes_do_not() {
        let journal = TestJournal::new();
        let partner = "# partner\n\n## getting started\nintro\n\n## work patterns\nold\n";
        write_identity(
            journal.path(),
            "partner.md",
            "writer",
            "replace",
            None,
            partner,
            "test",
        )
        .expect("seed partner");
        assert!(
            update_identity_section(
                journal.path(),
                "partner.md",
                "work patterns",
                "new",
                "writer",
                "test"
            )
            .expect("partner update")
        );
        let updated = fs::read_to_string(journal.identity_dir().join("partner.md")).unwrap();
        assert!(!updated.contains("## getting started"));
        assert!(!updated.contains("intro"));

        let other = "## getting started\nkeep\n\n## work patterns\nold\n";
        write_identity(
            journal.path(),
            "other.md",
            "writer",
            "replace",
            None,
            other,
            "test",
        )
        .expect("seed other");
        update_identity_section(
            journal.path(),
            "other.md",
            "work patterns",
            "new",
            "writer",
            "test",
        )
        .expect("other update");
        assert!(
            fs::read_to_string(journal.identity_dir().join("other.md"))
                .unwrap()
                .contains("## getting started")
        );

        write_identity(
            journal.path(),
            "partner.md",
            "writer",
            "replace",
            None,
            partner,
            "test",
        )
        .expect("plain replace");
        assert!(
            fs::read_to_string(journal.identity_dir().join("partner.md"))
                .unwrap()
                .contains("## getting started")
        );
    }

    #[test]
    fn json_escape_ascii_matches_python_ensure_ascii_rules() {
        assert_eq!(json_escape_ascii("plain / ascii"), "plain / ascii");
        assert_eq!(
            json_escape_ascii("\"\\\u{0008}\u{000c}\n\r\t\u{0001}"),
            "\\\"\\\\\\b\\f\\n\\r\\t\\u0001"
        );
        assert_eq!(json_escape_ascii("—😀"), "\\u2014\\ud83d\\ude00");
    }
}
