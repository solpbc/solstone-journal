// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable append-only text and JSONL writers.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use serde::Serialize;

#[cfg(unix)]
use crate::atomic::{fsync_dir, sync_file};
use crate::errors::AppendError;

#[cfg(windows)]
fn sync_file(file: &fs::File) -> io::Result<()> {
    // `File::sync_all` is `FlushFileBuffers` on Windows.  The existing Unix append contract
    // only requires the record-file sync to succeed; parent-directory sync after a create is
    // best effort there, and Windows has no corresponding directory-handle operation here.
    file.sync_all()
}

/// Append one newline-terminated text record through a single raw write.
///
/// A successful return means exactly one complete record was appended and
/// synced. A returned error can still leave a partial write on disk when the
/// underlying single write reports a short byte count.
#[cfg(any(unix, windows))]
pub fn append_text(path: impl AsRef<Path>, text: &str) -> Result<(), AppendError> {
    let mut contents = Vec::with_capacity(text.len() + 1);
    contents.extend_from_slice(text.as_bytes());
    contents.push(b'\n');
    append_record(path.as_ref(), &contents)
}

/// Serialize and append one newline-terminated JSON record through a single raw write.
///
/// A successful return means exactly one complete record was appended and
/// synced. A returned error can still leave a partial write on disk when the
/// underlying single write reports a short byte count.
pub fn append_jsonl<T: Serialize>(path: impl AsRef<Path>, record: &T) -> Result<(), AppendError> {
    let path = path.as_ref();
    let mut contents = serde_json::to_vec(record)
        .map_err(|source| io_error(path, io::Error::new(io::ErrorKind::InvalidData, source)))?;
    contents.push(b'\n');
    append_record(path, &contents)
}

fn append_record(path: &Path, contents: &[u8]) -> Result<(), AppendError> {
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    #[cfg(unix)]
    let is_new = !path.exists();
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let written = file
        .write(contents)
        .map_err(|source| io_error(path, source))?;
    if written != contents.len() {
        return Err(io_error(
            path,
            io::Error::new(
                io::ErrorKind::WriteZero,
                "append record was only partially written",
            ),
        ));
    }
    sync_file(&file).map_err(|source| io_error(path, source))?;
    #[cfg(unix)]
    if is_new {
        fsync_dir(parent);
    }
    Ok(())
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn io_error(path: &Path, source: io::Error) -> AppendError {
    AppendError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn appends_one_complete_newline_terminated_record_per_call() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_text(&path, "first").unwrap();
        let first_len = fs::metadata(&path).unwrap().len();
        append_jsonl(&path, &serde_json::json!({"second": true})).unwrap();
        let contents = fs::read_to_string(&path).unwrap();

        assert_eq!(first_len, "first\n".len() as u64);
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            vec!["first", r#"{"second":true}"#]
        );
        assert!(contents.ends_with('\n'));
    }

    #[test]
    fn appends_jsonl_as_one_complete_newline_terminated_record_per_call() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_jsonl(&path, &serde_json::json!({"first": true})).unwrap();
        let first_len = fs::metadata(&path).unwrap().len();
        append_jsonl(&path, &serde_json::json!({"second": true})).unwrap();
        let contents = fs::read_to_string(&path).unwrap();

        assert_eq!(first_len, r#"{"first":true}"#.len() as u64 + 1);
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            vec![r#"{"first":true}"#, r#"{"second":true}"#]
        );
        assert!(contents.ends_with('\n'));
    }

    #[test]
    fn appends_empty_text_as_one_complete_newline_terminated_record() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_text(&path, "").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"\n");
    }

    #[test]
    fn appends_nonempty_text_as_one_complete_newline_terminated_record() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_text(&path, "first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first\n");
    }

    #[test]
    fn appends_repeated_text_records_accumulate_in_call_order() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_text(&path, "a").unwrap();
        append_text(&path, "b").unwrap();
        append_text(&path, "c").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"a\nb\nc\n");
    }

    #[test]
    fn appends_text_into_a_missing_nested_parent_directory() {
        let temporary = TempDir::new();
        let path = temporary.path().join("nested/a/b/records.jsonl");
        append_text(&path, "first").unwrap();
        assert!(path.parent().unwrap().is_dir());
        assert_eq!(fs::read(&path).unwrap(), b"first\n");
    }
}
