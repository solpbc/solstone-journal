// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

/// A scratch directory removed when the test ends, pass or fail.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        // Removes a symbolic link itself rather than following it.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "indexer-todo-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

#[test]
fn a_resolved_path_under_the_journal_keys_like_its_lexical_path() {
    let scratch = scratch("resolved");
    let journal = scratch.0.as_path();
    let target = solstone_core_journal_io::paths::contained_path(
        journal,
        "facets/work/entities/ada/observations.jsonl",
    )
    .unwrap();
    assert_eq!(
        path_key(journal, &target).unwrap(),
        "facets/work/entities/ada/observations.jsonl"
    );
    assert_eq!(
        path_key(
            journal,
            &journal.join("chronicle/20261021/talents/activity.md")
        )
        .unwrap(),
        "20261021/talents/activity.md"
    );
}

#[test]
fn a_path_outside_the_journal_is_still_refused() {
    let inside = scratch("inside");
    let outside = scratch("outside");
    let (journal, other) = (inside.0.as_path(), outside.0.as_path());
    let error = path_key(journal, &other.join("notes.md")).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[cfg(unix)]
#[test]
fn a_journal_reached_through_a_link_keys_its_resolved_paths() {
    let real = scratch("real");
    let link = Scratch(real.0.with_file_name(format!(
        "{}-link",
        real.0.file_name().unwrap().to_str().unwrap()
    )));
    let link = link.0.as_path();
    std::os::unix::fs::symlink(&real.0, link).unwrap();
    let target =
        solstone_core_journal_io::paths::contained_path(link, "facets/work/notes.md").unwrap();
    assert!(!target.starts_with(link));
    assert_eq!(path_key(link, &target).unwrap(), "facets/work/notes.md");
}

// The verbatim prefix only means something to Windows path parsing; on other
// hosts `\\?\C:\...` is one relative file name, so the link case above is the
// same comparison there.
#[cfg(windows)]
#[test]
fn verbatim_and_plain_spellings_of_one_journal_key_alike() {
    let scratch = scratch("verbatim");
    let journal = scratch.0.as_path();
    assert!(journal.is_absolute() && !journal.to_string_lossy().starts_with(r"\\"));
    let verbatim = PathBuf::from(format!(r"\\?\{}", journal.display()));
    assert!(matches!(
        verbatim.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::VerbatimDisk(_))
    ));
    let notes = r"facets\work\entities\ada\observations.jsonl";
    let activity = r"chronicle\20261021\talents\activity.md";
    for (root, path, key) in [
        (
            journal,
            verbatim.join(notes),
            "facets/work/entities/ada/observations.jsonl",
        ),
        (
            verbatim.as_path(),
            journal.join(notes),
            "facets/work/entities/ada/observations.jsonl",
        ),
        (
            journal,
            verbatim.join(activity),
            "20261021/talents/activity.md",
        ),
        (
            verbatim.as_path(),
            journal.join(activity),
            "20261021/talents/activity.md",
        ),
    ] {
        assert_eq!(path_key(root, &path).unwrap(), key, "{}", path.display());
    }
}
