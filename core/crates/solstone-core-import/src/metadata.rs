// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Open import metadata records.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, path_lexists};

use crate::{ImportError, OrderedMetadata};

/// Ordered JSON object stored in `imports/<id>/import.json`.
pub type ImportMetadata = OrderedMetadata;

/// Read a complete open import metadata record.
pub fn read_import_metadata(
    journal_root: &Path,
    import_id: &str,
) -> Result<ImportMetadata, ImportError> {
    let path = import_metadata_path(journal_root, import_id)?;
    let bytes = fs::read(&path).map_err(|error| ImportError::MetadataCorrupt {
        path: path.clone(),
        message: error.to_string(),
    })?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|error| ImportError::MetadataCorrupt {
            path: path.clone(),
            message: error.to_string(),
        })?;
    let Value::Object(metadata) = value else {
        return Err(ImportError::MetadataCorrupt {
            path,
            message: "import metadata must be a JSON object".to_owned(),
        });
    };
    Ok(metadata)
}

/// Read provenance when metadata exists, treating only absence as no provenance.
pub fn read_provenance(
    journal_root: &Path,
    import_id: &str,
) -> Result<Option<ImportMetadata>, ImportError> {
    let path = import_metadata_path(journal_root, import_id)?;
    if !path_lexists(&path).map_err(|error| ImportError::PathResolution {
        path: path.clone(),
        message: error.to_string(),
    })? {
        return Ok(None);
    }
    read_import_metadata(journal_root, import_id).map(Some)
}

/// Atomically write a complete ordered import metadata record.
pub fn write_import_metadata(
    journal_root: &Path,
    import_id: &str,
    metadata: &ImportMetadata,
) -> Result<PathBuf, ImportError> {
    let import_dir = crate::staging::ensure_import_private_chain(journal_root, import_id)?;
    let path = import_dir.join("import.json");
    let bytes =
        serde_json::to_vec_pretty(metadata).map_err(|error| ImportError::MetadataWriteFailed {
            path: path.clone(),
            message: error.to_string(),
        })?;
    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) }).map_err(|error| {
        ImportError::MetadataWriteFailed {
            path: path.clone(),
            message: error.to_string(),
        }
    })?;
    Ok(path)
}

pub(crate) fn import_metadata_path(
    journal_root: &Path,
    import_id: &str,
) -> Result<PathBuf, ImportError> {
    Ok(crate::staging::import_directory(journal_root, import_id)?.join("import.json"))
}
