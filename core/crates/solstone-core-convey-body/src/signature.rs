// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The database and WAL metadata used to invalidate Body-derived caches.
pub type TrendsSignature = (u128, u64, u128, u64);

#[derive(Debug)]
pub enum DatabaseSignatureError {
    Io { path: PathBuf, source: io::Error },
}

impl std::fmt::Display for DatabaseSignatureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "could not stat {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for DatabaseSignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
        }
    }
}

pub(crate) fn health_dedupe_database_path(journal_root: &Path) -> PathBuf {
    journal_root.join("imports/health-dedupe.sqlite")
}

pub(crate) fn read_database_signature(
    path: &Path,
) -> Result<Option<TrendsSignature>, DatabaseSignatureError> {
    let database = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DatabaseSignatureError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let database_mtime = modified_nanos(path, &database)?;
    let wal_path = path.with_file_name(format!(
        "{}-wal",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    ));
    let (wal_mtime, wal_size) = match fs::metadata(&wal_path) {
        Ok(metadata) => (modified_nanos(&wal_path, &metadata)?, metadata.len()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => (0, 0),
        Err(source) => {
            return Err(DatabaseSignatureError::Io {
                path: wal_path,
                source,
            });
        }
    };
    Ok(Some((database_mtime, database.len(), wal_mtime, wal_size)))
}

/// Reads the Python-compatible trends cache signature, including its no-import sentinel.
///
/// The forthcoming day-page wave uses this shared signature for its baseline lookup.
pub fn trends_signature(
    journal_root: impl AsRef<Path>,
) -> Result<TrendsSignature, DatabaseSignatureError> {
    Ok(
        read_database_signature(&health_dedupe_database_path(journal_root.as_ref()))?
            .unwrap_or((0, 0, 0, 0)),
    )
}

fn modified_nanos(path: &Path, metadata: &fs::Metadata) -> Result<u128, DatabaseSignatureError> {
    metadata
        .modified()
        .map_err(|source| DatabaseSignatureError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(|source| DatabaseSignatureError::Io {
            path: path.to_path_buf(),
            source: io::Error::other(source),
        })
}
