// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exclusive allocation of `imports/YYYYMMDD_HHMMSS` record ids.
//!
//! The id is bookkeeping. Chronicle placement keeps the source timestamp the
//! caller already resolved. A probe may cross midnight only in that id.

use std::fs;
use std::io;
use std::path::Path;

use chrono::NaiveDateTime;
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    DirEntryKind, contained_path, create_directory_with_mode, list_dir_entries, path_lexists,
};

use crate::ImportError;
use crate::SourceHash;
use crate::dedupe::{find_manifest_by_hash_where, hash_source};
use crate::metadata::{read_import_metadata, read_provenance, write_import_metadata};
use crate::timestamp::{Timestamp, validate_timestamp};

/// Same bound the chronicle segment probe uses. The next candidate is not created.
pub const IMPORT_ID_PROBE_LIMIT: u32 = 60;

const DIRECTORY_MODE: u32 = 0o700;

/// A fresh import directory and the source time that chose the first candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllocatedImport {
    pub import_id: Timestamp,
    pub source_timestamp: Timestamp,
}

/// The record a caller admits. `import_id` is the directory. `source_timestamp`
/// is the pre-shift source time, which may equal `import_id`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundImport {
    pub import_id: Timestamp,
    pub source_timestamp: Timestamp,
}

/// Claim a fresh `imports/{id}` directory for `source_timestamp`.
///
/// Occupied, symlinked, and non-directory candidates are left in place. An
/// unreadable candidate or an unwritable `imports/` parent fails the allocation.
pub fn allocate_import_id(
    journal_root: &Path,
    source_timestamp: &Timestamp,
) -> Result<AllocatedImport, ImportError> {
    ensure_imports_directory(journal_root)?;
    for offset in 0..IMPORT_ID_PROBE_LIMIT {
        let candidate = shift_timestamp(source_timestamp, offset)?;
        // Stat the lexical path first. `contained_path` resolves through a
        // symlink, and a dangling link is an occupant, not a free id.
        let lexical = journal_root.join("imports").join(candidate.as_str());
        match fs::symlink_metadata(&lexical) {
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ImportError::PathResolution {
                    path: lexical,
                    message: error.to_string(),
                });
            }
        }
        let candidate_path = contained_import_dir(journal_root, candidate.as_str())?;
        match fs::create_dir(&candidate_path) {
            Ok(()) => {
                create_directory_with_mode(&candidate_path, DIRECTORY_MODE).map_err(|error| {
                    ImportError::PathResolution {
                        path: candidate_path,
                        message: error.to_string(),
                    }
                })?;
                return Ok(AllocatedImport {
                    import_id: candidate,
                    source_timestamp: source_timestamp.clone(),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                // Lost the race, or a symlink appeared. Do not reuse it.
                fs::symlink_metadata(&candidate_path).map_err(|error| {
                    ImportError::PathResolution {
                        path: candidate_path,
                        message: error.to_string(),
                    }
                })?;
            }
            Err(error) => {
                return Err(ImportError::PathResolution {
                    path: candidate_path,
                    message: error.to_string(),
                });
            }
        }
    }
    Err(ImportError::ImportIdExhausted {
        attempts: IMPORT_ID_PROBE_LIMIT,
    })
}

/// Bind a new or existing record for `requested`.
///
/// Reuse requires matching source bytes in metadata or a manifest, including
/// when an earlier collision shifted the matching record off the requested id.
/// Missing or malformed metadata is an occupant, not evidence of a retry.
/// Callers retrying an already-admitted generation retain its selected id
/// without invoking this fresh-input binding. A free requested id is claimed
/// as its own record; other occupied slots are left in place.
/// Dry-run callers must not call this.
pub fn bind_import_record(
    journal_root: &Path,
    requested: &Timestamp,
    source: Option<&Path>,
) -> Result<BoundImport, ImportError> {
    let source_hash = match source {
        Some(path) => Some(hash_source(path)?),
        None => None,
    };
    if let Some(hash) = &source_hash
        && let Some(import_id) = matching_record_id(journal_root, requested, hash)?
    {
        let source_timestamp = source_timestamp_of(journal_root, &import_id)?;
        return Ok(BoundImport {
            import_id,
            source_timestamp,
        });
    }
    let allocated = allocate_import_id(journal_root, requested)?;
    persist_identity(journal_root, &allocated, source_hash.as_ref())?;
    Ok(BoundImport {
        import_id: allocated.import_id,
        source_timestamp: allocated.source_timestamp,
    })
}

/// The source time stored on a record.
///
/// A record written by this allocator always has `source_timestamp`. A record
/// from before that write used its directory id as both, so a missing key is
/// that id and not a second syntax.
pub fn stored_source_timestamp(
    metadata: &Map<String, Value>,
    import_id: &str,
) -> Result<Timestamp, ImportError> {
    match metadata.get("source_timestamp").and_then(Value::as_str) {
        Some(raw) => validate_timestamp(raw).map_err(|_| ImportError::InvalidImportId {
            import_id: raw.to_owned(),
        }),
        None => validate_timestamp(import_id).map_err(|_| ImportError::InvalidImportId {
            import_id: import_id.to_owned(),
        }),
    }
}

/// Record `source_hash` on an existing import without dropping `source_timestamp`.
pub fn remember_source_hash(
    journal_root: &Path,
    import_id: &str,
    source_hash: &SourceHash,
) -> Result<(), ImportError> {
    let mut metadata = read_import_metadata(journal_root, import_id)?;
    metadata.insert("source_hash".to_owned(), json!(source_hash.as_str()));
    write_import_metadata(journal_root, import_id, &metadata).map(|_| ())
}

enum AddressedRecord {
    Absent,
    Occupant,
    NoHash,
    Hash(String),
}

fn matching_record_id(
    journal_root: &Path,
    requested: &Timestamp,
    source_hash: &SourceHash,
) -> Result<Option<Timestamp>, ImportError> {
    match addressed_record(journal_root, requested.as_str())? {
        AddressedRecord::Hash(stored) if stored == source_hash.as_str() => {
            Ok(Some(requested.clone()))
        }
        AddressedRecord::Absent => Ok(None),
        AddressedRecord::Hash(_) | AddressedRecord::NoHash | AddressedRecord::Occupant => {
            occupant_record_id(journal_root, source_hash)
        }
    }
}

fn occupant_record_id(
    journal_root: &Path,
    source_hash: &SourceHash,
) -> Result<Option<Timestamp>, ImportError> {
    if let Some(import_id) = find_record_by_source_hash(journal_root, source_hash)? {
        return Ok(Some(import_id));
    }
    let scan = find_manifest_by_hash_where(journal_root, source_hash, |found| {
        let Some(id) = found
            .path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .filter(|name| validate_timestamp(name).is_ok())
        else {
            return false;
        };
        match addressed_record(journal_root, id) {
            Ok(AddressedRecord::Hash(stored)) => stored == source_hash.as_str(),
            Ok(AddressedRecord::NoHash) => true,
            _ => false,
        }
    })?;
    let Some(found) = scan.found else {
        return Ok(None);
    };
    let Some(name) = found
        .path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
    else {
        return Ok(None);
    };
    Ok(validate_timestamp(name).ok())
}

fn addressed_record(journal_root: &Path, import_id: &str) -> Result<AddressedRecord, ImportError> {
    let directory = journal_root.join("imports").join(import_id);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(AddressedRecord::Occupant),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AddressedRecord::Absent);
        }
        Err(error) => {
            return Err(ImportError::PathResolution {
                path: directory,
                message: error.to_string(),
            });
        }
    }
    let metadata_path = directory.join("import.json");
    match fs::symlink_metadata(&metadata_path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(AddressedRecord::Occupant),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AddressedRecord::Occupant);
        }
        Err(error) => {
            return Err(ImportError::PathResolution {
                path: metadata_path,
                message: error.to_string(),
            });
        }
    }
    let metadata = match read_provenance(journal_root, import_id) {
        Ok(Some(metadata)) => metadata,
        Ok(None) | Err(ImportError::MetadataCorrupt { .. }) => {
            return Ok(AddressedRecord::Occupant);
        }
        Err(error) => return Err(error),
    };
    Ok(match metadata.get("source_hash").and_then(Value::as_str) {
        Some(hash) => AddressedRecord::Hash(hash.to_owned()),
        None => AddressedRecord::NoHash,
    })
}

fn find_record_by_source_hash(
    journal_root: &Path,
    source_hash: &SourceHash,
) -> Result<Option<Timestamp>, ImportError> {
    let imports = journal_root.join("imports");
    if !path_lexists(&imports).map_err(|error| ImportError::PathResolution {
        path: imports.clone(),
        message: error.to_string(),
    })? {
        return Ok(None);
    }
    let entries = list_dir_entries(&imports).map_err(|error| ImportError::PathResolution {
        path: imports.clone(),
        message: error.to_string(),
    })?;
    for entry in entries {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let Some(name) = entry.name.to_str() else {
            continue;
        };
        let Ok(import_id) = validate_timestamp(name) else {
            continue;
        };
        if provenance_hash_matches(journal_root, import_id.as_str(), source_hash)? {
            return Ok(Some(import_id));
        }
    }
    Ok(None)
}

fn provenance_hash_matches(
    journal_root: &Path,
    import_id: &str,
    source_hash: &SourceHash,
) -> Result<bool, ImportError> {
    Ok(matches!(
        addressed_record(journal_root, import_id)?,
        AddressedRecord::Hash(stored) if stored == source_hash.as_str()
    ))
}

fn source_timestamp_of(
    journal_root: &Path,
    import_id: &Timestamp,
) -> Result<Timestamp, ImportError> {
    match read_provenance(journal_root, import_id.as_str())? {
        Some(metadata) => stored_source_timestamp(&metadata, import_id.as_str()),
        None => Ok(import_id.clone()),
    }
}

fn persist_identity(
    journal_root: &Path,
    allocated: &AllocatedImport,
    source_hash: Option<&SourceHash>,
) -> Result<(), ImportError> {
    let mut metadata = Map::new();
    metadata.insert("import_id".to_owned(), json!(allocated.import_id.as_str()));
    metadata.insert(
        "source_timestamp".to_owned(),
        json!(allocated.source_timestamp.as_str()),
    );
    if let Some(source_hash) = source_hash {
        metadata.insert("source_hash".to_owned(), json!(source_hash.as_str()));
    }
    write_import_metadata(journal_root, allocated.import_id.as_str(), &metadata).map(|_| ())
}

fn ensure_imports_directory(journal_root: &Path) -> Result<(), ImportError> {
    let imports = journal_root.join("imports");
    match fs::symlink_metadata(&imports) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(ImportError::ImportDirectoryIsSymlink { path: imports })
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(ImportError::PathResolution {
            path: imports,
            message: "imports path is not a directory".to_owned(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(&imports) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(ImportError::PathResolution {
                        path: imports,
                        message: error.to_string(),
                    });
                }
            }
            // The loser of a concurrent create must accept the directory the
            // winner just made, and must still refuse a file or symlink.
            match fs::symlink_metadata(&imports) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    Err(ImportError::ImportDirectoryIsSymlink { path: imports })
                }
                Ok(metadata) if metadata.is_dir() => {
                    create_directory_with_mode(&imports, DIRECTORY_MODE).map_err(|error| {
                        ImportError::PathResolution {
                            path: imports,
                            message: error.to_string(),
                        }
                    })
                }
                Ok(_) => Err(ImportError::PathResolution {
                    path: imports,
                    message: "imports path is not a directory".to_owned(),
                }),
                Err(error) => Err(ImportError::PathResolution {
                    path: imports,
                    message: error.to_string(),
                }),
            }
        }
        Err(error) => Err(ImportError::PathResolution {
            path: imports,
            message: error.to_string(),
        }),
    }
}

fn contained_import_dir(
    journal_root: &Path,
    import_id: &str,
) -> Result<std::path::PathBuf, ImportError> {
    contained_path(journal_root, &format!("imports/{import_id}")).map_err(|error| {
        ImportError::PathResolution {
            path: journal_root.join("imports").join(import_id),
            message: error.to_string(),
        }
    })
}

fn shift_timestamp(source: &Timestamp, offset_seconds: u32) -> Result<Timestamp, ImportError> {
    let naive = NaiveDateTime::parse_from_str(source.as_str(), "%Y%m%d_%H%M%S").map_err(|_| {
        ImportError::InvalidImportId {
            import_id: source.as_str().to_owned(),
        }
    })?;
    let shifted = naive + chrono::Duration::seconds(i64::from(offset_seconds));
    let raw = shifted.format("%Y%m%d_%H%M%S").to_string();
    validate_timestamp(&raw).map_err(|_| ImportError::InvalidImportId { import_id: raw })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(all(unix, feature = "full-tests"))]
    use std::sync::{Arc, Barrier};
    #[cfg(all(unix, feature = "full-tests"))]
    use std::thread;

    fn stamp(raw: &str) -> Timestamp {
        validate_timestamp(raw).unwrap()
    }

    fn journal() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    #[test]
    fn first_candidate_matches_the_source_timestamp_and_records_it() {
        let root = journal();
        let source = stamp("20260616_120000");
        let allocated = allocate_import_id(root.path(), &source).unwrap();
        assert_eq!(allocated.import_id, source);
        assert_eq!(allocated.source_timestamp, source);
        let directory = root.path().join("imports/20260616_120000");
        assert!(directory.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::symlink_metadata(&directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
        persist_identity(root.path(), &allocated, None).unwrap();
        let metadata = read_import_metadata(root.path(), "20260616_120000").unwrap();
        assert_eq!(
            stored_source_timestamp(&metadata, "20260616_120000").unwrap(),
            source
        );
    }

    #[test]
    fn occupied_file_symlink_and_directory_are_left_in_place() {
        let root = journal();
        let imports = root.path().join("imports");
        fs::create_dir(&imports).unwrap();
        let source = stamp("20260616_120000");
        let file_id = imports.join("20260616_120000");
        fs::write(&file_id, b"keep").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("elsewhere", imports.join("20260616_120001")).unwrap();
        }
        fs::create_dir(imports.join("20260616_120002")).unwrap();
        fs::write(imports.join("20260616_120002/note"), b"occupant").unwrap();
        let allocated = allocate_import_id(root.path(), &source).unwrap();
        #[cfg(unix)]
        assert_eq!(allocated.import_id.as_str(), "20260616_120003");
        #[cfg(not(unix))]
        assert_eq!(allocated.import_id.as_str(), "20260616_120001");
        assert_eq!(fs::read(&file_id).unwrap(), b"keep");
        assert_eq!(
            fs::read(imports.join("20260616_120002/note")).unwrap(),
            b"occupant"
        );
        #[cfg(unix)]
        assert!(
            fs::symlink_metadata(imports.join("20260616_120001"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn probe_stops_at_sixty_and_a_file_parent_fails_closed() {
        let root = journal();
        let imports = root.path().join("imports");
        fs::create_dir(&imports).unwrap();
        let source = stamp("20260616_120000");
        for offset in 0..IMPORT_ID_PROBE_LIMIT {
            let id = shift_timestamp(&source, offset).unwrap();
            fs::create_dir(imports.join(id.as_str())).unwrap();
        }
        let error = allocate_import_id(root.path(), &source).unwrap_err();
        assert!(matches!(
            error,
            ImportError::ImportIdExhausted { attempts: 60 }
        ));
        assert!(!imports.join("20260616_120100").exists());

        let blocked = journal();
        fs::write(blocked.path().join("imports"), b"not-a-directory").unwrap();
        let error = allocate_import_id(blocked.path(), &source).unwrap_err();
        assert!(matches!(error, ImportError::PathResolution { .. }));
    }

    #[test]
    fn midnight_crosses_only_the_bookkeeping_id() {
        let root = journal();
        let imports = root.path().join("imports");
        fs::create_dir(&imports).unwrap();
        fs::create_dir(imports.join("20260616_235959")).unwrap();
        let source = stamp("20260616_235959");
        let allocated = allocate_import_id(root.path(), &source).unwrap();
        assert_eq!(allocated.import_id.as_str(), "20260617_000000");
        assert_eq!(allocated.source_timestamp, source);
    }

    #[test]
    fn bind_reuses_a_matching_hash_and_allocates_for_different_bytes() {
        let root = journal();
        let source = stamp("20260616_120000");
        let first = root.path().join("first.bin");
        let second = root.path().join("second.bin");
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"two").unwrap();
        let bound = bind_import_record(root.path(), &source, Some(&first)).unwrap();
        assert_eq!(bound.import_id.as_str(), "20260616_120000");
        let again = bind_import_record(root.path(), &source, Some(&first)).unwrap();
        assert_eq!(again.import_id, bound.import_id);
        assert_eq!(again.source_timestamp, source);
        let other = bind_import_record(root.path(), &source, Some(&second)).unwrap();
        assert_eq!(other.import_id.as_str(), "20260616_120001");
        assert_eq!(other.source_timestamp, source);
        let prior = fs::read(root.path().join("imports/20260616_120001/import.json")).unwrap();
        let other_again = bind_import_record(root.path(), &source, Some(&second)).unwrap();
        assert_eq!(other_again.import_id, other.import_id);
        assert_eq!(other_again.source_timestamp, source);
        assert_eq!(
            fs::read(root.path().join("imports/20260616_120001/import.json")).unwrap(),
            prior
        );
        assert!(!root.path().join("imports/20260616_120002").exists());
        assert!(fs::read(root.path().join("imports/20260616_120000/import.json")).is_ok());
        let first_meta = read_import_metadata(root.path(), "20260616_120000").unwrap();
        assert_eq!(
            first_meta.get("source_hash").and_then(Value::as_str),
            Some(hash_source(&first).unwrap().as_str())
        );
    }

    #[test]
    fn bind_skips_a_file_occupant_and_keeps_its_bytes() {
        let root = journal();
        let source = stamp("20260616_235959");
        let occupant = root.path().join("imports/20260616_235959");
        fs::create_dir_all(occupant.parent().unwrap()).unwrap();
        fs::write(&occupant, b"keep").unwrap();
        let bytes = root.path().join("payload.bin");
        fs::write(&bytes, b"payload").unwrap();
        let bound = bind_import_record(root.path(), &source, Some(&bytes)).unwrap();
        assert_eq!(bound.import_id.as_str(), "20260617_000000");
        assert_eq!(bound.source_timestamp.as_str(), "20260616_235959");
        assert_eq!(fs::read(&occupant).unwrap(), b"keep");
    }

    #[test]
    fn a_record_without_a_hash_does_not_prove_the_bytes_match() {
        let root = journal();
        let source = stamp("20260616_120000");
        let record = root.path().join("imports/20260616_120000/import.json");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        let original = br#"{"task_id":"20260616_120000","source_hint":"image","attempt":{"attempt_id":"20260616_120000:1","generation":1,"state":"completed"}}"#;
        fs::write(&record, original).unwrap();
        let input = root.path().join("photo.png");
        fs::write(&input, b"new bytes").unwrap();
        let bound = bind_import_record(root.path(), &source, Some(&input)).unwrap();
        assert_eq!(bound.import_id.as_str(), "20260616_120001");
        assert_eq!(bound.source_timestamp, source);
        assert_eq!(fs::read(&record).unwrap(), original);
    }

    #[test]
    fn a_matching_manifest_can_prove_an_unhashed_records_identity() {
        let root = journal();
        let source = stamp("20260616_120000");
        let record = root.path().join("imports/20260616_120000/import.json");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        let original = br#"{"task_id":"20260616_120000","source_hint":"image"}"#;
        fs::write(&record, original).unwrap();
        let input = root.path().join("photo.png");
        fs::write(&input, b"same bytes").unwrap();
        let source_hash = hash_source(&input).unwrap();
        crate::write_manifest(&crate::ManifestWriteRequest {
            journal_root: root.path(),
            import_id: source.as_str(),
            source_type: "image",
            source_hash: &source_hash,
            entry_count: 1,
            days_affected: &[],
            files_created: &[],
            imported_via: "native",
            link_id: None,
            observer_handle: None,
            raw_retention: None,
        })
        .unwrap();
        let bound = bind_import_record(root.path(), &source, Some(&input)).unwrap();
        assert_eq!(bound.import_id, source);
        assert_eq!(bound.source_timestamp, source);
        assert_eq!(fs::read(&record).unwrap(), original);
        assert!(!root.path().join("imports/20260616_120001").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_metadata_does_not_prove_a_matching_record() {
        let root = journal();
        let source = stamp("20260616_120000");
        let directory = root.path().join("imports/20260616_120000");
        fs::create_dir_all(&directory).unwrap();
        let input = root.path().join("photo.png");
        fs::write(&input, b"same bytes").unwrap();
        let target = root.path().join("unrelated.json");
        let original = serde_json::to_vec(&json!({
            "source_hash": hash_source(&input).unwrap().as_str()
        }))
        .unwrap();
        fs::write(&target, &original).unwrap();
        let metadata = directory.join("import.json");
        std::os::unix::fs::symlink(&target, &metadata).unwrap();
        let bound = bind_import_record(root.path(), &source, Some(&input)).unwrap();
        assert_eq!(bound.import_id.as_str(), "20260616_120001");
        assert!(
            fs::symlink_metadata(&metadata)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).unwrap(), original);
    }

    #[test]
    fn an_unrelated_malformed_record_does_not_block_an_occupied_slot() {
        let root = journal();
        let malformed = root.path().join("imports/20260615_120000/import.json");
        fs::create_dir_all(malformed.parent().unwrap()).unwrap();
        fs::write(&malformed, b"{invalid metadata").unwrap();
        let occupant = root.path().join("imports/20260616_120000");
        fs::write(&occupant, b"keep occupant").unwrap();
        let input = root.path().join("photo.png");
        fs::write(&input, b"new bytes").unwrap();
        let source = stamp("20260616_120000");
        let bound = bind_import_record(root.path(), &source, Some(&input)).unwrap();
        assert_eq!(bound.import_id.as_str(), "20260616_120001");
        assert_eq!(fs::read(&malformed).unwrap(), b"{invalid metadata");
        assert_eq!(fs::read(&occupant).unwrap(), b"keep occupant");
    }

    #[test]
    fn malformed_metadata_at_the_requested_id_is_an_untouched_occupant() {
        let root = journal();
        let malformed = root.path().join("imports/20260616_120000/import.json");
        fs::create_dir_all(malformed.parent().unwrap()).unwrap();
        fs::write(&malformed, b"{invalid metadata").unwrap();
        let input = root.path().join("photo.png");
        fs::write(&input, b"new bytes").unwrap();
        let source = stamp("20260616_120000");
        let bound = bind_import_record(root.path(), &source, Some(&input)).unwrap();
        assert_eq!(bound.import_id.as_str(), "20260616_120001");
        assert_eq!(fs::read(&malformed).unwrap(), b"{invalid metadata");
    }

    #[test]
    fn explicit_retry_of_a_shifted_id_does_not_allocate() {
        let root = journal();
        let source = stamp("20260616_235959");
        fs::create_dir_all(root.path().join("imports/20260616_235959")).unwrap();
        fs::write(root.path().join("occupant"), b"occupant").unwrap();
        fs::write(root.path().join("imports/20260616_235959/keep"), b"keep").unwrap();
        let bytes = root.path().join("payload.bin");
        fs::write(&bytes, b"payload").unwrap();
        let bound = bind_import_record(root.path(), &source, Some(&bytes)).unwrap();
        assert_eq!(bound.import_id.as_str(), "20260617_000000");
        assert_eq!(bound.source_timestamp.as_str(), "20260616_235959");
        let retry = bind_import_record(root.path(), &bound.import_id, Some(&bytes)).unwrap();
        assert_eq!(retry.import_id, bound.import_id);
        assert_eq!(retry.source_timestamp.as_str(), "20260616_235959");
        assert_eq!(
            fs::read(root.path().join("imports/20260616_235959/keep")).unwrap(),
            b"keep"
        );
        assert!(!root.path().join("imports/20260617_000001").exists());
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn two_threads_receive_distinct_fresh_ids() {
        let root = journal();
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let barrier = Arc::clone(&barrier);
            let path = root.path().to_path_buf();
            handles.push(thread::spawn(move || {
                barrier.wait();
                allocate_import_id(&path, &stamp("20260616_120000")).unwrap()
            }));
        }
        let ids: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().import_id.as_str().to_owned())
            .collect();
        assert_ne!(ids[0], ids[1]);
    }
}
