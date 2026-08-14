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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportManifestBackfillReport {
    pub scanned: usize,
    pub backfilled: usize,
    pub skipped_already_has_manifest: usize,
    pub skipped_no_retained_original: usize,
}

/// Backfill manifests for old import directories that retained their source.
pub fn backfill_retained_import_manifests(
    journal_root: &Path,
) -> Result<ImportManifestBackfillReport, ImportError> {
    let mut report = ImportManifestBackfillReport::default();
    let imports = journal_root.join("imports");
    let entries = match fs::read_dir(&imports) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(error) => {
            return Err(ImportError::PathResolution {
                path: imports,
                message: error.to_string(),
            });
        }
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        report.scanned += 1;
        if dir.join("manifest.json").exists() {
            report.skipped_already_has_manifest += 1;
            continue;
        }
        let Ok(import_meta) = fs::read(dir.join("import.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .ok_or(())
        else {
            report.skipped_no_retained_original += 1;
            continue;
        };
        let Some(file_path) = import_meta.get("file_path").and_then(Value::as_str) else {
            report.skipped_no_retained_original += 1;
            continue;
        };
        let Some(file_name) = Path::new(file_path).file_name() else {
            report.skipped_no_retained_original += 1;
            continue;
        };
        let retained = dir.join(file_name);
        if !retained.is_file() {
            report.skipped_no_retained_original += 1;
            continue;
        }
        let imported = fs::read(dir.join("imported.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .unwrap_or(Value::Object(Map::new()));
        let source_type = imported
            .get("source_type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                match retained
                    .extension()
                    .and_then(|v| v.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "m4a" => "apple",
                    "txt" | "md" | "pdf" => "text",
                    _ => "audio",
                }
            });
        let files_created = imported
            .get("all_created_files")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let days = imported
            .get("target_day")
            .and_then(Value::as_str)
            .map(|day| vec![day.to_owned()])
            .or_else(|| {
                imported
                    .get("date_range")
                    .and_then(Value::as_array)
                    .map(|items| {
                        let mut days = items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>();
                        days.sort();
                        days.dedup();
                        days
                    })
            })
            .unwrap_or_default();
        let hash = hash_source(&retained)?;
        let import_id = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        write_manifest(&ManifestWriteRequest {
            journal_root,
            import_id,
            source_type,
            source_hash: &hash,
            entry_count: files_created.len() as u64,
            days_affected: &days,
            files_created: &files_created,
            imported_via: "import",
            link_id: None,
            observer_handle: None,
            raw_retention: None,
        })?;
        report.backfilled += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod backfill_tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn backfills_only_retained_legacy_original() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("imports/123");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("import.json"),
            br#"{"file_path":"/old/audio.m4a"}"#,
        )
        .unwrap();
        fs::write(dir.join("audio.m4a"), b"audio").unwrap();
        let report = backfill_retained_import_manifests(temp.path()).unwrap();
        assert_eq!(report.backfilled, 1);
        assert!(dir.join("manifest.json").is_file());
        assert_eq!(
            backfill_retained_import_manifests(temp.path())
                .unwrap()
                .skipped_already_has_manifest,
            1
        );
    }
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
    let files = directory_files(import_dir)?;
    let mut total_bytes = 0_u64;
    let entries = files
        .into_iter()
        .map(|file| {
            total_bytes += file.size;
            let hash = hash_file(&file.path)?.into_inner();
            Ok(json!({
                "name": relative_name(&file)?,
                "bytes": file.size,
                "hash": hash,
            }))
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
    let files = directory_files(root)?;
    let mut lines = Vec::with_capacity(files.len());
    for entry in files {
        let relative = relative_name(&entry)?;
        lines.push(format!("{relative}:{}", entry.size));
    }
    let mut hasher = Sha256::new();
    hasher.update(lines.join("\n").as_bytes());
    Ok(SourceHash::new(format!("{:x}", hasher.finalize())))
}

fn directory_files(root: &Path) -> Result<Vec<DirectoryFile>, ImportError> {
    let mut files = Vec::new();
    collect_directory_files(root, Vec::new(), &mut files)?;
    files.sort_by(|left, right| left.parts.cmp(&right.parts));
    if let Some(file) = files
        .iter()
        .find(|file| file.parts.iter().any(|part| part.to_str().is_none()))
    {
        return Err(ImportError::NonUtf8DirectoryEntry {
            path: file.path.clone(),
        });
    }
    Ok(files)
}

fn relative_name(file: &DirectoryFile) -> Result<String, ImportError> {
    file.parts
        .iter()
        .map(|part| {
            part.to_str()
                .map(str::to_owned)
                .ok_or_else(|| ImportError::NonUtf8DirectoryEntry {
                    path: file.path.clone(),
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
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
