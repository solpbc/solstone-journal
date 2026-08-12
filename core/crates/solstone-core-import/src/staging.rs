// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Non-destructive import staging.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use solstone_core_journal_io::{
    AtomicWriteOptions, create_directory_with_mode, path_lexists, realpath_non_strict,
    write_reader_exclusive,
};

use crate::dedupe::build_import_inventory;
use crate::metadata::{ImportMetadata, read_provenance, write_import_metadata};
use crate::{AuditSinkError, ForceReimportAudit, ImportError, ImportForceEffects, RemovalError};

/// Resolved relationship between an owner source and journal imports.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SourceLocation {
    External { source: PathBuf },
    AlreadyInImports { source: PathBuf },
}

/// Inputs for one staging operation.
pub struct StageRequest<'a> {
    pub journal_root: &'a Path,
    pub import_id: &'a str,
    pub source: &'a Path,
    pub destination_name: &'a OsStr,
    pub metadata: &'a ImportMetadata,
    pub force: bool,
    pub dry_run: bool,
    pub days_affected: &'a [&'a str],
}

/// Result of staging, reuse, or preview.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StageOutcome {
    pub path: PathBuf,
    pub source_location: SourceLocation,
    pub disposition: StageDisposition,
    pub force_audit_recorded: bool,
}

/// Whether source material was written, reused, or previewed.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StageDisposition {
    Staged,
    AlreadyStaged,
    Preview,
}

/// Resolve a source and classify it against the resolved imports directory.
pub fn classify_source_location(
    journal_root: &Path,
    source: &Path,
) -> Result<SourceLocation, ImportError> {
    let resolved_source = resolve(source)?;
    let imports = resolve(&journal_root.join("imports"))?;
    if resolved_source.starts_with(&imports) {
        Ok(SourceLocation::AlreadyInImports {
            source: resolved_source,
        })
    } else {
        Ok(SourceLocation::External {
            source: resolved_source,
        })
    }
}

/// Stage one external source without replacing existing destination bytes.
pub fn stage_source(
    request: &StageRequest<'_>,
    effects: &dyn ImportForceEffects,
) -> Result<StageOutcome, ImportError> {
    let source_location = classify_source_location(request.journal_root, request.source)?;
    if let SourceLocation::AlreadyInImports { source } = &source_location {
        return Ok(StageOutcome {
            path: source.clone(),
            source_location,
            disposition: StageDisposition::AlreadyStaged,
            force_audit_recorded: false,
        });
    }

    let import_dir = import_directory(request.journal_root, request.import_id)?;
    let mut force_audit_recorded = false;
    let exists =
        path_lexists(&import_dir).map_err(|error| path_error(&import_dir, error.to_string()))?;
    if exists {
        if request.dry_run && !request.force {
            return Ok(preview(&import_dir, source_location, false));
        }
        if !request.force {
            return Err(ImportError::ExistingImportDirectory { path: import_dir });
        }
        if fs::symlink_metadata(&import_dir)
            .map_err(|error| path_error(&import_dir, error.to_string()))?
            .file_type()
            .is_symlink()
        {
            return Err(ImportError::ImportDirectoryIsSymlink { path: import_dir });
        }
        verify_force_containment(request.journal_root, &import_dir)?;
        verify_force_metadata(request, &import_dir)?;
        let inventory = build_import_inventory(&import_dir)?;
        effects
            .append_force_reimport(&ForceReimportAudit {
                import_dir: import_dir.clone(),
                inventory,
                days_affected: request
                    .days_affected
                    .iter()
                    .map(|day| (*day).to_owned())
                    .collect(),
                dry_run: request.dry_run,
            })
            .map_err(audit_error)?;
        force_audit_recorded = true;
        if request.dry_run {
            return Ok(preview(&import_dir, source_location, true));
        }
        effects
            .remove_import_directory(&import_dir)
            .map_err(|error| removal_error(&import_dir, error))?;
    }

    let source = match &source_location {
        SourceLocation::External { source } => source,
        SourceLocation::AlreadyInImports { .. } => {
            unreachable!("already-staged sources return above")
        }
    };
    let source_metadata = fs::metadata(source).map_err(|error| source_error(source, error))?;
    if !source_metadata.is_file() {
        return Err(ImportError::SourceNotFile {
            path: source.clone(),
        });
    }
    let import_dir = import_directory(request.journal_root, request.import_id)?;
    let destination = import_destination(&import_dir, request.destination_name)?;
    if resolve(source)? == resolve(&destination)? {
        return Ok(StageOutcome {
            path: destination,
            source_location,
            disposition: StageDisposition::AlreadyStaged,
            force_audit_recorded,
        });
    }
    if request.dry_run {
        return Ok(preview(&destination, source_location, force_audit_recorded));
    }
    ensure_import_private_chain(request.journal_root, request.import_id)?;
    let mut reader = File::open(source).map_err(|error| source_error(source, error))?;
    write_reader_exclusive(
        &destination,
        &mut reader,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| promotion_error(&destination, error.to_string()))?;
    write_import_metadata(request.journal_root, request.import_id, request.metadata)?;
    Ok(StageOutcome {
        path: destination,
        source_location,
        disposition: StageDisposition::Staged,
        force_audit_recorded,
    })
}

/// Move an import directory within `imports/` and repair its privacy mode.
pub fn relocate_import(
    journal_root: &Path,
    from_import_id: &str,
    to_import_id: &str,
) -> Result<PathBuf, ImportError> {
    let source = import_directory(journal_root, from_import_id)?;
    let destination = import_directory(journal_root, to_import_id)?;
    if !path_lexists(&source).map_err(|error| path_error(&source, error.to_string()))? {
        return Err(ImportError::SourceMissing { path: source });
    }
    if fs::symlink_metadata(&source)
        .map_err(|error| path_error(&source, error.to_string()))?
        .file_type()
        .is_symlink()
    {
        return Err(ImportError::ImportDirectoryIsSymlink { path: source });
    }
    if path_lexists(&destination).map_err(|error| path_error(&destination, error.to_string()))? {
        return Err(ImportError::DestinationExists { path: destination });
    }
    fs::rename(&source, &destination).map_err(|error| ImportError::RelocationFailed {
        path: destination.clone(),
        message: error.to_string(),
    })?;
    ensure_import_private_chain(journal_root, to_import_id)
}

pub(crate) fn import_directory(
    journal_root: &Path,
    import_id: &str,
) -> Result<PathBuf, ImportError> {
    if !matches!(
        Path::new(import_id).components().next(),
        Some(Component::Normal(_))
    ) || Path::new(import_id).components().count() != 1
    {
        return Err(ImportError::InvalidImportId {
            import_id: import_id.to_owned(),
        });
    }
    Ok(journal_root.join("imports").join(import_id))
}

pub(crate) fn ensure_import_private_chain(
    journal_root: &Path,
    import_id: &str,
) -> Result<PathBuf, ImportError> {
    let imports = journal_root.join("imports");
    let imports_is_symlink = path_lexists(&imports)
        .map_err(|error| path_error(&imports, error.to_string()))?
        && fs::symlink_metadata(&imports)
            .map_err(|error| path_error(&imports, error.to_string()))?
            .file_type()
            .is_symlink();
    if !imports_is_symlink {
        create_directory_with_mode(&imports, 0o700)
            .map_err(|error| path_error(&imports, error.to_string()))?;
    }
    let import_dir = import_directory(journal_root, import_id)?;
    create_directory_with_mode(&import_dir, 0o700)
        .map_err(|error| path_error(&import_dir, error.to_string()))?;
    Ok(import_dir)
}

fn import_destination(import_dir: &Path, destination_name: &OsStr) -> Result<PathBuf, ImportError> {
    let candidate = Path::new(destination_name);
    if !matches!(candidate.components().next(), Some(Component::Normal(_)))
        || candidate.components().count() != 1
    {
        return Err(ImportError::InvalidDestinationName {
            name: destination_name.to_owned(),
        });
    }
    Ok(import_dir.join(destination_name))
}

fn verify_force_metadata(request: &StageRequest<'_>, import_dir: &Path) -> Result<(), ImportError> {
    let Some(existing) = read_provenance(request.journal_root, request.import_id)? else {
        return Ok(());
    };
    for key in ["source_hash", "client_item_id", "task_id"] {
        if existing.get(key) != request.metadata.get(key) {
            return Err(ImportError::MetadataMismatchOnForce {
                path: import_dir.to_path_buf(),
                key,
            });
        }
    }
    Ok(())
}

fn verify_force_containment(journal_root: &Path, import_dir: &Path) -> Result<(), ImportError> {
    let imports = resolve(&journal_root.join("imports"))?;
    let resolved_import_dir = resolve(import_dir)?;
    // These path observations cannot close a replacement race; that needs dirfd/O_NOFOLLOW handling.
    if resolved_import_dir == imports || !resolved_import_dir.starts_with(&imports) {
        return Err(ImportError::ImportDirectoryEscapesImports {
            path: resolved_import_dir,
            imports,
        });
    }
    Ok(())
}

fn preview(
    path: &Path,
    source_location: SourceLocation,
    force_audit_recorded: bool,
) -> StageOutcome {
    StageOutcome {
        path: path.to_path_buf(),
        source_location,
        disposition: StageDisposition::Preview,
        force_audit_recorded,
    }
}

fn resolve(path: &Path) -> Result<PathBuf, ImportError> {
    realpath_non_strict(path).map_err(|error| path_error(path, error.to_string()))
}

fn path_error(path: &Path, message: String) -> ImportError {
    ImportError::PathResolution {
        path: path.to_path_buf(),
        message,
    }
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

fn promotion_error(path: &Path, message: String) -> ImportError {
    ImportError::PromotionFailed {
        path: path.to_path_buf(),
        message,
    }
}

fn audit_error(error: AuditSinkError) -> ImportError {
    ImportError::AuditSinkFailed {
        message: error.message,
    }
}

fn removal_error(path: &Path, error: RemovalError) -> ImportError {
    ImportError::RemovalFailed {
        path: path.to_path_buf(),
        message: error.message,
    }
}
