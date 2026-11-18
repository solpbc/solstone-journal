// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Forward-only choices for copies the journal stages during an import.

use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_journal_io::LockOptions;

use crate::{AttemptState, ImportMetadata, ProjectionStatus};

const PREFS: &str = "keep_import_originals";
pub(crate) const NEW_IMPORT: &str = "originals_policy_v1";
const STAGED_FILE: &str = "staged_original_v1";
const CHOICE: &str = "keep_original_at_import";

pub const SOURCES: &[&str] = &[
    "audio",
    "audio_archive",
    "chatgpt",
    "claude",
    "document",
    "evernote",
    "gemini",
    "plaud",
];

pub fn preferences(journal: &Path) -> Result<Map<String, Value>, String> {
    let config = read_journal_config(journal).map_err(|error| error.to_string())?;
    Ok(config
        .config
        .as_ref()
        .and_then(|config| config.get(PREFS))
        .and_then(Value::as_object)
        .map(|values| {
            SOURCES
                .iter()
                .filter_map(|source| {
                    let keep = values.get(*source)?.as_bool()?;
                    Some(((*source).to_owned(), Value::Bool(keep)))
                })
                .collect()
        })
        .unwrap_or_default())
}

pub fn set_preference(journal: &Path, source: &str, keep: bool) -> Result<(), String> {
    if !SOURCES.contains(&source) {
        return Err("unknown import source".to_owned());
    }
    mutate_journal_config(journal, LockOptions::default(), |config| {
        let preferences = config.entry(PREFS).or_insert_with(|| json!({}));
        if !preferences.is_object() {
            *preferences = json!({});
        }
        let preferences = preferences.as_object_mut().expect("preference object");
        let changed =
            preferences.insert(source.to_owned(), Value::Bool(keep)) != Some(Value::Bool(keep));
        JournalConfigMutation { changed, value: () }
    })
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// This witness is written by the upload path, after it installed the staged
/// file. A path import or a pre-update saved import never has this witness.
pub fn record_staged_upload(metadata: &mut ImportMetadata, path: &Path) {
    record_new_import(metadata);
    if let Some(leaf) = path.file_name().and_then(|leaf| leaf.to_str()) {
        metadata.insert(STAGED_FILE.to_owned(), json!(leaf));
    }
}

/// Mark the allocation of an import, independently of whether its source
/// pointer belongs to the journal.
pub fn record_new_import(metadata: &mut ImportMetadata) {
    metadata.insert(NEW_IMPORT.to_owned(), Value::Bool(true));
}

pub fn is_new_import(metadata: &ImportMetadata) -> bool {
    metadata.get(NEW_IMPORT).and_then(Value::as_bool) == Some(true)
}

/// Set an import's choice once. Sources without a choice and previously staged
/// files retain their historical behavior. Retries read the saved choice.
pub fn capture_choice(journal: &Path, id: &str, source: &str) -> Result<(), String> {
    if !SOURCES.contains(&source) {
        return Ok(());
    }
    let _lock = crate::hold_import_lock(journal, id).map_err(|error| error.to_string())?;
    let mut metadata =
        crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    if is_new_import(&metadata) && metadata.get(CHOICE).is_none() {
        let keep = preferences(journal)?
            .get(source)
            .and_then(Value::as_bool)
            .unwrap_or_default();
        metadata.insert(CHOICE.to_owned(), json!(keep));
        crate::metadata::write_import_metadata_unlocked(journal, id, &metadata)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn captured_choice(journal: &Path, id: &str) -> Option<bool> {
    crate::read_import_metadata(journal, id)
        .ok()?
        .get(CHOICE)?
        .as_bool()
}

pub(crate) fn preserve_custody(merged: &mut ImportMetadata, existing: &ImportMetadata) {
    for key in [NEW_IMPORT, STAGED_FILE, CHOICE] {
        if let Some(value) = existing.get(key) {
            merged.insert(key.to_owned(), value.clone());
        }
    }
}

/// Release an exact single-file upload after its source has completed all
/// durable writes. A caller retaining a canonical segment copy (image or
/// PDF) can also release its redundant staging copy when the choice is on.
pub fn discard_finished_upload(
    journal: &Path,
    id: &str,
    media: &Path,
    generation: u64,
    redundant: bool,
) -> Result<(), String> {
    let _target = solstone_core_journal_io::whole_journal_restore::begin_target_use(journal)
        .map_err(|error| error.to_string())?;
    let _lock = crate::hold_import_lock(journal, id).map_err(|error| error.to_string())?;
    let metadata = crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    if !is_new_import(&metadata) {
        return Ok(());
    }
    let discard_source = redundant || metadata.get(CHOICE).and_then(Value::as_bool) == Some(false);
    let preview = metadata.get("preview").and_then(Value::as_object);
    let stage = metadata.get(STAGED_FILE).and_then(Value::as_str);
    let input = metadata
        .get("file_path")
        .and_then(Value::as_str)
        .map(Path::new);
    let snapshot = preview
        .and_then(|record| record.get("relative"))
        .and_then(Value::as_str);
    let snapshot_path = snapshot.map(|rel| journal.join("imports").join(id).join(rel));
    if input != Some(media) && snapshot_path.as_deref() != Some(media) {
        return Ok(());
    }
    if !completed_generation(&metadata, generation)
        || crate::project_import_result(journal, id).status != ProjectionStatus::Success
    {
        return Ok(());
    }
    if (discard_source || stage.is_some())
        && let Some(relative) = snapshot
    {
        discard_preview_file(journal, id, relative, &metadata)?;
    }
    if discard_source && let Some(leaf) = stage {
        let mut components = Path::new(leaf).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err("saved upload name is invalid".to_owned());
        }
        let relative = format!("imports/{id}/{leaf}");
        let staged = solstone_core_journal_io::resolve_journal_path(journal, &relative)
            .map_err(|error| error.to_string())?;
        if input == Some(staged.as_path()) {
            discard_file(journal, &relative, expected_hash(&metadata)?)?;
        }
    }
    Ok(())
}

fn expected_hash(metadata: &ImportMetadata) -> Result<&str, String> {
    metadata
        .get("source_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| "the staged original has no saved fingerprint".to_owned())
}

fn discard_preview_file(
    journal: &Path,
    id: &str,
    relative: &str,
    metadata: &ImportMetadata,
) -> Result<(), String> {
    let mut components = Path::new(relative).components();
    let (Some(Component::Normal(directory)), Some(Component::Normal(_)), None) =
        (components.next(), components.next(), components.next())
    else {
        return Ok(());
    };
    if !directory
        .to_str()
        .is_some_and(|name| name.starts_with("preview-"))
    {
        return Ok(());
    }
    let relative = format!("imports/{id}/{relative}");
    // A folder source cleans up by member. Its directory hash is not a file hash.
    if journal.join(&relative).is_dir() {
        return Ok(());
    }
    discard_file(journal, &relative, expected_hash(metadata)?)
}

pub(crate) fn discard_file(journal: &Path, rel: &str, hash: &str) -> Result<(), String> {
    let root = journal.canonicalize().map_err(|error| error.to_string())?;
    let expected = solstone_core_journal_io::resolve_journal_path(&root, rel)
        .map_err(|error| error.to_string())?;
    let path = solstone_core_journal_io::contained_path(journal, rel)
        .map_err(|error| error.to_string())?;
    match std::fs::symlink_metadata(&expected) {
        Ok(file) if file.is_file() && path == expected => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(_) => return Err("staged original no longer names the saved file".to_owned()),
        Err(error) => return Err(error.to_string()),
    }
    let actual = crate::hash_source(&path).map_err(|error| error.to_string())?;
    if actual.as_str() != hash {
        return Err("staged original changed after it was saved".to_owned());
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "exact journal-created upload retired at completion, before it is kept as content"
    )]
    solstone_core_journal_io::remove_file(journal, rel).map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn journal_relative(path: &Path) -> Option<String> {
    let parts = path
        .components()
        .map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join("/"))
}

/// A folder-format export is a single original. Bind every member of its
/// admitted copy to the same snapshot before the importer reads it.
pub fn admitted_export_files(
    journal: &Path,
    id: &str,
    source: &Path,
) -> Result<Vec<(PathBuf, String)>, String> {
    let metadata = crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    let Some(snapshot) = metadata.get("folder_snapshot").and_then(Value::as_str) else {
        return Ok(Vec::new());
    };
    if !is_new_import(&metadata)
        || crate::saved_start::owned_import_source_relative(journal, source).is_none()
    {
        return Ok(Vec::new());
    }
    let (digest, files) =
        crate::snapshot::fingerprint_with_files(source).map_err(|error| error.to_string())?;
    if digest != snapshot {
        return Err("the staged export folder changed after it was saved".to_owned());
    }
    Ok(files)
}

/// Discard the journal's copies of confirmed members of an owned folder pick.
/// The producer supplies the exact bytes it processed. Paths outside the
/// registered tree and its admitted preview are never eligible.
pub fn discard_folder_members(
    journal: &Path,
    id: &str,
    generation: u64,
    processed: &[(PathBuf, String)],
    redundant: bool,
) -> Result<(), String> {
    let _target = solstone_core_journal_io::whole_journal_restore::begin_target_use(journal)
        .map_err(|error| error.to_string())?;
    let _lock = crate::hold_import_lock(journal, id).map_err(|error| error.to_string())?;
    let metadata = crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    if !completed_generation(&metadata, generation) || !is_new_import(&metadata) {
        return Ok(());
    }
    let duplicate = redundant || metadata.get(CHOICE).and_then(Value::as_bool) == Some(false);
    release_folder_members(journal, id, &metadata, processed, duplicate)
}

fn completed_generation(metadata: &ImportMetadata, generation: u64) -> bool {
    crate::get_attempt_facts(metadata).is_some_and(|facts| {
        facts.generation == generation
            && facts.state == AttemptState::Completed
            && facts.failure_reason.is_none()
            && facts.input_failures.unwrap_or_default() == 0
    })
}

fn release_folder_members(
    journal: &Path,
    id: &str,
    metadata: &ImportMetadata,
    processed: &[(PathBuf, String)],
    release_tree_files: bool,
) -> Result<(), String> {
    if !is_new_import(metadata)
        || metadata
            .get("folder_snapshot")
            .and_then(Value::as_str)
            .is_none()
    {
        return Ok(());
    }
    let import_dir = journal.join("imports").join(id);
    let tree = import_dir.join("tree");
    if metadata
        .get("file_path")
        .and_then(Value::as_str)
        .map(Path::new)
        != Some(tree.as_path())
    {
        return Ok(());
    }
    let preview_tree = metadata
        .get("preview")
        .and_then(|record| record["relative"].as_str())
        .and_then(|relative| {
            let mut parts = Path::new(relative).components();
            match (parts.next(), parts.next(), parts.next()) {
                (Some(Component::Normal(parent)), Some(Component::Normal(tree)), None)
                    if tree == "tree"
                        && parent
                            .to_str()
                            .is_some_and(|name| name.starts_with("preview-")) =>
                {
                    Some(import_dir.join(relative))
                }
                _ => None,
            }
        });
    for (path, hash) in processed {
        let relative = path.strip_prefix(&tree).ok().or_else(|| {
            preview_tree
                .as_ref()
                .and_then(|preview| path.strip_prefix(preview).ok())
        });
        let Some(relative) = relative else {
            continue;
        };
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err("invalid saved folder member".to_owned());
        }
        if let Some(preview) = &preview_tree {
            let file = preview.join(relative);
            let rel = file
                .strip_prefix(journal)
                .map_err(|error| error.to_string())?;
            discard_file(
                journal,
                &journal_relative(rel).ok_or("invalid preview member name")?,
                hash,
            )?;
        }
        if release_tree_files {
            let file = tree.join(relative);
            let rel = file
                .strip_prefix(journal)
                .map_err(|error| error.to_string())?;
            discard_file(
                journal,
                &journal_relative(rel).ok_or("invalid saved folder member name")?,
                hash,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn discard_archive_folder(
    journal: &Path,
    run_id: &str,
    source: &Path,
) -> Result<(), String> {
    let Some(relative) = crate::saved_start::owned_import_source_relative(journal, source) else {
        return Ok(());
    };
    let Some(Component::Normal(stage_id)) = relative.components().next() else {
        return Ok(());
    };
    let Some(stage_id) = stage_id
        .to_str()
        .filter(|id| crate::validate_timestamp(id).is_ok())
    else {
        return Ok(());
    };
    if stage_id == run_id {
        return Ok(());
    }
    let _lock = crate::hold_import_lock(journal, stage_id).map_err(|error| error.to_string())?;
    let stage =
        crate::read_import_metadata(journal, stage_id).map_err(|error| error.to_string())?;
    let Some(run) = crate::audio_archive::read_audio_archive(journal, run_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let files = run
        .members
        .iter()
        .filter(|member| member.disposition == crate::audio_archive::MemberDisposition::Included)
        .map(|member| {
            (
                source.join(&member.original_relative_name),
                member.sha256.clone(),
            )
        })
        .collect::<Vec<_>>();
    release_folder_members(journal, stage_id, &stage, &files, true)
}

/// A direct path import does not already have an upload to keep. If the
/// owner opted in, copy its exact source into the import before publication.
pub fn preserve_unstaged_choice(
    journal: &Path,
    id: &str,
    media: &Path,
    hash: &str,
) -> Result<(), String> {
    let metadata = crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    if metadata.get(CHOICE).and_then(Value::as_bool) == Some(true)
        && metadata.get(STAGED_FILE).is_none()
        && metadata.get("preview").is_none()
        && media.is_file()
    {
        let name = media.file_name().ok_or("the original has no filename")?;
        preserve_original(journal, id, media, Path::new(name), hash)?;
    }
    Ok(())
}

/// Install the original once under a name private to this import. The exact
/// original is copied, including container and metadata. Existing files must
/// agree with the caller's hash before they can be used on retry.
pub fn preserve_original(
    journal: &Path,
    id: &str,
    media: &Path,
    relative: &Path,
    hash: &str,
) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("invalid original filename".to_owned());
    }
    let directory_rel = Path::new("imports")
        .join(id)
        .join("originals")
        .join(relative.parent().unwrap_or(Path::new("")));
    let portable = journal_relative(&directory_rel).ok_or("invalid original path")?;
    let directory = solstone_core_journal_io::contained_path(journal, &portable)
        .map_err(|error| error.to_string())?;
    if directory
        != journal
            .canonicalize()
            .map_err(|error| error.to_string())?
            .join(&directory_rel)
    {
        return Err("original directory changed".to_owned());
    }
    solstone_core_journal_io::create_directory_with_mode(&directory, 0o700)
        .map_err(|error| error.to_string())?;
    let path = directory.join(relative.file_name().ok_or("invalid original filename")?);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            if crate::hash_source(&path)
                .map_err(|error| error.to_string())?
                .as_str()
                != hash
            {
                return Err("the kept original changed".to_owned());
            }
            return Ok(path);
        }
        Ok(_) => return Err("the original destination is occupied".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let mut input = std::fs::File::open(media).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(&directory).map_err(|error| error.to_string())?;
    std::io::copy(&mut input, temporary.as_file_mut()).map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    if crate::hash_source(temporary.path())
        .map_err(|error| error.to_string())?
        .as_str()
        != hash
    {
        return Err("the import source changed while copying it".to_owned());
    }
    temporary
        .persist_noclobber(&path)
        .map_err(|error| error.to_string())?;
    solstone_core_journal_io::sync_dir(journal, &portable).map_err(|error| error.to_string())?;
    Ok(path)
}
