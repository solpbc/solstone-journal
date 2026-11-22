// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::reconcile_io;
use solstone_core_format::stored_markdown::{Document, MAX_RECEIPT, day_projection_receipt};
use solstone_core_journal_io::{JournalRoot, LockOptions};
use std::{fs, io, io::Read, path::Path, sync::Arc};

/// Serialize the saved document and its provenance. An older index or reader
/// can continue to use the original Markdown without opening the metadata.
pub(crate) fn publish(
    journal: &Path,
    path: &Path,
    document: Option<&Document>,
    before_replace: impl FnOnce() -> io::Result<()>,
) -> io::Result<bool> {
    let receipt = day_projection_receipt(journal, path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid day document path"))?;
    if document.is_none() && !path.parent().is_some_and(Path::is_dir) {
        return Ok(false);
    }
    let _guard = reconcile_io::hold_lock(
        path,
        LockOptions {
            mode: Some(0o600),
            ..Default::default()
        },
        false,
    )?;
    // A hide removes the canonical document even when optional metadata is
    // unreadable. A receipt cannot render a missing source.
    let changed = match document {
        Some(doc) => !fs::read(path).is_ok_and(|saved| saved == doc.as_bytes()),
        None => reconcile_io::remove(path)?,
    };
    let next = document.and_then(|doc| match doc.display_receipt_bytes() {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!("day document display metadata unavailable: {error}");
            None
        }
    });
    let mut would_reuse_old_receipt = false;
    let mut prepare = || -> io::Result<_> {
        let root = Arc::new(JournalRoot::open(journal).map_err(io::Error::other)?);
        let relative = receipt.strip_prefix(journal).map_err(io::Error::other)?;
        let receipt_path = root.canonical_path().join(relative);
        let publish_next = reconcile_io::bound_metadata(Arc::clone(&root), || {
            let previous = match previous_receipt(&receipt_path) {
                Ok(previous) => previous,
                Err(_) => {
                    reconcile_io::clear_receipt(&receipt_path)?;
                    return Ok(true);
                }
            };
            if previous == next {
                return Ok(false);
            }
            if let Some(old) = previous.as_deref() {
                // An older identity can also match a returning document after
                // an intervening hide or manual change to the derived file.
                if let Some(doc) = document {
                    use sha2::{Digest, Sha256};
                    let digest = format!("{:x}", Sha256::digest(doc.as_bytes()));
                    would_reuse_old_receipt = serde_json::from_slice::<serde_json::Value>(old)
                        .ok()
                        .is_some_and(|row| row["source_sha256"].as_str() == Some(&digest));
                }
                reconcile_io::remove(&receipt_path)?;
            }
            // An unchanged old body with no provenance stays historical.
            Ok(changed || previous.is_some())
        })?;
        Ok((root, receipt_path, publish_next))
    };
    let prepared = match prepare() {
        Ok(metadata) => Some(metadata),
        Err(error) if would_reuse_old_receipt => {
            // Neither a superseded visible day nor a different attribution for
            // identical words may survive. This derived document can be
            // rebuilt once its old receipt can be retired.
            reconcile_io::remove(path)?;
            if let Ok(relative) = path.strip_prefix(journal.join("chronicle")) {
                reconcile_io::enqueue(journal, relative)?;
            }
            return Err(error);
        }
        Err(error) if changed || document.is_none() => {
            log::warn!("day document display metadata unavailable: {error}");
            None
        }
        Err(error) => return Err(error),
    };
    if changed && let Some(doc) = document {
        before_replace()?;
        reconcile_io::replace(path, doc.as_bytes())?;
    }
    if let Some((root, receipt_path, true)) = prepared
        && let Some(bytes) = next
        && let Err(error) =
            reconcile_io::bound_metadata(root, || reconcile_io::replace(&receipt_path, &bytes))
    {
        log::warn!("day document display metadata unavailable: {error}");
    }
    reconcile_io::checkpoint()?;
    Ok(changed)
}

fn previous_receipt(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(stat) if stat.is_file() && stat.len() <= MAX_RECEIPT as u64 => {}
        Ok(_) => return Ok(None),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    }
    let mut bytes = Vec::new();
    reconcile_io::open_file(path)?
        .take(MAX_RECEIPT as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok((!bytes.is_empty() && bytes.len() <= MAX_RECEIPT).then_some(bytes))
}
