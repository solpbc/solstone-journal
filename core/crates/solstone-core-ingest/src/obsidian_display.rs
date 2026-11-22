// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::path::Path;

use solstone_core_format::obsidian::NOTE_TRANSCRIPT_FILE;
use solstone_core_format::stored_markdown::{Document, NOTE_RECEIPT_DIR, NOTE_RECEIPT_FILE};
use solstone_core_segment::output_attempt::OutputClaims;

/// Attach display provenance only while the newly published transcript is still current.
pub(crate) fn save(segment: &Path, document: &Document) -> io::Result<()> {
    let Some(bytes) = document.display_receipt_bytes()? else {
        return Ok(());
    };
    let transcript = segment.join(NOTE_TRANSCRIPT_FILE);
    let claims = OutputClaims::acquire([transcript.with_extension("jsonl")], Default::default())?;
    let commit = claims.commit(segment)?;
    let current =
        solstone_core_journal_io::read_regular_file_capped(&transcript, document.len() as u64)
            .map_err(io::Error::other)?;
    if current != document.as_bytes() {
        return Ok(());
    }
    publish(segment, &bytes, &|| commit.revalidate())
}

#[cfg(unix)]
fn publish(segment: &Path, bytes: &[u8], validate: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
    use solstone_core_journal_io::{
        JournalRoot, atomic::atomic_replace_bound_checked, create_or_open_flat_directory_bound,
    };
    use std::ffi::OsStr;
    let parent = JournalRoot::open(segment).map_err(io::Error::other)?;
    let directory =
        create_or_open_flat_directory_bound(&parent, OsStr::new(NOTE_RECEIPT_DIR), 0o700, segment)
            .map_err(io::Error::other)?;
    atomic_replace_bound_checked(
        &directory,
        OsStr::new(NOTE_RECEIPT_FILE),
        bytes,
        0o600,
        &|| {
            validate().map_err(|error| error.to_string())?;
            parent
                .revalidate_canonical_binding()
                .map_err(|error| error.to_string())?;
            directory.revalidate().map_err(|error| error.to_string())
        },
    )
    .map_err(io::Error::other)?;
    Ok(())
}

#[cfg(windows)]
fn publish(segment: &Path, bytes: &[u8], validate: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
    use solstone_core_journal_io::{
        AtomicWriteOptions, JournalRoot, atomic_replace_checked, create_directory_with_mode,
    };
    let directory = segment.join(NOTE_RECEIPT_DIR);
    create_directory_with_mode(&directory, 0o700).map_err(io::Error::other)?;
    let retained = JournalRoot::open(&directory).map_err(io::Error::other)?;
    atomic_replace_checked(
        directory.join(NOTE_RECEIPT_FILE),
        bytes,
        AtomicWriteOptions { mode: Some(0o600) },
        &|| {
            validate().map_err(|error| error.to_string())?;
            retained
                .revalidate_canonical_binding()
                .map_err(|error| error.to_string())
        },
    )
    .map_err(io::Error::other)
}
