// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Recovery records written by v2.0.19 recover on this build. The fixtures
//! are that build's own journal trees; see `fixtures/merge-recovery-v2019.provenance.md`.
#![cfg(unix)]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const RECOVERY: &str = "health/entity-merge-recovery";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/merge-recovery-v2019")
}

/// Every fixture path with its mode, directories included.
fn manifest() -> Vec<(u32, String)> {
    fs::read_to_string(fixtures().with_extension("modes"))
        .expect("read mode manifest")
        .lines()
        .map(|line| {
            let (mode, path) = line.split_once(' ').expect("mode and path");
            (
                u32::from_str_radix(mode, 8).expect("octal mode"),
                path.to_owned(),
            )
        })
        .collect()
}

/// A copied journal; its scratch directory goes when this does.
struct Copied {
    scratch: PathBuf,
    journal: PathBuf,
}

impl std::ops::Deref for Copied {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.journal
    }
}

impl Drop for Copied {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.scratch);
    }
}

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "entity-merge-recovery-v2019-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    fs::canonicalize(path).unwrap()
}

/// Copy one fixture tree as v2.0.19 left it: git keeps neither empty
/// directories nor modes, and recovery fingerprints both.
fn journal_from(name: &str, apply_modes: bool) -> Copied {
    let root = scratch();
    let prefix = format!("{name}/");
    let mut entries: Vec<(u32, String)> = manifest()
        .into_iter()
        .filter(|(_, path)| path.starts_with(&prefix))
        .collect();
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    for (_, path) in &entries {
        let from = fixtures().join(path);
        let to = root.join(path);
        if from.is_file() {
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(&from, &to).unwrap();
        } else {
            fs::create_dir_all(&to).unwrap();
        }
    }
    // Deepest first, so a directory's mode never blocks setting its children.
    for (mode, path) in entries.iter().rev() {
        let to = root.join(path);
        let mode = if apply_modes {
            *mode
        } else if to.is_dir() {
            0o755
        } else {
            0o644
        };
        fs::set_permissions(&to, fs::Permissions::from_mode(mode)).unwrap();
    }
    if apply_modes {
        for (mode, path) in &entries {
            let metadata = fs::symlink_metadata(root.join(path))
                .unwrap_or_else(|_| panic!("fixture path {path} is missing"));
            assert_eq!(metadata.permissions().mode() & 0o7777, *mode, "{path}");
        }
    }
    Copied {
        journal: root.join(name),
        scratch: root,
    }
}

/// File bytes and modes under a tree, without the locks a run takes and the
/// identity map cache, which is derived.
fn tree(root: &Path) -> BTreeMap<String, (Vec<u8>, u32)> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, (Vec<u8>, u32)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if relative.ends_with(".lock") || relative.ends_with(".identity-map-cache.json") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                walk(base, &path, out);
            } else {
                out.insert(
                    relative,
                    (
                        fs::read(&path).unwrap(),
                        metadata.permissions().mode() & 0o7777,
                    ),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn without_recovery(tree: BTreeMap<String, (Vec<u8>, u32)>) -> BTreeMap<String, (Vec<u8>, u32)> {
    tree.into_iter()
        .filter(|(path, _)| !path.starts_with(RECOVERY))
        .collect()
}

#[test]
fn an_interrupted_v2019_merge_rolls_back_to_the_journal_before_it() {
    let journal = journal_from("merge-interrupted", true);
    let merge_id = fs::read_to_string(journal.join("logs/entity-merges.jsonl"))
        .expect("the fixture keeps its merge log")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .next_back()
        .unwrap()["merge_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        journal
            .join(format!("entities/target/history/private/{merge_id}.json"))
            .is_file()
    );
    solstone_core_entity::recover_interrupted_entity_merge(&journal).unwrap();
    assert!(!journal.join(RECOVERY).exists());
    let before = journal_from("merge-before", true);
    assert_eq!(tree(&journal), tree(&before));
}

#[test]
fn a_v2019_record_whose_modes_were_lost_refuses() {
    let journal = journal_from("merge-interrupted", false);
    let error = solstone_core_entity::recover_interrupted_entity_merge(&journal).unwrap_err();
    assert!(error.contains("conflicts"), "{error}");
    assert!(journal.join(RECOVERY).exists());
}

#[test]
fn a_committed_v2019_undo_finishes_without_touching_the_journal() {
    let journal = journal_from("undo-committed", true);
    let before = without_recovery(tree(&journal));
    solstone_core_entity::recover_interrupted_entity_merge(&journal).unwrap();
    assert!(!journal.join(RECOVERY).exists());
    assert_eq!(tree(&journal), before);
}

#[test]
fn the_same_undo_record_uncommitted_rolls_back_to_before_the_undo() {
    let journal = journal_from("undo-committed", true);
    let count = fs::read_dir(journal.join(RECOVERY))
        .unwrap()
        .filter(|entry| {
            let name = entry.as_ref().unwrap().file_name();
            let name = name.to_string_lossy();
            name.len() == 13 && name[..8].bytes().all(|byte| byte.is_ascii_digit())
        })
        .count();
    let state = journal.join(RECOVERY).join("state.json");
    fs::write(
        &state,
        serde_json::to_vec(&serde_json::json!({"source_committed":false,"snapshot_count":count}))
            .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o600)).unwrap();
    solstone_core_entity::recover_interrupted_entity_merge(&journal).unwrap();
    assert!(!journal.join(RECOVERY).exists());
    let before = journal_from("undo-before", true);
    assert_eq!(tree(&journal), tree(&before));
}
