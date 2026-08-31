// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

/// Maximum text size accepted by bounded journal reads.
pub const MAX_BYTES: u64 = 16_384;

/// Failure while resolving or reading bounded journal text.
#[derive(Debug, PartialEq, Eq)]
pub enum JournalReadError {
    Path(String),
    TooLarge(String),
    Encoding(String),
    NotFound,
    Io,
}

/// Read one UTF-8 journal file within the fixed byte limit.
pub fn read_text(journal_root: &Path, rel: &str) -> Result<String, JournalReadError> {
    let path = resolve_read_path(journal_root, rel)?;
    let metadata = fs::metadata(&path).map_err(map_io)?;
    if metadata.len() > MAX_BYTES {
        return Err(JournalReadError::TooLarge(
            "file exceeds the 16,384-byte read limit".to_owned(),
        ));
    }
    let bytes = fs::read(path).map_err(map_io)?;
    String::from_utf8(bytes)
        .map_err(|_| JournalReadError::Encoding("file is not valid UTF-8".into()))
}

/// Resolve one bounded journal-relative file without following an escape.
pub fn resolve_read_path(journal_root: &Path, rel: &str) -> Result<PathBuf, JournalReadError> {
    if percent_decode_changes(rel) {
        return Err(JournalReadError::Path(
            "rel must not contain percent-encoded components".into(),
        ));
    }
    let candidate = crate::resolve_journal_path(journal_root, rel).map_err(|_| {
        JournalReadError::Path(if rel.trim().is_empty() {
            "rel must not be empty".into()
        } else if Path::new(rel).is_absolute() {
            "rel must be a relative path".into()
        } else if rel.contains('\\') {
            "rel must use forward slashes".into()
        } else {
            "rel must not contain empty, '.', or '..' components".into()
        })
    })?;
    let metadata = fs::metadata(&candidate).map_err(map_io)?;
    if !metadata.is_file() {
        return Err(JournalReadError::NotFound);
    }
    let root = fs::canonicalize(journal_root).map_err(map_io)?;
    let resolved = fs::canonicalize(candidate).map_err(map_io)?;
    if !resolved.starts_with(&root) {
        return Err(JournalReadError::Path(
            "rel must resolve within the journal".into(),
        ));
    }
    Ok(resolved)
}

fn percent_decode_changes(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes
        .windows(3)
        .any(|part| part[0] == b'%' && part[1].is_ascii_hexdigit() && part[2].is_ascii_hexdigit())
}

fn map_io(error: std::io::Error) -> JournalReadError {
    if error.kind() == std::io::ErrorKind::NotFound {
        JournalReadError::NotFound
    } else {
        JournalReadError::Io
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{JournalReadError, MAX_BYTES, read_text};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn root() -> PathBuf {
        let path = PathBuf::from("/var/tmp").join(format!(
            "solstone-journal-io-bounded-read-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn bounded_reader_rejects_encoded_paths_and_oversized_files() {
        let root = root();
        fs::write(root.join("note.txt"), "ok").unwrap();
        assert_eq!(read_text(&root, "note.txt").unwrap(), "ok");
        assert!(matches!(
            read_text(&root, "%2e%2e/note.txt"),
            Err(JournalReadError::Path(_))
        ));
        fs::write(root.join("large.txt"), vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            read_text(&root, "large.txt"),
            Err(JournalReadError::TooLarge(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
