// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Dependency-free Obsidian vault catalogue sync.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::ImportError;
use crate::sync_state::{
    FileSyncBackend, FileSyncState, SyncActionFailure, SyncActionRequest, SyncActionSeams,
    SyncReport, load_sync_state, run_actions, write_sync_state,
};

/// Obsidian catalogue options, matching the reference's keyword shape.
pub struct ObsidianSyncOptions<'a> {
    pub journal: &'a Path,
    pub save: bool,
    pub source_path: Option<&'a Path>,
    pub force: bool,
}

pub type ObsidianSyncState = FileSyncState;
pub type ObsidianFileState = Map<String, Value>;

pub fn sync_obsidian<A>(
    options: &ObsidianSyncOptions<'_>,
    seams: &mut SyncActionSeams<A>,
) -> Result<SyncReport, ImportError>
where
    A: for<'a> FnMut(SyncActionRequest<'a>) -> Result<(), SyncActionFailure>,
{
    let existing = load_sync_state(options.journal, FileSyncBackend::Obsidian)?;
    let vault = resolve_vault(options.source_path, existing.as_ref())?;
    let mut state = existing.unwrap_or_else(|| {
        FileSyncState::empty(FileSyncBackend::Obsidian, Some(vault.display().to_string()))
    });
    state.source_path = Some(vault.display().to_string());
    if options.force {
        state.files.clear();
    }

    let mut current = BTreeSet::new();
    let mut paths = BTreeMap::new();
    for path in markdown_files(&vault)? {
        let relative = path
            .strip_prefix(&vault)
            .expect("discovered below vault")
            .to_string_lossy()
            .replace('\\', "/");
        current.insert(relative.clone());
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        if content.trim().is_empty() {
            continue;
        }
        let metadata = fs::metadata(&path).map_err(|error| catalog_error(error.to_string()))?;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0.0, |value| value.as_secs_f64());
        let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
        let previously_imported = state.files.get(&relative).is_some_and(|entry| {
            entry.get("status").and_then(Value::as_str) == Some("imported")
                && entry.get("content_hash").and_then(Value::as_str) == Some(hash.as_str())
                && !options.force
        });
        let mut entry = Map::new();
        entry.insert(
            "filename".to_owned(),
            Value::String(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
                    .to_owned(),
            ),
        );
        entry.insert(
            "title".to_owned(),
            Value::String(
                path.file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
                    .to_owned(),
            ),
        );
        entry.insert("mtime".to_owned(), json!(mtime));
        entry.insert("content_hash".to_owned(), Value::String(hash));
        entry.insert(
            "edit_count".to_owned(),
            json!(
                state
                    .files
                    .get(&relative)
                    .and_then(|entry| entry.get("edit_count"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
        );
        entry.insert(
            "status".to_owned(),
            Value::String(
                if previously_imported {
                    "imported"
                } else {
                    "available"
                }
                .to_owned(),
            ),
        );
        if previously_imported && let Some(imported_at) = state.files[&relative].get("imported_at")
        {
            entry.insert("imported_at".to_owned(), imported_at.clone());
        }
        state.files.insert(relative.clone(), Value::Object(entry));
        paths.insert(relative, path);
    }
    for (relative, entry) in &mut state.files {
        if !current.contains(relative) {
            entry["status"] = Value::String("removed".to_owned());
        }
    }
    state.stamp();
    write_sync_state(options.journal, &state)?;
    run_actions(&mut state, options.save, options.journal, seams, &paths)
}

fn resolve_vault(
    source_path: Option<&Path>,
    state: Option<&FileSyncState>,
) -> Result<PathBuf, ImportError> {
    let candidate = source_path
        .map(Path::to_path_buf)
        .or_else(|| state.and_then(|state| state.source_path.as_ref().map(PathBuf::from)))
        .or_else(default_vault)
        .ok_or_else(|| {
            catalog_error(
                "No Obsidian vault found. Use --path to specify your vault location.".to_owned(),
            )
        })?;
    if !candidate.is_dir() {
        return Err(catalog_error(format!(
            "Obsidian vault not found at {}. Use --path to specify your vault location.",
            candidate.display()
        )));
    }
    candidate
        .canonicalize()
        .map_err(|error| catalog_error(error.to_string()))
}

fn default_vault() -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    [home.join("Documents/Obsidian"), home.join("Obsidian")]
        .into_iter()
        .find(|path| path.is_dir())
}

fn markdown_files(root: &Path) -> Result<Vec<PathBuf>, ImportError> {
    let mut files = Vec::new();
    walk_markdown(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_markdown(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), ImportError> {
    for entry in fs::read_dir(directory).map_err(|error| catalog_error(error.to_string()))? {
        let entry = entry.map_err(|error| catalog_error(error.to_string()))?;
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| catalog_error(error.to_string()))?;
        if metadata.is_dir() {
            let relative = path.strip_prefix(root).expect("walk remains rooted");
            if relative
                .components()
                .any(|part| part.as_os_str() == "Templates" || part.as_os_str() == "_templates")
            {
                continue;
            }
            walk_markdown(root, &path, files)?;
        } else if metadata.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn catalog_error(message: String) -> ImportError {
    ImportError::SyncCatalog {
        backend: "obsidian",
        message,
    }
}
