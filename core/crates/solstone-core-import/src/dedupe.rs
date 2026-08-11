// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source hashing and import-manifest deduplication.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use chrono::Local;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteOptions, DirEntryKind, atomic_replace, list_dir_entries, path_lexists,
};

use crate::{ImportError, SourceHash};

/// Inputs for one native manifest write.
pub struct ManifestWriteRequest<'a> {
    pub journal_root: &'a Path,
    pub import_id: &'a str,
    pub source_type: &'a str,
    pub source_hash: &'a SourceHash,
    pub entry_count: u64,
    pub days_affected: &'a [String],
    pub files_created: &'a [String],
    pub imported_via: &'a str,
    pub link_id: Option<&'a str>,
    pub observer_handle: Option<&'a str>,
    pub raw_retention: Option<&'a str>,
}

/// A matching import manifest and its path.
#[derive(Debug, Clone)]
pub struct ManifestMatch {
    pub path: PathBuf,
    pub manifest: Map<String, Value>,
}

/// One manifest that was skipped while scanning.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ManifestSkip {
    pub path: PathBuf,
    pub reason: ManifestSkipReason,
}

/// Reason a manifest was skipped.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ManifestSkipReason {
    Unreadable,
    MalformedJson,
    WrongJsonShape,
}

/// Observable result of scanning manifests by source hash.
#[derive(Debug, Clone)]
pub struct ManifestScan {
    pub found: Option<ManifestMatch>,
    pub skipped: Vec<ManifestSkip>,
}

/// Hash a file's bytes or a directory's compatible path-and-size listing.
pub fn hash_source(path: &Path) -> Result<SourceHash, ImportError> {
    let metadata = fs::metadata(path).map_err(|error| source_error(path, error))?;
    if metadata.is_file() {
        return hash_file(path);
    }
    if metadata.is_dir() {
        return hash_directory(path);
    }
    Err(ImportError::SourceNotFile {
        path: path.to_path_buf(),
    })
}

/// Hash a source and add the reference window identity when supplied.
pub fn windowed_source_hash(
    path: &Path,
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Result<SourceHash, ImportError> {
    let base = hash_source(path)?.into_inner();
    match (date_from, date_to) {
        (None, None) => Ok(SourceHash::new(base)),
        _ => {
            let start = date_from.unwrap_or("").replace('-', "");
            let end = date_to.unwrap_or("").replace('-', "");
            Ok(SourceHash::new(format!(
                "{base}#window:{}:{}",
                if start.is_empty() { "open" } else { &start },
                if end.is_empty() { "open" } else { &end }
            )))
        }
    }
}

/// Write a compatible import manifest without deriving days from paths.
pub fn write_manifest(request: &ManifestWriteRequest<'_>) -> Result<PathBuf, ImportError> {
    let import_dir =
        crate::staging::ensure_import_private_chain(request.journal_root, request.import_id)?;
    let path = import_dir.join("manifest.json");
    let mut manifest = Map::new();
    manifest.insert("import_id".to_owned(), json!(request.import_id));
    manifest.insert("source_type".to_owned(), json!(request.source_type));
    manifest.insert(
        "source_hash".to_owned(),
        json!(request.source_hash.as_str()),
    );
    manifest.insert("entry_count".to_owned(), json!(request.entry_count));
    manifest.insert("days_affected".to_owned(), json!(request.days_affected));
    manifest.insert("files_created".to_owned(), json!(request.files_created));
    manifest.insert(
        "imported_at".to_owned(),
        Value::String(Local::now().format("%Y-%m-%dT%H:%M:%S%.6f").to_string()),
    );
    manifest.insert("imported_via".to_owned(), json!(request.imported_via));
    manifest.insert("link_id".to_owned(), json!(request.link_id));
    manifest.insert("observer_handle".to_owned(), json!(request.observer_handle));
    if let Some(raw_retention) = request.raw_retention {
        manifest.insert("raw_retention".to_owned(), json!(raw_retention));
    }
    let bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|error| ImportError::ManifestWriteFailed {
            path: path.clone(),
            message: error.to_string(),
        })?;
    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) }).map_err(|error| {
        ImportError::ManifestWriteFailed {
            path: path.clone(),
            message: error.to_string(),
        }
    })?;
    Ok(path)
}

/// Scan manifest files and retain observable corruption skips.
pub fn find_manifest_by_hash(
    journal_root: &Path,
    source_hash: &SourceHash,
) -> Result<ManifestScan, ImportError> {
    let imports = journal_root.join("imports");
    if !path_lexists(&imports).map_err(|error| ImportError::PathResolution {
        path: imports.clone(),
        message: error.to_string(),
    })? {
        return Ok(ManifestScan {
            found: None,
            skipped: Vec::new(),
        });
    }
    let mut skipped = Vec::new();
    for entry in list_dir_entries(&imports).map_err(|error| ImportError::PathResolution {
        path: imports.clone(),
        message: error.to_string(),
    })? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let path = entry.path.join("manifest.json");
        if !path_lexists(&path).map_err(|error| ImportError::PathResolution {
            path: path.clone(),
            message: error.to_string(),
        })? {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                skipped.push(ManifestSkip {
                    path,
                    reason: ManifestSkipReason::Unreadable,
                });
                continue;
            }
        };
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                skipped.push(ManifestSkip {
                    path,
                    reason: ManifestSkipReason::MalformedJson,
                });
                continue;
            }
        };
        let Value::Object(manifest) = value else {
            skipped.push(ManifestSkip {
                path,
                reason: ManifestSkipReason::WrongJsonShape,
            });
            continue;
        };
        if manifest.get("source_hash").and_then(Value::as_str) == Some(source_hash.as_str()) {
            return Ok(ManifestScan {
                found: Some(ManifestMatch { path, manifest }),
                skipped,
            });
        }
    }
    Ok(ManifestScan {
        found: None,
        skipped,
    })
}

pub(crate) fn build_import_inventory(import_dir: &Path) -> Result<Value, ImportError> {
    let mut files = Vec::new();
    collect_inventory_files(import_dir, import_dir, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut total_bytes = 0_u64;
    let entries = files
        .into_iter()
        .map(|(relative, path, size)| {
            total_bytes += size;
            let hash = hash_file(&path)?.into_inner();
            Ok(json!({ "name": relative, "bytes": size, "hash": hash }))
        })
        .collect::<Result<Vec<_>, ImportError>>()?;
    Ok(json!({
        "timestamp": Local::now().to_rfc3339(),
        "import_dir": import_dir.display().to_string(),
        "total_bytes": total_bytes,
        "file_count": entries.len(),
        "files": entries,
    }))
}

fn hash_file(path: &Path) -> Result<SourceHash, ImportError> {
    let mut file = File::open(path).map_err(|error| source_error(path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| source_error(path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(SourceHash::new(format!("{:x}", hasher.finalize())))
}

fn hash_directory(root: &Path) -> Result<SourceHash, ImportError> {
    let mut files = Vec::new();
    collect_directory_files(root, Vec::new(), &mut files)?;
    files.sort_by(|left, right| left.parts.cmp(&right.parts));
    let invalid = files
        .iter()
        .filter(|entry| entry.parts.iter().any(|part| part.to_str().is_none()))
        .map(|entry| entry.path.clone())
        .min();
    if let Some(path) = invalid {
        return Err(ImportError::NonUtf8DirectoryEntry { path });
    }
    let mut lines = Vec::with_capacity(files.len());
    for entry in files {
        let relative = entry
            .parts
            .iter()
            .map(|part| part.to_str().expect("non-UTF-8 names were rejected"))
            .collect::<Vec<_>>()
            .join("/");
        lines.push(format!("{relative}:{}", entry.size));
    }
    let mut hasher = Sha256::new();
    hasher.update(lines.join("\n").as_bytes());
    Ok(SourceHash::new(format!("{:x}", hasher.finalize())))
}

struct DirectoryFile {
    parts: Vec<OsString>,
    path: PathBuf,
    size: u64,
}

fn collect_directory_files(
    directory: &Path,
    parts: Vec<OsString>,
    files: &mut Vec<DirectoryFile>,
) -> Result<(), ImportError> {
    for entry in fs::read_dir(directory).map_err(|error| source_error(directory, error))? {
        let entry = entry.map_err(|error| source_error(directory, error))?;
        let path = entry.path();
        let mut child_parts = parts.clone();
        child_parts.push(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| source_error(&path, error))?;
        if file_type.is_dir() {
            collect_directory_files(&path, child_parts, files)?;
            continue;
        }
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(source_error(&path, error)),
        };
        if metadata.is_file() {
            files.push(DirectoryFile {
                parts: child_parts,
                path,
                size: metadata.len(),
            });
        }
    }
    Ok(())
}

fn collect_inventory_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf, u64)>,
) -> Result<(), ImportError> {
    for entry in fs::read_dir(directory).map_err(|error| source_error(directory, error))? {
        let entry = entry.map_err(|error| source_error(directory, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| source_error(&path, error))?;
        if file_type.is_dir() {
            collect_inventory_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("walk remains under root")
                .to_string_lossy()
                .into_owned();
            files.push((
                relative,
                path,
                entry
                    .metadata()
                    .map_err(|error| source_error(root, error))?
                    .len(),
            ));
        }
    }
    Ok(())
}

fn source_error(path: &Path, error: io::Error) -> ImportError {
    if error.kind() == io::ErrorKind::NotFound {
        ImportError::SourceMissing {
            path: path.to_path_buf(),
        }
    } else {
        ImportError::PromotionFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}
