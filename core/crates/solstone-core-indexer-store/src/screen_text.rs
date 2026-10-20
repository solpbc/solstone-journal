// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only verification and bounded page projection for screen-text sources.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use solstone_core_format::paths::resolve_journal_path;

use crate::StoreError;
use crate::db::sqlite_table_exists;

const PAGE_BYTES: usize = 65_536;

pub struct ScreenTextPage {
    pub text: String,
    pub next_offset: Option<u64>,
}

fn stored_identity(conn: &Connection, rel: &str) -> Result<Option<(String, u64)>, StoreError> {
    if !sqlite_table_exists(conn, "screen_text_source")? {
        return Ok(None);
    }
    let stored: Option<(String, i64)> = conn
        .query_row(
            "SELECT sha256, bytes FROM screen_text_source WHERE path=?",
            [rel],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(stored.and_then(|(digest, bytes)| u64::try_from(bytes).ok().map(|n| (digest, n))))
}

// Read only the indexed length plus one byte, even if the source grows while
// being read. Retain at most one page; hashing never allocates the whole source.
fn digest_and_slice(
    reader: impl Read,
    expected_bytes: u64,
    offset: u64,
    limit: usize,
) -> io::Result<(String, Vec<u8>)> {
    let mut reader = reader.take(expected_bytes.saturating_add(1));
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; PAGE_BYTES];
    let mut total = 0_u64;
    let mut slice = Vec::with_capacity(limit);
    let end = offset.saturating_add(limit as u64);
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        let next = total + count as u64;
        let left = total.max(offset);
        let right = next.min(end);
        if left < right {
            slice.extend_from_slice(&buffer[(left - total) as usize..(right - total) as usize]);
        }
        total = next;
    }
    if total != expected_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source length changed",
        ));
    }
    Ok((format!("{:x}", hash.finalize()), slice))
}

pub fn screen_text_current_digest(
    conn: &Connection,
    journal: &Path,
    rel: &str,
) -> Result<Option<String>, StoreError> {
    let Some((stored_digest, stored_bytes)) = stored_identity(conn, rel)? else {
        return Ok(None);
    };
    let Ok(path) = resolve_journal_path(journal, rel) else {
        return Ok(None);
    };
    let Ok(file) = File::open(path) else {
        return Ok(None);
    };
    let Ok((digest, _)) = digest_and_slice(file, stored_bytes, 0, 0) else {
        return Ok(None);
    };
    Ok((digest == stored_digest).then_some(digest))
}

pub fn screen_text_source_current(
    conn: &Connection,
    journal: &Path,
    rel: &str,
) -> Result<bool, StoreError> {
    Ok(screen_text_current_digest(conn, journal, rel)?.is_some())
}

pub fn screen_text_pinned_bytes_match(
    conn: &Connection,
    rel: &str,
    bytes: &[u8],
    pinned_digest: &str,
) -> Result<bool, StoreError> {
    let Some((stored_digest, stored_bytes)) = stored_identity(conn, rel)? else {
        return Ok(false);
    };
    Ok(bytes.len() as u64 == stored_bytes
        && stored_digest == pinned_digest
        && format!("{:x}", Sha256::digest(bytes)) == pinned_digest)
}

/// Verify the full indexed source while retaining only the requested UTF-8 page.
/// Final release checks must still revalidate lifecycle and current identity.
pub fn screen_text_pinned_page(
    conn: &Connection,
    journal: &Path,
    rel: &str,
    pinned_digest: &str,
    offset: u64,
) -> Result<Option<ScreenTextPage>, StoreError> {
    let Some((stored_digest, stored_bytes)) = stored_identity(conn, rel)? else {
        return Ok(None);
    };
    if stored_digest != pinned_digest
        || offset > stored_bytes
        || (offset == stored_bytes && offset > 0)
    {
        return Ok(None);
    }
    let Ok(path) = resolve_journal_path(journal, rel) else {
        return Ok(None);
    };
    let Ok(file) = File::open(path) else {
        return Ok(None);
    };
    let Ok((digest, bytes)) = digest_and_slice(file, stored_bytes, offset, PAGE_BYTES) else {
        return Ok(None);
    };
    if digest != pinned_digest {
        return Ok(None);
    }
    let valid_len = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(_) => return Ok(None),
    };
    if valid_len == 0 && stored_bytes > 0 {
        return Ok(None);
    }
    let text = std::str::from_utf8(&bytes[..valid_len])
        .expect("validated page prefix")
        .to_owned();
    let next = offset + valid_len as u64;
    Ok(Some(ScreenTextPage {
        text,
        next_offset: (next < stored_bytes).then_some(next),
    }))
}

#[cfg(test)]
mod tests {
    use super::digest_and_slice;
    use sha2::{Digest, Sha256};
    use std::io::{self, Read};

    #[test]
    fn a_growing_source_is_bounded_by_indexed_length_plus_one() {
        struct Endless(usize);
        impl Read for Endless {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                self.0 += out.len();
                out.fill(b'a');
                Ok(out.len())
            }
        }
        let mut source = Endless(0);
        assert!(digest_and_slice(&mut source, 100, 0, 16).is_err());
        assert_eq!(source.0, 101);
    }

    #[test]
    fn projection_hashes_the_source_but_retains_only_the_requested_page() {
        let bytes = b"before selected after";
        let (digest, slice) = digest_and_slice(bytes.as_slice(), bytes.len() as u64, 7, 8).unwrap();
        assert_eq!(digest, format!("{:x}", Sha256::digest(bytes)));
        assert_eq!(slice, b"selected");
        assert!(digest_and_slice(bytes.as_slice(), bytes.len() as u64 + 1, 0, 8).is_err());
    }
}
