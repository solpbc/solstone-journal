// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The backup argv leaves the ChatGPT sign-in file and its moved-aside copies out.

use std::fs;
use std::path::Path;

use super::{backup_args, resolve_backup_journal};

fn components(path: &str) -> Vec<String> {
    Path::new(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

/// One path component against one pattern component, where `*` spans any run of characters.
fn component_matches(pattern: &str, value: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = value.strip_prefix(first) else {
        return false;
    };
    let tail: Vec<&str> = parts.collect();
    let Some((last, middle)) = tail.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(index) => rest = &rest[index + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// Restic's exclude rule: an absolute pattern matches the path or one of its parents; a
/// relative pattern matches a run of components anywhere in the path.
fn excluded_by(pattern: &str, path: &str) -> bool {
    let absolute = Path::new(pattern).is_absolute();
    let pattern = components(pattern);
    let path = components(path);
    let window = |start: usize| {
        path.len() >= start + pattern.len()
            && pattern
                .iter()
                .zip(&path[start..])
                .all(|(pattern, value)| component_matches(pattern, value))
    };
    if absolute {
        window(0)
    } else {
        (0..path.len()).any(window)
    }
}

#[test]
fn backup_argv_excludes_the_chatgpt_sign_in_file_and_its_moved_aside_copies() {
    // The default `.tmp` prefix would itself match the `.tmp*` exclude.
    let dir = tempfile::Builder::new()
        .prefix("journal-")
        .tempdir()
        .expect("tempdir");
    let config = dir.path().join("config");
    fs::create_dir_all(&config).expect("config dir");
    let excluded = [
        "chatgpt-sign-in.json",
        "chatgpt-sign-in.json.lock",
        "chatgpt-sign-in.json.unreadable-1700000000",
        "chatgpt-sign-in.json.unreadable-1700000500",
    ];
    let kept = ["journal.json", "chatgpt-notes.json"];
    for name in excluded.iter().chain(&kept) {
        fs::write(config.join(name), b"{}").expect("file writes");
    }

    let resolved = resolve_backup_journal(dir.path()).expect("journal resolves");
    let args = backup_args(&resolved);
    let patterns: Vec<&str> = args
        .windows(2)
        .filter(|pair| pair[0] == "--exclude")
        .map(|pair| pair[1].as_str())
        .collect();
    assert_eq!(args[1], crate::restic_filesystem_path(&resolved));

    let resolved_config = resolved.join("config");
    for name in excluded {
        let path = crate::restic_filesystem_path(&resolved_config.join(name));
        assert!(
            patterns.iter().any(|pattern| excluded_by(pattern, &path)),
            "{name} is backed up: {patterns:?}"
        );
    }
    for name in kept {
        let path = crate::restic_filesystem_path(&resolved_config.join(name));
        assert!(
            !patterns.iter().any(|pattern| excluded_by(pattern, &path)),
            "{name} is left out: {patterns:?}"
        );
    }
}
