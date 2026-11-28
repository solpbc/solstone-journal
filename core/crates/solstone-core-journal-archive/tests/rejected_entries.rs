// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

mod common;

use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use common::{TempDir, directory, journal, write};
use solstone_core_journal_archive::{ArchiveError, ArchiveSource, JournalEntryKind};

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn unreadable_included_directory_returns_source_io() {
    use nix::unistd::Uid;

    if Uid::effective().is_root() {
        eprintln!("skipping permission-denied assertion as effective root");
        return;
    }

    let temporary = TempDir::new("permission-denied");
    let root = common::valid_four_root_journal(&temporary);
    let restricted = root.join("chronicle/20260101");
    fs::set_permissions(&restricted, fs::Permissions::from_mode(0o000))
        .expect("remove directory permissions");
    let result = ArchiveSource::open(&root);
    fs::set_permissions(&restricted, fs::Permissions::from_mode(0o755))
        .expect("restore directory permissions before temporary cleanup");

    assert!(matches!(result, Err(ArchiveError::SourceIo { .. })));
}

#[test]
#[allow(clippy::disallowed_methods)]
fn rejects_relative_missing_and_file_roots() {
    let temporary = TempDir::new("invalid-roots");
    let file = temporary.path().join("file");
    let missing = temporary.path().join("missing");
    fs::write(&file, b"not a journal").expect("write file root");

    for root in [Path::new("relative"), missing.as_path(), file.as_path()] {
        assert!(matches!(
            ArchiveSource::open(root),
            Err(ArchiveError::InvalidJournal { .. })
        ));
    }
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn rejects_dangling_file_and_loop_symlink_roots() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new("invalid-symlink-roots");
    let missing = temporary.path().join("missing");
    let dangling = temporary.path().join("dangling");
    let file = temporary.path().join("file");
    let file_link = temporary.path().join("file-link");
    let loop_left = temporary.path().join("loop-left");
    let loop_right = temporary.path().join("loop-right");
    fs::write(&file, b"not a directory").expect("write file root");
    symlink(&missing, &dangling).expect("create dangling root symlink");
    symlink(&file, &file_link).expect("create file root symlink");
    symlink(&loop_right, &loop_left).expect("create first loop symlink");
    symlink(&loop_left, &loop_right).expect("create second loop symlink");

    for root in [&dangling, &file_link, &loop_left] {
        assert!(matches!(
            ArchiveSource::open(root),
            Err(ArchiveError::InvalidJournal { .. })
        ));
    }
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn skips_symlinks_without_following_and_rejects_fifo_and_socket() {
    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;

    let temporary = TempDir::new("special-members");
    let root = journal(&temporary);
    directory(&root, "chronicle/20260101");
    write(&root, "chronicle/20260101/note.md", b"kept");
    let outside = temporary.path().join("outside");
    fs::write(&outside, b"outside").expect("write outside marker");
    symlink(&outside, root.join("chronicle/20260101/link")).expect("create symlink");
    let source = ArchiveSource::open(&root).expect("symlink is skipped instead of refusing export");
    let members: Vec<_> = source
        .inventory()
        .entries()
        .iter()
        .map(|entry| entry.member_name().as_str())
        .collect();
    assert_eq!(members, vec!["chronicle/20260101/note.md"]);
    let skipped: Vec<_> = source
        .inventory()
        .skipped_symlinks()
        .iter()
        .map(|name| name.as_str())
        .collect();
    assert_eq!(skipped, vec!["chronicle/20260101/link"]);
    fs::remove_file(root.join("chronicle/20260101/link")).expect("remove symlink");

    mkfifo(
        &root.join("chronicle/20260101/pipe"),
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .expect("create fifo");
    let root_for_thread = root.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        sender
            .send(ArchiveSource::open(&root_for_thread))
            .expect("send inventory result");
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("FIFO classification must not block");
    assert!(matches!(
        result,
        Err(ArchiveError::UnsafeJournalEntry {
            kind: JournalEntryKind::Fifo,
            ..
        })
    ));
    fs::remove_file(root.join("chronicle/20260101/pipe")).expect("remove fifo");

    let _socket = UnixListener::bind(root.join("chronicle/20260101/socket")).expect("bind socket");
    assert!(matches!(
        ArchiveSource::open(&root),
        Err(ArchiveError::UnsafeJournalEntry {
            kind: JournalEntryKind::Socket,
            ..
        })
    ));
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn reports_unexcluded_skill_sibling_symlink_without_following() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new("unexcluded-skill-symlink");
    let root = journal(&temporary);
    directory(&root, ".agents/skills");
    let outside = temporary.path().join("outside");
    fs::write(&outside, b"outside").expect("write outside marker");
    symlink(&outside, root.join(".agents/skills/other")).expect("create symlink");

    let source = ArchiveSource::open(&root).expect("unexcluded symlink is skipped and reported");
    assert!(source.inventory().entries().is_empty());
    let skipped: Vec<_> = source
        .inventory()
        .skipped_symlinks()
        .iter()
        .map(|name| name.as_str())
        .collect();
    assert_eq!(skipped, vec![".agents/skills/other"]);
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn legacy_symlinks_are_silently_omitted_while_regular_files_and_post_swap_stay_strict() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new("legacy-symlinks");
    let root = journal(&temporary);
    let outside_dir = temporary.path().join("outside-dir");
    fs::create_dir(&outside_dir).expect("create outside dir");
    let outside_file = outside_dir.join("secret.txt");
    fs::write(&outside_file, b"outside-secret").expect("write outside file");
    let missing_target = temporary.path().join("does-not-exist");

    write(&root, "config/journal.json", b"{}");
    write(&root, "chronicle/20260101/kept.md", b"kept");
    // Regular owner file at a path that is only omitted when it is a legacy symlink.
    write(&root, "health/retention.log", b"owner-regular-log");

    directory(&root, ".agents/skills");
    directory(&root, ".claude/skills");
    directory(&root, "health");
    directory(&root, "chronicle/20260101/health");
    directory(&root, "talents");
    directory(&root, "agents");
    directory(&root, "apps/chat/bookmarks");

    // Legacy Solstone symlinks (all silently omitted).
    for legacy in [
        "CLAUDE.md",
        "GEMINI.md",
        ".agents/skills/sol",
        ".claude/skills/sol",
        "health/cron.log",
        "chronicle/20260101/health/sense.log",
        "talents/daily.log",
        "agents/helper.log",
        "apps/chat/bookmarks/pin-1",
    ] {
        symlink(&outside_file, root.join(legacy)).expect(legacy);
    }

    // Non-legacy symlinks: file, directory, and dangling (all skipped without following and reported).
    symlink(&outside_dir, root.join("linked-dir")).expect("dir symlink");
    symlink(&missing_target, root.join("dangling.txt")).expect("dangling symlink");

    let source = ArchiveSource::open(&root).expect("open journal with legacy and custom symlinks");
    let members: Vec<_> = source
        .inventory()
        .entries()
        .iter()
        .map(|entry| entry.member_name().as_str())
        .collect();
    assert_eq!(
        members,
        vec![
            "chronicle/20260101/kept.md",
            "config/journal.json",
            "health/retention.log",
        ]
    );
    let skipped: Vec<_> = source
        .inventory()
        .skipped_symlinks()
        .iter()
        .map(|name| name.as_str())
        .collect();
    assert_eq!(skipped, vec!["dangling.txt", "linked-dir"]);

    // Post-inventory swap of an inventoried regular file to a symlink still fails closed.
    fs::remove_file(root.join("chronicle/20260101/kept.md")).expect("remove regular file");
    symlink(&outside_file, root.join("chronicle/20260101/kept.md")).expect("swap with symlink");
    let kept_entry = source
        .inventory()
        .entries()
        .iter()
        .find(|entry| entry.member_name().as_str() == "chronicle/20260101/kept.md")
        .expect("kept entry");
    assert!(matches!(
        source.open_file(kept_entry),
        Err(ArchiveError::SourceChanged { .. })
    ));
    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn regular_claude_and_gemini_files_stay_exported_and_repo_fixture_opens_cleanly() {
    let temporary = TempDir::new("regular-agent-docs");
    let root = journal(&temporary);
    write(&root, "CLAUDE.md", b"owner notes");
    write(&root, "GEMINI.md", b"owner notes 2");
    let source = ArchiveSource::open(&root).expect("open regular CLAUDE.md and GEMINI.md");
    let members: Vec<_> = source
        .inventory()
        .entries()
        .iter()
        .map(|entry| entry.member_name().as_str())
        .collect();
    assert_eq!(members, vec!["CLAUDE.md", "GEMINI.md"]);
    assert!(source.inventory().skipped_symlinks().is_empty());

    // Repo fixture carries legacy CLAUDE.md/GEMINI.md symlinks and must open cleanly without edits.
    let fixture_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/journal");
    if fixture_root.is_dir() {
        let fixture_source =
            ArchiveSource::open(&fixture_root).expect("repo fixture journal opens cleanly");
        assert!(
            fixture_source.inventory().skipped_symlinks().is_empty(),
            "unexpected skipped symlinks in fixture: {:?}",
            fixture_source.inventory().skipped_symlinks()
        );
    }
}

#[cfg(unix)]
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "APFS rejects non-UTF-8 path components before the product boundary"
)]
#[allow(clippy::disallowed_methods)]
fn rejects_distinct_non_utf8_member_names_without_lossy_normalization() {
    let temporary = TempDir::new("non-utf8-members");
    let root = journal(&temporary);
    directory(&root, "imports");
    for bytes in [vec![b'a', 0xff], vec![b'a', 0xfe]] {
        let name = std::ffi::OsString::from_vec(bytes);
        fs::write(root.join("imports").join(name), b"bad").expect("write invalid name");
        assert!(matches!(
            ArchiveSource::open(&root),
            Err(ArchiveError::UnsafeJournalEntry {
                kind: JournalEntryKind::Other,
                ..
            })
        ));
        fs::remove_dir_all(root.join("imports")).expect("clear invalid entry");
        fs::create_dir(root.join("imports")).expect("restore imports root");
    }
}

#[cfg(unix)]
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "APFS rejects non-UTF-8 path components before the product boundary"
)]
#[allow(clippy::disallowed_methods)]
fn rejects_distinct_non_utf8_root_names_with_invalid_member_placeholder() {
    let temporary = TempDir::new("non-utf8-root-names");
    let root = journal(&temporary);
    for bytes in [vec![b'a', 0xff], vec![b'a', 0xfe]] {
        let name = std::ffi::OsString::from_vec(bytes);
        fs::write(root.join(&name), b"bad").expect("write invalid root name");
        assert!(matches!(
            ArchiveSource::open(&root),
            Err(ArchiveError::UnsafeJournalEntry {
                member,
                kind: JournalEntryKind::Other,
            }) if member.as_str() == "<invalid>"
        ));
        fs::remove_file(root.join(&name)).expect("remove invalid root name");
    }
}

#[cfg(unix)]
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "APFS rejects non-UTF-8 path components before the product boundary"
)]
#[allow(clippy::disallowed_methods)]
fn rejects_non_utf8_canonical_root_ancestor() {
    let temporary = TempDir::new("non-utf8-ancestor");
    let ancestor = temporary
        .path()
        .join(std::ffi::OsString::from_vec(vec![0xff]));
    fs::create_dir(&ancestor).expect("create non-utf8 ancestor");
    let root = ancestor.join("journal");
    fs::create_dir(&root).expect("create journal");
    write(&root, "imports/file", b"x");
    assert!(matches!(
        ArchiveSource::open(&root),
        Err(ArchiveError::InvalidJournal { .. })
    ));
}

#[test]
#[allow(clippy::disallowed_methods)]
fn operational_socket_is_omitted_while_owner_socket_named_file_is_kept() {
    use std::os::unix::net::UnixListener;
    let temporary = TempDir::new("operational-socket");
    let root = journal(&temporary);
    directory(&root, "health");
    let _socket = UnixListener::bind(root.join("health/callosum.sock")).unwrap();
    write(&root, "entities/cache/entity.json", b"{}");
    write(
        &root,
        "chronicle/20260101/source/120000_60/owner.sock",
        b"owner",
    );
    let source = ArchiveSource::open(&root).expect("operational socket does not block export");
    let names: Vec<_> = source
        .inventory()
        .entries()
        .iter()
        .map(|entry| entry.member_name().as_str())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"chronicle/20260101/source/120000_60/owner.sock"));
    assert!(names.contains(&"entities/cache/entity.json"));
    assert!(!names.contains(&"health/callosum.sock"));
}
