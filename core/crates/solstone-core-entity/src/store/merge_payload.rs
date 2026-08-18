// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::contained_path;
use solstone_core_journal_io::list_dir_entries;
use solstone_core_journal_io::path_lexists;
use solstone_core_journal_io::read_json;
use solstone_core_journal_io::restore_snapshot;
use solstone_core_journal_io::write_json;
use solstone_core_journal_io::AtomicWriteError;
use solstone_core_journal_io::DirEntryKind;
use solstone_core_journal_io::JournalSnapshot;
use solstone_core_journal_io::JsonWriteOptions;
use solstone_core_journal_io::MalformedPolicy;
use solstone_core_journal_io::PathError;
use solstone_core_journal_io::SnapshotDirectory;
use solstone_core_journal_io::SnapshotError;
use solstone_core_journal_io::SnapshotFile;

#[derive(Debug)]
pub enum MergePayloadError {
    Path(PathError),
    Read(solstone_core_journal_io::ReadError),
    Write(AtomicWriteError),
    Snapshot(SnapshotError),
    Invalid(String),
}

impl fmt::Display for MergePayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Snapshot(error) => error.fmt(formatter),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}
impl Error for MergePayloadError {}

pub(crate) fn record_entity_merge_payload(
    journal: &Path,
    entity_id: &str,
    merge_id: &str,
    payload: &Value,
) -> Result<String, MergePayloadError> {
    validate_merge_payload(journal, payload)?;
    let path = payload_path(journal, entity_id, merge_id)?;
    write_json(
        &path,
        payload,
        JsonWriteOptions {
            indent: Some(2),
            sort_keys: false,
            mode: None,
        },
    )
    .map_err(MergePayloadError::Write)?;
    Ok(payload_relative_path(entity_id, merge_id))
}

pub(crate) fn load_entity_merge_payload(
    journal: &Path,
    entity_id: &str,
    merge_id: &str,
) -> Result<Value, MergePayloadError> {
    let path = payload_path(journal, entity_id, merge_id)?;
    if !path_lexists(&path).map_err(MergePayloadError::Path)? {
        return Err(invalid(&format!(
            "missing private merge payload for {entity_id}: {merge_id}"
        )));
    }
    let payload =
        read_json(&path, Value::Null, MalformedPolicy::Raise).map_err(MergePayloadError::Read)?;
    if !payload.is_object() {
        return Err(invalid(&format!(
            "private merge payload is not an object: {}",
            path.display()
        )));
    }
    validate_merge_payload(journal, &payload).map_err(|error| {
        MergePayloadError::Invalid(format!(
            "invalid private merge payload for {entity_id}:{merge_id}: {error}"
        ))
    })?;
    Ok(payload)
}

pub(crate) fn move_entity_merge_payload(
    journal: &Path,
    source_id: &str,
    target_id: &str,
    merge_id: &str,
    rebased_from_entity_id: Option<&str>,
) -> Result<(Value, String), MergePayloadError> {
    let mut payload = load_entity_merge_payload(journal, source_id, merge_id)?;
    payload
        .as_object_mut()
        .ok_or_else(|| MergePayloadError::Invalid("merge payload is not an object".to_owned()))?
        .insert("target_id".to_owned(), Value::String(target_id.to_owned()));
    if let Some(rebased_from_entity_id) = rebased_from_entity_id {
        payload.as_object_mut().expect("validated object").insert(
            "rebased_from_entity_id".to_owned(),
            Value::String(rebased_from_entity_id.to_owned()),
        );
    }
    let target_rel = record_entity_merge_payload(journal, target_id, merge_id, &payload)?;
    if source_id != target_id {
        remove_entity_merge_payload(journal, source_id, merge_id)?;
    }
    Ok((payload, target_rel))
}

pub(crate) fn remove_entity_merge_payload(
    journal: &Path,
    entity_id: &str,
    merge_id: &str,
) -> Result<(), MergePayloadError> {
    let path = payload_path(journal, entity_id, merge_id)?;
    if path_lexists(&path).map_err(MergePayloadError::Path)? {
        restore_snapshot(
            journal,
            &JournalSnapshot::Missing {
                path: payload_relative_path(entity_id, merge_id),
            },
        )
        .map_err(MergePayloadError::Snapshot)?;
    }
    Ok(())
}

fn payload_path(
    journal: &Path,
    entity_id: &str,
    merge_id: &str,
) -> Result<std::path::PathBuf, MergePayloadError> {
    contained_path(
        journal,
        &format!("entities/{entity_id}/history/private/{merge_id}.json"),
    )
    .map_err(MergePayloadError::Path)
}

fn payload_relative_path(entity_id: &str, merge_id: &str) -> String {
    format!("entities/{entity_id}/history/private/{merge_id}.json")
}

pub(crate) fn list_entity_merge_payload_ids(
    journal: &Path,
    entity_id: &str,
) -> Result<Vec<String>, MergePayloadError> {
    let directory = contained_path(journal, &format!("entities/{entity_id}/history/private"))
        .map_err(MergePayloadError::Path)?;
    Ok(list_dir_entries(&directory)
        .map_err(MergePayloadError::Path)?
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .filter_map(|entry| {
            entry
                .name
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .map(ToOwned::to_owned)
        })
        .collect())
}

pub(crate) fn validate_merge_payload(
    journal: &Path,
    payload: &Value,
) -> Result<(), MergePayloadError> {
    let object = payload
        .as_object()
        .ok_or_else(|| invalid("merge payload is not an object"))?;
    let source_id = required_string(
        object,
        "source_id",
        "merge payload missing source entity id",
    )?;
    let target_id = required_string(
        object,
        "target_id",
        "merge payload missing target entity id",
    )?;
    contained_path(journal, &format!("entities/{source_id}")).map_err(MergePayloadError::Path)?;
    contained_path(journal, &format!("entities/{target_id}")).map_err(MergePayloadError::Path)?;
    let source_state = required_object(
        object,
        "source_state",
        "merge payload missing source_state",
        "merge payload source_state is not an object",
    )?;
    let snapshots = required_array(
        source_state,
        "snapshots",
        "merge payload missing snapshots",
        "merge payload snapshots is not a list",
    )?;
    for snapshot in snapshots {
        let snapshot = snapshot
            .as_object()
            .ok_or_else(|| invalid("merge payload snapshot is not an object"))?;
        let rel = optional_string(snapshot, "rel", "manifest snapshot missing relative path")?;
        contained_path(journal, rel).map_err(MergePayloadError::Path)?;
        if let Some(files) = snapshot.get("files") {
            for file in files
                .as_array()
                .ok_or_else(|| invalid("manifest snapshot files is not a list"))?
            {
                let file = file
                    .as_object()
                    .ok_or_else(|| invalid("manifest snapshot file is not an object"))?;
                let item_rel =
                    optional_string(file, "rel", "manifest snapshot file missing relative path")?;
                contained_path(journal, &format!("{rel}/{item_rel}"))
                    .map_err(MergePayloadError::Path)?;
            }
        }
    }
    let manifest = required_object(
        object,
        "manifest",
        "merge payload missing manifest",
        "merge payload manifest is not an object",
    )?;
    let identity = required_object(
        manifest,
        "identity",
        "merge payload missing identity manifest",
        "merge payload identity manifest is not an object",
    )?;
    for field in ["aka_support", "email_support", "scalar_support"] {
        required_array(
            identity,
            field,
            &format!("merge payload identity missing {field}"),
            &format!("merge payload identity {field} is not a list"),
        )?;
    }
    required_object(
        identity,
        "target_before",
        "merge payload identity missing target_before",
        "merge payload identity target_before is not an object",
    )?;
    let voiceprints = required_object(
        manifest,
        "voiceprints",
        "merge payload missing voiceprints manifest",
        "merge payload voiceprints manifest is not an object",
    )?;
    required_array(
        voiceprints,
        "support",
        "merge payload voiceprints missing support",
        "merge payload voiceprints support is not a list",
    )?;
    let facets = required_object(
        manifest,
        "facets",
        "merge payload missing facets manifest",
        "merge payload facets manifest is not an object",
    )?;
    let facets_entries = required_array(
        facets,
        "entries",
        "merge payload facets missing entries",
        "merge payload facets entries is not a list",
    )?;
    for entry in facets_entries {
        let entry = entry
            .as_object()
            .ok_or_else(|| invalid("merge payload facet entry is not an object"))?;
        let facet = required_string(entry, "facet", "manifest facet entry missing facet name")?;
        let directory = entry
            .get("target_dir")
            .and_then(Value::as_str)
            .filter(|dir| !dir.is_empty())
            .unwrap_or(target_id);
        contained_path(journal, &format!("facets/{facet}/entities/{directory}"))
            .map_err(MergePayloadError::Path)?;
    }
    for section in ["segments", "activities", "observation_relations"] {
        let section_value = required_object(
            manifest,
            section,
            &format!("merge payload missing {section} manifest"),
            &format!("merge payload {section} manifest is not an object"),
        )?;
        let entries = required_array(
            section_value,
            "entries",
            &format!("merge payload {section} missing entries"),
            &format!("merge payload {section} entries is not a list"),
        )?;
        for entry in entries {
            let entry = entry.as_object().ok_or_else(|| {
                invalid(&format!("merge payload {section} entry is not an object"))
            })?;
            let path = optional_string(
                entry,
                "path",
                &format!("manifest {section} entry missing path"),
            )?;
            contained_path(journal, path).map_err(MergePayloadError::Path)?;
        }
    }
    required_array(
        manifest,
        "rebased_merge_ids",
        "merge payload missing rebased_merge_ids",
        "merge payload rebased_merge_ids is not a list",
    )?;
    Ok(())
}

pub(crate) fn snapshot_payload(snapshot: &JournalSnapshot) -> Value {
    match snapshot {
        JournalSnapshot::Missing { path } => serde_json::json!({"kind":"missing","path":path}),
        JournalSnapshot::File(file) => {
            serde_json::json!({"kind":"file","path":file.path,"bytes":file.bytes,"mode":file.mode})
        }
        JournalSnapshot::Directory(directory) => serde_json::json!({
            "kind":"directory",
            "path":directory.path,
            "entries":directory.entries.iter().map(snapshot_payload).collect::<Vec<_>>(),
        }),
    }
}

pub(crate) fn snapshot_from_payload(value: &Value) -> Result<JournalSnapshot, MergePayloadError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("merge payload snapshot image is not an object"))?;
    let kind = optional_string(object, "kind", "merge payload snapshot image missing kind")?;
    let path = optional_string(object, "path", "merge payload snapshot image missing path")?;
    match kind {
        "missing" => Ok(JournalSnapshot::Missing {
            path: path.to_owned(),
        }),
        "file" => {
            let bytes = required_array(
                object,
                "bytes",
                "merge payload snapshot file missing bytes",
                "merge payload snapshot file bytes is not a list",
            )?
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(|| invalid("merge payload snapshot file bytes are invalid"))
            })
            .collect::<Result<Vec<_>, _>>()?;
            let mode = object
                .get("mode")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| invalid("merge payload snapshot file missing mode"))?;
            Ok(JournalSnapshot::File(SnapshotFile {
                path: path.to_owned(),
                bytes,
                mode,
            }))
        }
        "directory" => {
            let entries = required_array(
                object,
                "entries",
                "merge payload snapshot directory missing entries",
                "merge payload snapshot directory entries is not a list",
            )?
            .iter()
            .map(snapshot_from_payload)
            .collect::<Result<Vec<_>, _>>()?;
            Ok(JournalSnapshot::Directory(SnapshotDirectory {
                path: path.to_owned(),
                entries,
            }))
        }
        _ => Err(invalid("merge payload snapshot image has unknown kind")),
    }
}
fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    missing: &str,
) -> Result<&'a str, MergePayloadError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(missing))
}
fn optional_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    missing: &str,
) -> Result<&'a str, MergePayloadError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(missing))
}
fn required_object<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    missing: &str,
    invalid_message: &str,
) -> Result<&'a serde_json::Map<String, Value>, MergePayloadError> {
    object
        .get(key)
        .ok_or_else(|| invalid(missing))?
        .as_object()
        .ok_or_else(|| invalid(invalid_message))
}
fn required_array<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    missing: &str,
    invalid_message: &str,
) -> Result<&'a Vec<Value>, MergePayloadError> {
    object
        .get(key)
        .ok_or_else(|| invalid(missing))?
        .as_array()
        .ok_or_else(|| invalid(invalid_message))
}
fn invalid(message: &str) -> MergePayloadError {
    MergePayloadError::Invalid(message.to_owned())
}
