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

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// Serialize and append one newline-terminated JSON record through a single raw write,
/// refusing symlink traversal and refusing to create missing parent directories.
#[cfg(any(unix, windows))]
pub fn append_jsonl_no_follow<T: Serialize>(
    path: impl AsRef<Path>,
    record: &T,
) -> Result<(), AppendError> {
    let path = path.as_ref();
    let mut contents = serde_json::to_vec(record)
        .map_err(|source| io_error(path, io::Error::new(io::ErrorKind::InvalidData, source)))?;
    contents.push(b'\n');
    append_record_no_follow(path, &contents)
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new()
        .append(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, OPEN_ALWAYS, SYNCHRONIZE,
    };
    let file = crate::locking::open_windows_path(
        path,
        FILE_APPEND_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        OPEN_ALWAYS,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
    )?;
    let attributes = crate::locking::attribute_tag_windows(&file)?;
    if crate::locking::is_reparse_point_windows(attributes) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to append through a Windows reparse point",
        ));
    }
    Ok(file)
}

fn write_and_sync(path: &Path, mut file: fs::File, contents: &[u8]) -> Result<(), AppendError> {
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
    Ok(())
}

fn append_record_no_follow(path: &Path, contents: &[u8]) -> Result<(), AppendError> {
    // A newly created file's directory entry is made durable too, as `append_record`
    // does. `symlink_metadata` does not follow the leaf, so a link is not "new".
    #[cfg(unix)]
    let is_new = matches!(
        fs::symlink_metadata(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound
    );
    let file = open_no_follow(path).map_err(|source| io_error(path, source))?;
    write_and_sync(path, file, contents)?;
    #[cfg(unix)]
    if is_new {
        fsync_dir(parent_dir(path));
    }
    Ok(())
}

fn append_record(path: &Path, contents: &[u8]) -> Result<(), AppendError> {
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    #[cfg(unix)]
    let is_new = !path.exists();
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    write_and_sync(path, file, contents)?;
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

    #[test]
    fn append_jsonl_no_follow_appends_records_in_order() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_jsonl_no_follow(&path, &serde_json::json!({"first": true})).unwrap();
        append_jsonl_no_follow(&path, &serde_json::json!({"second": 2})).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            vec![r#"{"first":true}"#, r#"{"second":2}"#]
        );
        assert!(contents.ends_with('\n'));
    }

    #[cfg(unix)]
    #[test]
    fn append_jsonl_no_follow_refuses_symlinked_leaf_on_unix() {
        let temporary = TempDir::new();
        let target = temporary.path().join("target.jsonl");
        let link = temporary.path().join("link.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let error =
            append_jsonl_no_follow(&link, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(!target.exists() || fs::read(&target).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn append_jsonl_no_follow_refuses_missing_parent_and_creates_nothing_on_unix() {
        let temporary = TempDir::new();
        let missing_parent = temporary.path().join("missing_dir");
        let path = missing_parent.join("records.jsonl");

        let error =
            append_jsonl_no_follow(&path, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(!missing_parent.exists());
        assert!(!path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn append_jsonl_no_follow_accumulates_records_in_order_on_windows() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_jsonl_no_follow(&path, &serde_json::json!({"first": 1})).unwrap();
        append_jsonl_no_follow(&path, &serde_json::json!({"second": 2})).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            vec![r#"{"first":1}"#, r#"{"second":2}"#]
        );
        assert!(contents.ends_with('\n'));
    }

    #[cfg(windows)]
    #[test]
    fn append_jsonl_no_follow_refuses_symlinked_leaf_on_windows() {
        let temporary = TempDir::new();
        let target = temporary.path().join("target.jsonl");
        fs::write(&target, b"").unwrap();
        let link = temporary.path().join("link.jsonl");
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            return;
        }

        let error =
            append_jsonl_no_follow(&link, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(fs::read(&target).unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn append_jsonl_no_follow_refuses_dangling_symlink_on_windows() {
        let temporary = TempDir::new();
        let target = temporary.path().join("nonexistent_target.jsonl");
        let link = temporary.path().join("link.jsonl");
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            return;
        }

        let error =
            append_jsonl_no_follow(&link, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(!target.exists());
    }

    #[cfg(windows)]
    #[test]
    fn append_jsonl_no_follow_refuses_missing_parent_and_creates_nothing_on_windows() {
        let temporary = TempDir::new();
        let missing_parent = temporary.path().join("missing_dir");
        let path = missing_parent.join("records.jsonl");

        let error =
            append_jsonl_no_follow(&path, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(!missing_parent.exists());
        assert!(!path.exists());
    }
}
