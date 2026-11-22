// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{ffi::OsStr, io, path::Path, sync::Arc};

use crate::store::review_candidates::display::{FIELD, MAX_BYTES};
use serde_json::Value;
use solstone_core_journal_io::{JournalRoot, media::MediaDirectory};

pub(in crate::store) const DISPLAY_STAGES: &str = ".face-derived-staging";

/// Candidate writers already hold entity trust. The public Derived and strict
/// review row stay unchanged. A reader of the raw private face file can ignore
/// its optional field, as all older builds do.
pub(in crate::store) fn publish_candidate_display(
    dir: &MediaDirectory,
    field: Option<Value>,
) -> io::Result<()> {
    let current = dir.read_file_bounded(OsStr::new(super::FILE), MAX_BYTES)?;
    let mut value = if let Some(file) = current {
        let value: Value = serde_json::from_slice(&file.bytes)?;
        let known: super::Derived = serde_json::from_value(value.clone())?;
        if known.schema_version != 1 {
            return Ok(());
        }
        value
    } else if field.is_some() {
        serde_json::to_value(super::Derived::default())?
    } else {
        return Ok(());
    };
    let fields = value.as_object_mut().ok_or(io::ErrorKind::InvalidData)?;
    if let Some(field) = field {
        fields.insert(FIELD.to_owned(), field);
    } else {
        fields.remove(FIELD);
    }
    let bytes = serde_json::to_vec(&value)?;
    let root = dir
        .diagnostic_path()
        .parent()
        .ok_or(io::ErrorKind::NotFound)?;
    if value.get(FIELD).is_some() {
        publish_augmented(root, &bytes)
    } else {
        let binding = JournalRoot::open(dir.diagnostic_path()).map_err(io::Error::other)?;
        if binding.identity() != dir.object_identity() {
            return Err(io::Error::other("face directory changed"));
        }
        super::super::storage::publish(dir.diagnostic_path(), &binding, super::FILE, &bytes)
            .map_err(io::Error::other)
    }
}

/// Old face wipes remove the derived record and pending publication files in
/// each child of entities/. Only unpublished data enters this staging folder.
/// An existing identity or any unrelated content makes it ineligible.
pub(super) fn publish_augmented(journal: &Path, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_BYTES {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    let root = Arc::new(JournalRoot::open(journal).map_err(io::Error::other)?);
    let entities = MediaDirectory::open(Arc::clone(&root), Path::new("entities"))?;
    #[cfg(unix)]
    let destination = solstone_core_journal_io::open_flat_directory_bound(
        root.as_ref(),
        OsStr::new("entities"),
        journal,
    );
    #[cfg(windows)]
    let destination = solstone_core_journal_io::open_windows_flat_directory_bound(
        root.as_ref(),
        OsStr::new("entities"),
        journal,
    );
    let destination = destination
        .map_err(io::Error::other)?
        .ok_or(io::ErrorKind::NotFound)?;
    #[cfg(unix)]
    let stages = solstone_core_journal_io::create_or_open_flat_directory_bound(
        &destination,
        OsStr::new(DISPLAY_STAGES),
        0o700,
        entities.diagnostic_path(),
    );
    #[cfg(windows)]
    let stages = solstone_core_journal_io::create_or_open_windows_flat_directory_bound(
        &destination,
        OsStr::new(DISPLAY_STAGES),
        entities.diagnostic_path(),
    );
    let stages = stages.map_err(io::Error::other)?;
    let observed = MediaDirectory::open(root, &Path::new("entities").join(DISPLAY_STAGES))?;
    #[cfg(unix)]
    let identities = (destination.identity(), stages.identity());
    #[cfg(windows)]
    let identities = (destination.object_identity(), stages.object_identity());
    if identities != (entities.object_identity(), observed.object_identity()) {
        return Err(io::Error::other("face publication directory changed"));
    }
    let entries = solstone_core_journal_io::list_dir_entries(observed.diagnostic_path())
        .map_err(io::Error::other)?;
    if entries
        .iter()
        .any(|entry| !solstone_core_journal_io::is_publication_candidate_name(&entry.name))
    {
        return Err(io::Error::other("face publication directory is occupied"));
    }
    let check = || {
        entities
            .revalidate()
            .and_then(|_| observed.revalidate())
            .map_err(|e| e.to_string())
    };
    let outcome = solstone_core_journal_io::atomic_replace_from_bound_stage_checked(
        &destination,
        &stages,
        OsStr::new(super::FILE),
        bytes,
        0o600,
        &check,
    )
    .map_err(io::Error::other)?;
    #[cfg(unix)]
    let confirmed = matches!(
        outcome,
        solstone_core_journal_io::BoundAtomicOutcome::Published { .. }
    );
    #[cfg(windows)]
    let confirmed = matches!(
        outcome,
        solstone_core_journal_io::DetailedAtomicOutcome::Published
    );
    if !confirmed {
        return Err(io::Error::other("face publication was not confirmed"));
    }
    Ok(())
}
