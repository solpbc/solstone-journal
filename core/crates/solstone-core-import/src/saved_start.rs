// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable first-Start permission, independent of a worker's later attempt.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ImportError;
use crate::metadata::{
    ImportMetadata, hold_import_lock, read_import_metadata, write_import_metadata_unlocked,
};

pub const SAVED_START_KEY: &str = "saved_start";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Witness {
    version: u8,
    import_id: String,
    source_hash: String,
    file_path: String,
    file_size: Value,
    consumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedStartState {
    Eligible,
    Consumed,
    Unknown,
}

fn unavailable() -> ImportError {
    ImportError::InvalidAttemptState {
        message: "saved import Start permission could not be confirmed".to_owned(),
    }
}

fn identity(metadata: &ImportMetadata, import_id: &str) -> Result<Witness, ImportError> {
    let string = |key: &str| {
        metadata
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(unavailable)
    };
    if string("import_id")? != import_id {
        return Err(unavailable());
    }
    Ok(Witness {
        version: 1,
        import_id: import_id.to_owned(),
        source_hash: string("source_hash")?,
        file_path: string("file_path")?,
        file_size: metadata.get("file_size").cloned().unwrap_or(Value::Null),
        consumed: false,
    })
}

fn witness(metadata: &ImportMetadata, import_id: &str) -> Result<Witness, ImportError> {
    let recorded: Witness = serde_json::from_value(
        metadata
            .get(SAVED_START_KEY)
            .cloned()
            .ok_or_else(unavailable)?,
    )
    .map_err(|_| unavailable())?;
    let mut expected = identity(metadata, import_id)?;
    expected.consumed = recorded.consumed;
    if recorded != expected {
        return Err(unavailable());
    }
    Ok(recorded)
}

/// Initialize only a newly saved record, before its first metadata publication.
/// Ordinary metadata writes must never call this to upgrade an existing record.
pub fn initialize_saved_start(
    metadata: &mut ImportMetadata,
    import_id: &str,
) -> Result<(), ImportError> {
    if metadata.contains_key(SAVED_START_KEY)
        || metadata.contains_key("attempt")
        || metadata.contains_key("task_id")
    {
        return Err(unavailable());
    }
    let value = serde_json::to_value(identity(metadata, import_id)?).map_err(|_| unavailable())?;
    metadata.insert(SAVED_START_KEY.to_owned(), value);
    Ok(())
}

/// Read-only eligibility. Missing or malformed history is never unconsumed.
pub fn saved_start_state(metadata: &ImportMetadata, import_id: &str) -> SavedStartState {
    match witness(metadata, import_id) {
        Ok(recorded) if recorded.consumed => SavedStartState::Consumed,
        Ok(_) if !metadata.contains_key("attempt") && !metadata.contains_key("task_id") => {
            SavedStartState::Eligible
        }
        _ => SavedStartState::Unknown,
    }
}

fn consume_with(
    metadata: &mut ImportMetadata,
    import_id: &str,
    write: impl FnOnce(&ImportMetadata) -> Result<(), ImportError>,
) -> Result<(), ImportError> {
    if saved_start_state(metadata, import_id) != SavedStartState::Eligible {
        return Err(unavailable());
    }
    let mut recorded = witness(metadata, import_id)?;
    recorded.consumed = true;
    metadata.insert(
        SAVED_START_KEY.to_owned(),
        serde_json::to_value(recorded).map_err(|_| unavailable())?,
    );
    write(metadata)
}

/// Consume under the maintained metadata lock before dispatch/acceptance.
/// The existing atomic writer's guarantees apply; a write error refuses work.
pub fn consume_saved_start(
    journal_root: &Path,
    import_id: &str,
    first_only: bool,
) -> Result<(), ImportError> {
    if !first_only {
        return revoke_saved_start(journal_root, import_id);
    }
    let _target_use =
        solstone_core_journal_io::whole_journal_restore::begin_target_use(journal_root)
            .map_err(|_| unavailable())?;
    let _lock = hold_import_lock(journal_root, import_id)?;
    let mut metadata = read_import_metadata(journal_root, import_id)?;
    if saved_start_state(&metadata, import_id) == SavedStartState::Eligible {
        let projection = crate::project_import_result(journal_root, import_id);
        if projection.status != crate::ProjectionStatus::Pending
            || projection.raw_publication.is_some()
        {
            return Err(unavailable());
        }
    }
    consume_with(&mut metadata, import_id, |metadata| {
        write_import_metadata_unlocked(journal_root, import_id, metadata).map(|_| ())
    })
}

/// Revoke future first-Start eligibility before an independently authorized
/// native/desktop request is accepted. This is not a Start grant or retry check:
/// a consumed witness is idempotent, and legacy absence is never upgraded.
pub fn revoke_saved_start(journal_root: &Path, import_id: &str) -> Result<(), ImportError> {
    let _target_use =
        solstone_core_journal_io::whole_journal_restore::begin_target_use(journal_root)
            .map_err(|_| unavailable())?;
    let _lock = hold_import_lock(journal_root, import_id)?;
    let mut metadata = read_import_metadata(journal_root, import_id)?;
    if !metadata.contains_key(SAVED_START_KEY) {
        return Ok(());
    }
    revoke_with(&mut metadata, import_id, |metadata| {
        write_import_metadata_unlocked(journal_root, import_id, metadata).map(|_| ())
    })
}

fn revoke_with(
    metadata: &mut ImportMetadata,
    import_id: &str,
    write: impl FnOnce(&ImportMetadata) -> Result<(), ImportError>,
) -> Result<(), ImportError> {
    if !metadata.contains_key(SAVED_START_KEY) {
        return Ok(());
    }
    let mut recorded = witness(metadata, import_id)?;
    if recorded.consumed {
        return Ok(());
    }
    recorded.consumed = true;
    metadata.insert(
        SAVED_START_KEY.to_owned(),
        serde_json::to_value(recorded).map_err(|_| unavailable())?,
    );
    write(metadata)
}

/// Preserve absence as well as consumption against stale complete-record writes.
pub(crate) fn preserve_saved_start(merged: &mut ImportMetadata, existing: &ImportMetadata) {
    if let Some(recorded) = existing.get(SAVED_START_KEY) {
        merged.insert(SAVED_START_KEY.to_owned(), recorded.clone());
    } else {
        merged.remove(SAVED_START_KEY);
    }
}

/// An owned Start may rename its record to an explicit owner date. Carry only
/// already-consumed custody to that exact new location; this cannot mint a
/// first-Start grant or make a changed source eligible.
pub(crate) fn preserve_consumed_relocation(
    root: &Path,
    import_id: &str,
    merged: &mut ImportMetadata,
    existing: &ImportMetadata,
) {
    let Some(old_id) = existing.get("import_id").and_then(Value::as_str) else {
        return;
    };
    if old_id == import_id {
        return;
    }
    let Ok(old) = witness(existing, old_id) else {
        return;
    };
    if !old.consumed {
        return;
    }
    let Ok(mut relocated) = identity(merged, import_id) else {
        return;
    };
    let old_directory = root.join("imports").join(old_id);
    let Ok(relative) = Path::new(&old.file_path).strip_prefix(&old_directory) else {
        return;
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        || Path::new(&relocated.file_path) != root.join("imports").join(import_id).join(relative)
        || relocated.source_hash != old.source_hash
        || relocated.file_size != old.file_size
    {
        return;
    }
    relocated.consumed = true;
    merged.insert(
        SAVED_START_KEY.to_owned(),
        serde_json::to_value(relocated).expect("saved Start witness is serializable"),
    );
}

/// A direct producer records consumption with its durable attempt/task facts.
pub(crate) fn consume_with_attempt(metadata: &mut ImportMetadata, import_id: &str) {
    if let Ok(mut recorded) = witness(metadata, import_id) {
        recorded.consumed = true;
        metadata.insert(
            SAVED_START_KEY.to_owned(),
            serde_json::to_value(recorded).expect("saved Start witness is serializable"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "20261029_120000";

    fn saved() -> ImportMetadata {
        let mut metadata = json!({
            "import_id": ID, "source_hash": "source-digest",
            "file_path": "/journal/imports/20261029_120000/note.txt", "file_size": 6,
            "source_timestamp": "20250101_090000", "setting": "planning"
        })
        .as_object()
        .unwrap()
        .clone();
        initialize_saved_start(&mut metadata, ID).unwrap();
        metadata
    }

    #[test]
    fn legacy_corrupt_and_changed_sources_are_not_first_start_permissions() {
        let metadata = saved();
        assert_eq!(saved_start_state(&metadata, ID), SavedStartState::Eligible);
        let mut legacy = metadata.clone();
        legacy.remove(SAVED_START_KEY);
        assert_eq!(saved_start_state(&legacy, ID), SavedStartState::Unknown);
        assert!(consume_with(&mut legacy, ID, |_| panic!("must not write")).is_err());
        for key in ["import_id", "file_path", "source_hash", "file_size"] {
            let mut changed = metadata.clone();
            changed.insert(key.to_owned(), json!("changed"));
            assert_eq!(saved_start_state(&changed, ID), SavedStartState::Unknown);
        }
        let mut corrupt = metadata;
        corrupt[SAVED_START_KEY]["version"] = json!(2);
        assert_eq!(saved_start_state(&corrupt, ID), SavedStartState::Unknown);
    }

    #[test]
    fn consumption_precedes_success_and_persistence_failure_refuses_permission() {
        let mut metadata = saved();
        let mut persisted = None;
        consume_with(&mut metadata, ID, |record| {
            assert_eq!(saved_start_state(record, ID), SavedStartState::Consumed);
            persisted = Some(record.clone());
            Ok(())
        })
        .unwrap();
        let mut reopened = persisted.unwrap();
        assert!(consume_with(&mut reopened, ID, |_| panic!("must not write twice")).is_err());

        let mut metadata = saved();
        assert!(consume_with(&mut metadata, ID, |_| Err(unavailable())).is_err());
        assert_eq!(saved_start_state(&metadata, ID), SavedStartState::Consumed);
    }

    #[test]
    fn revocation_is_monotone_and_a_failed_write_returns_no_success() {
        let mut metadata = saved();
        assert!(revoke_with(&mut metadata, ID, |_| Err(unavailable())).is_err());
        let mut metadata = saved();
        revoke_with(&mut metadata, ID, |value| {
            assert_eq!(saved_start_state(value, ID), SavedStartState::Consumed);
            Ok(())
        })
        .unwrap();
        revoke_with(&mut metadata, ID, |_| {
            panic!("already consumed must not write")
        })
        .unwrap();
        metadata.remove(SAVED_START_KEY);
        revoke_with(&mut metadata, ID, |_| panic!("legacy must not write")).unwrap();
        assert_eq!(saved_start_state(&metadata, ID), SavedStartState::Unknown);
    }

    #[test]
    fn full_record_merge_preserves_consumption_and_legacy_absence() {
        let unconsumed = saved();
        let mut consumed = unconsumed.clone();
        consume_with(&mut consumed, ID, |_| Ok(())).unwrap();
        for mut stale in [unconsumed.clone(), {
            let mut absent = unconsumed.clone();
            absent.remove(SAVED_START_KEY);
            absent
        }] {
            crate::metadata::preserve_durable_attempt(&mut stale, &consumed);
            assert_eq!(saved_start_state(&stale, ID), SavedStartState::Consumed);
        }
        let mut edited = unconsumed.clone();
        edited.insert("setting".to_owned(), json!("new setting"));
        crate::metadata::preserve_durable_attempt(&mut edited, &unconsumed);
        assert_eq!(saved_start_state(&edited, ID), SavedStartState::Eligible);
        let mut legacy = unconsumed.clone();
        legacy.remove(SAVED_START_KEY);
        let mut attempted_upgrade = unconsumed;
        crate::metadata::preserve_durable_attempt(&mut attempted_upgrade, &legacy);
        assert_eq!(
            saved_start_state(&attempted_upgrade, ID),
            SavedStartState::Unknown
        );
    }

    #[test]
    fn relocation_carries_only_exact_already_consumed_source_custody() {
        let old = saved();
        let new_id = "20261030_090000";
        let relocated = |old: &ImportMetadata| {
            let mut moved = old.clone();
            moved.insert("import_id".into(), json!(new_id));
            moved.insert(
                "file_path".into(),
                json!("/journal/imports/20261030_090000/note.txt"),
            );
            moved
        };
        let mut moved = relocated(&old);
        preserve_consumed_relocation(Path::new("/journal"), new_id, &mut moved, &old);
        assert_eq!(saved_start_state(&moved, new_id), SavedStartState::Unknown);
        let mut consumed = old;
        consume_with(&mut consumed, ID, |_| Ok(())).unwrap();
        let mut moved = relocated(&consumed);
        preserve_consumed_relocation(Path::new("/journal"), new_id, &mut moved, &consumed);
        assert_eq!(saved_start_state(&moved, new_id), SavedStartState::Consumed);
        for (key, value) in [
            ("source_hash", json!("other")),
            ("file_size", json!(7)),
            (
                "file_path",
                json!("/other/imports/20261030_090000/note.txt"),
            ),
        ] {
            let mut changed = relocated(&consumed);
            changed.insert(key.into(), value);
            preserve_consumed_relocation(Path::new("/journal"), new_id, &mut changed, &consumed);
            assert_eq!(
                saved_start_state(&changed, new_id),
                SavedStartState::Unknown
            );
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn real_writer_stale_versions_cannot_reset_consumption_after_recreation() {
        let root = tempfile::tempdir().unwrap();
        let metadata = saved();
        crate::write_import_metadata(root.path(), ID, &metadata).unwrap();
        consume_saved_start(root.path(), ID, true).unwrap();
        for stale in [metadata.clone(), {
            let mut absent = metadata;
            absent.remove(SAVED_START_KEY);
            absent
        }] {
            crate::write_import_metadata(root.path(), ID, &stale).unwrap();
            let reopened = read_import_metadata(root.path(), ID).unwrap();
            assert_eq!(saved_start_state(&reopened, ID), SavedStartState::Consumed);
            assert!(consume_saved_start(root.path(), ID, true).is_err());
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn concurrent_consumers_get_one_first_start_permission() {
        let root = tempfile::tempdir().unwrap();
        crate::write_import_metadata(root.path(), ID, &saved()).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let root = root.path().to_path_buf();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    consume_saved_start(&root, ID, true).is_ok()
                })
            })
            .collect();
        assert_eq!(
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .filter(|success| *success)
                .count(),
            1
        );
    }
}
