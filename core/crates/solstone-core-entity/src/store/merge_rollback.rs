// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Before-images for the single entity merge/undo admitted by entity-trust.
//! Recovery never overwrites an artifact changed since its last checkpoint.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    DetailedAtomicOutcome, DirEntry, DirEntryKind, FileLock, JournalSnapshot, LockOptions,
    SnapshotError, atomic_replace_detailed, capture_snapshot, contained_path,
    create_directory_with_mode, hold_lock, list_dir_entries, path_lexists, realpath_non_strict,
    remove_dir_all, remove_file, resolve_journal_path, restore_snapshot,
};

use super::merge_payload::{snapshot_from_payload, snapshot_payload};

const RECOVERY: &str = "health/entity-merge-recovery";

#[derive(Debug, Default)]
pub(crate) struct MergeRollback {
    snapshots: Vec<JournalSnapshot>,
    locks: BTreeMap<PathBuf, FileLock>,
}

impl MergeRollback {
    pub(super) fn begin(journal: &Path) -> Result<Self, SnapshotError> {
        let root = recovery_root(journal)?;
        create_directory_with_mode(&root, 0o700).map_err(SnapshotError::Path)?;
        write_record(
            journal,
            "state.json",
            &json!({"source_committed":false,"snapshot_count":0}),
        )?;
        #[cfg(unix)]
        solstone_core_journal_io::sync_dir(journal, "health").map_err(SnapshotError::Path)?;
        Ok(Self::default())
    }

    /// Keep the existing artifact owner's lock until commit or rollback.
    pub(super) fn lock_file(&mut self, path: &Path) -> Result<(), SnapshotError> {
        if !self.locks.contains_key(path) {
            let lock =
                hold_lock(path, LockOptions::default()).map_err(|error| failure(path, error))?;
            self.locks.insert(path.to_owned(), lock);
        }
        Ok(())
    }

    pub(super) fn capture(&mut self, journal: &Path, path: &str) -> Result<(), SnapshotError> {
        let snapshot = capture_snapshot(journal, path)?;
        write_record(
            journal,
            &format!("{:08}.json", self.snapshots.len()),
            &snapshot_payload(&snapshot),
        )?;
        write_record(
            journal,
            "state.json",
            &json!({"source_committed":false,"snapshot_count":self.snapshots.len()+1}),
        )?;
        self.snapshots.push(snapshot);
        Ok(())
    }

    /// A crash before this checkpoint can require explicit conflict resolution;
    /// the before-images are already durable and must not be guessed away.
    pub(super) fn checkpoint(&self, journal: &Path) -> Result<(), SnapshotError> {
        let expected = self
            .snapshots
            .iter()
            .map(|snapshot| {
                capture_snapshot(journal, snapshot_path(snapshot))
                    .map(|current| fingerprint(&current))
            })
            .collect::<Result<Vec<_>, _>>()?;
        write_record(journal, "expected.json", &json!(expected))
    }

    pub(super) fn commit_source(
        &self,
        journal: &Path,
        operation: &str,
        report: &Value,
    ) -> Result<(), SnapshotError> {
        write_record(
            journal,
            "state.json",
            &json!({"source_committed":true,"operation":operation,"report":report}),
        )
    }

    pub(super) fn finish(&self, journal: &Path) -> Result<(), SnapshotError> {
        finish(journal)
    }

    pub(super) fn restore(&self, journal: &Path) -> Result<(), SnapshotError> {
        for snapshot in self.snapshots.iter().rev() {
            restore_snapshot(journal, snapshot)?;
        }
        finish(journal)
    }
}

/// Called by the real merge and undo entries, while entity-trust is held.
/// Returns a committed operation so a retry can return its existing result.
/// No source restoration occurs after the durable source-commit record.
pub(super) fn recover(journal: &Path) -> Result<Option<Value>, SnapshotError> {
    let root = recovery_root(journal)?;
    if !path_lexists(&root).map_err(SnapshotError::Path)? {
        return Ok(None);
    }
    let entries = recovery_entries(&root)?;
    let mut state: Value = read_record(journal, &root.join("state.json"), Value::Null)?;
    if state.is_null() {
        if entries.is_empty() {
            remove_dir_all(journal, RECOVERY).map_err(SnapshotError::Path)?;
            return Ok(None);
        }
        return Err(failure(
            &root,
            "entity recovery state is missing or invalid; before-images retained",
        ));
    }
    if !state["source_committed"].is_boolean() {
        return Err(failure(
            &root,
            "entity recovery commit state is invalid; before-images retained",
        ));
    }
    if state.get("finished") == Some(&Value::Bool(true)) {
        finish(journal)?;
        return Ok((state.get("source_committed") == Some(&Value::Bool(true))).then_some(state));
    }
    if state.get("source_committed") == Some(&Value::Bool(true)) {
        repair_index(journal, &mut state).map_err(|error| {
            failure(
                &root,
                format!("entity source change committed; index repair pending: {error}"),
            )
        })?;
        finish(journal)?;
        return Ok(Some(state));
    }
    let mut paths = entries
        .into_iter()
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    paths.sort();
    let count = state["snapshot_count"]
        .as_u64()
        .ok_or_else(|| failure(&root, "invalid entity recovery snapshot count"))?;
    let mut rollback = MergeRollback::default();
    for path in paths {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.len() != 13
            || !name.ends_with(".json")
            || !name[..8].bytes().all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        if name != format!("{:08}.json", rollback.snapshots.len()) {
            return Err(failure(
                &path,
                "entity recovery before-image is missing; before-images retained",
            ));
        }
        let value: Value = read_record(journal, &path, Value::Null)?;
        let snapshot = snapshot_from_payload(&value).map_err(|error| failure(&path, error))?;
        validate_before_image(journal, &snapshot, None)?;
        let relative = snapshot_path(&snapshot);
        if !(relative.starts_with("entities/")
            || relative.starts_with("facets/")
            || (relative.starts_with("chronicle/")
                && (relative.ends_with("/talents/speaker_labels.json")
                    || relative.ends_with("/talents/speaker_corrections.json")))
            || relative == "logs/entity-merges.jsonl")
        {
            return Err(failure(&path, "invalid entity recovery source path"));
        }
        // Other domain trees are protected by entity-trust. Segment writers
        // own their per-file locks and may have run since the interruption.
        if relative.starts_with("chronicle/") {
            rollback.lock_file(&contained_path(journal, relative).map_err(SnapshotError::Path)?)?;
        }
        rollback.snapshots.push(snapshot);
    }
    if rollback.snapshots.len() as u64 != count {
        return Err(failure(
            &root,
            "entity recovery before-image count differs; before-images retained",
        ));
    }
    let expected: Vec<String> = read_record(journal, &root.join("expected.json"), Vec::new())?;
    if expected.len() > rollback.snapshots.len() {
        return Err(failure(
            &root,
            "entity recovery checkpoint exceeds before-image set",
        ));
    }
    // Validate the complete set first. A new child, missing file, changed mode,
    // or changed bytes all refuse before any artifact is restored.
    for (index, snapshot) in rollback.snapshots.iter().enumerate() {
        let path = snapshot_path(snapshot);
        let current = capture_snapshot(journal, path)?;
        let matches = expected.get(index).map_or_else(
            || current == *snapshot,
            |expected| fingerprint(&current) == *expected,
        );
        if !matches {
            return Err(failure(
                &root,
                format!(
                    "interrupted entity merge recovery conflicts at {path}; current data and before-images retained"
                ),
            ));
        }
    }
    rollback.restore(journal)?;
    Ok(None)
}

pub(super) fn repair_index(journal: &Path, state: &mut Value) -> Result<(), String> {
    remove_file(journal, "awareness/discovery_clusters.json").map_err(|error| error.to_string())?;
    let report = &state["report"];
    match state["operation"].as_str() {
        Some("merge") => {
            let source = report["source_id"]
                .as_str()
                .ok_or("missing source entity")?;
            let target = report["target_id"]
                .as_str()
                .ok_or("missing target entity")?;
            let folded = solstone_core_indexer_store::merge::fold_entity_edges_for_recorded_merge(
                journal, source, target,
            )
            .map_err(|error| error.to_string())?;
            state["report"]["counts"]["edges"] = json!({"rows_folded":folded.rows_folded,"self_edges_dropped":folded.self_edges_dropped,"error":null});
            if let Some(phases) = state["report"]["completed_phases"].as_array_mut() {
                phases.push(json!("edges"));
            }
        }
        Some("undo") => {
            solstone_core_indexer_store::merge::rebuild_edges_for_recorded_merge_undo(journal)
                .map_err(|error| error.to_string())?;
        }
        _ => return Err("invalid committed entity operation".to_owned()),
    }
    Ok(())
}

fn snapshot_path(snapshot: &JournalSnapshot) -> &str {
    match snapshot {
        JournalSnapshot::Missing { path } => path,
        JournalSnapshot::File(file) => &file.path,
        JournalSnapshot::Directory(directory) => &directory.path,
    }
}

pub(super) fn fingerprint(snapshot: &JournalSnapshot) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&snapshot_payload(snapshot)).expect("snapshot JSON serializes")
        )
    )
}

fn write_record(journal: &Path, name: &str, value: &Value) -> Result<(), SnapshotError> {
    let path = recovery_root(journal)?.join(name);
    let bytes = serde_json::to_vec(value).map_err(|error| failure(&path, error))?;
    match atomic_replace_detailed(&path, &bytes, 0o600).map_err(|error| failure(&path, error))? {
        DetailedAtomicOutcome::Published => {}
        outcome => {
            return Err(failure(
                &path,
                format!("entity recovery publication uncertain: {outcome:?}"),
            ));
        }
    }
    Ok(())
}

fn finish(journal: &Path) -> Result<(), SnapshotError> {
    // Keep the terminal marker until all before-images have been removed. An
    // interrupted cleanup must never turn a committed operation into rollback.
    let root = recovery_root(journal)?;
    let mut state: Value = read_record(
        journal,
        &root.join("state.json"),
        json!({"source_committed":false}),
    )?;
    state["finished"] = json!(true);
    write_record(journal, "state.json", &state)?;
    let entries = recovery_entries(&root)?;
    for entry in entries {
        if entry.name != "state.json" {
            remove_file(
                journal,
                &format!("{RECOVERY}/{}", entry.name.to_string_lossy()),
            )
            .map_err(SnapshotError::Path)?;
        }
    }
    #[cfg(unix)]
    solstone_core_journal_io::sync_dir(journal, RECOVERY).map_err(SnapshotError::Path)?;
    remove_file(journal, &format!("{RECOVERY}/state.json")).map_err(SnapshotError::Path)?;
    remove_dir_all(journal, RECOVERY).map_err(SnapshotError::Path)?;
    #[cfg(unix)]
    solstone_core_journal_io::sync_dir(journal, "health").map_err(SnapshotError::Path)?;
    Ok(())
}

fn recovery_entries(root: &Path) -> Result<Vec<DirEntry>, SnapshotError> {
    let entries = list_dir_entries(root).map_err(SnapshotError::Path)?;
    for entry in &entries {
        let name = entry
            .name
            .to_str()
            .ok_or_else(|| failure(root, "invalid entity recovery filename"))?;
        if entry.kind != DirEntryKind::File
            || !(name == "state.json"
                || name == "expected.json"
                || (name.len() == 13
                    && name.ends_with(".json")
                    && name[..8].bytes().all(|byte| byte.is_ascii_digit())))
        {
            return Err(failure(
                root,
                "unexpected entity recovery artifact; evidence retained",
            ));
        }
    }
    Ok(entries)
}

fn recovery_root(journal: &Path) -> Result<PathBuf, SnapshotError> {
    // Check the lexical path too: contained_path resolves in-journal symlinks.
    for relative in ["health", RECOVERY] {
        let path = resolve_journal_path(journal, relative).map_err(SnapshotError::Path)?;
        let parent = path
            .parent()
            .ok_or_else(|| failure(&path, "invalid entity recovery path"))?;
        if let Some(entry) = list_dir_entries(parent)
            .map_err(SnapshotError::Path)?
            .into_iter()
            .find(|entry| Some(entry.name.as_os_str()) == path.file_name())
            && entry.kind != DirEntryKind::Directory
        {
            return Err(failure(
                &path,
                "entity recovery path is not an ordinary directory",
            ));
        }
    }
    contained_path(journal, RECOVERY).map_err(SnapshotError::Path)
}

pub(super) fn validate_before_image(
    journal: &Path,
    snapshot: &JournalSnapshot,
    parent: Option<&str>,
) -> Result<(), SnapshotError> {
    let relative = snapshot_path(snapshot);
    resolve_journal_path(journal, relative).map_err(SnapshotError::Path)?;
    if parent.is_some_and(|parent| {
        !relative
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
    }) {
        return Err(failure(
            Path::new(relative),
            "entity recovery child is outside its before-image parent",
        ));
    }
    match snapshot {
        JournalSnapshot::Directory(directory) => {
            for child in &directory.entries {
                validate_before_image(journal, child, Some(relative))?;
            }
        }
        JournalSnapshot::File(file) if file.mode > 0o777 => {
            return Err(failure(
                Path::new(relative),
                "invalid entity recovery file mode",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn read_record<T: serde::de::DeserializeOwned>(
    journal: &Path,
    path: &Path,
    absent: T,
) -> Result<T, SnapshotError> {
    let root = realpath_non_strict(journal).map_err(SnapshotError::Path)?;
    let relative = path
        .strip_prefix(&root)
        .ok()
        .and_then(|path| path.to_str())
        .ok_or_else(|| failure(path, "invalid entity recovery record path"))?;
    match capture_snapshot(journal, relative)? {
        JournalSnapshot::Missing { .. } => Ok(absent),
        JournalSnapshot::File(file) => {
            serde_json::from_slice(&file.bytes).map_err(|error| failure(path, error))
        }
        _ => Err(failure(
            path,
            "entity recovery record is not an ordinary file",
        )),
    }
}

fn failure(path: &Path, error: impl std::fmt::Display) -> SnapshotError {
    SnapshotError::Io {
        path: path.to_owned(),
        source: std::io::Error::other(error.to_string()),
    }
}
