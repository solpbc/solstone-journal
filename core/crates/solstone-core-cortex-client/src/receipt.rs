// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict completed receipts for recovery. The ordinary event reader tolerates
//! damaged rows for display; an execution proof may not discard any row.

use serde_json::Value;
#[cfg(unix)]
use solstone_core_journal_io::{
    FlatDirectory as Directory, open_flat_directory_bound as open,
    read_observed_file_bounded as read,
};
use solstone_core_journal_io::{
    JournalRoot, check_portable_component, cortex_use::talent_directory_name,
};
#[cfg(windows)]
use solstone_core_journal_io::{
    WindowsFlatDirectory as Directory, open_windows_flat_directory_bound as open,
    read_windows_observed_file_bounded as read,
};
use std::{io, path::Path};

fn acquire(root: &JournalRoot, name: &str) -> io::Result<Directory> {
    root.revalidate().map_err(io::Error::other)?;
    let talents = open(root, "talents".as_ref(), root.canonical_path())
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("talent receipts disappeared"))?;
    open(
        &talents,
        talent_directory_name(name).as_ref(),
        &root.canonical_path().join("talents"),
    )
    .map_err(io::Error::other)?
    .ok_or_else(|| io::Error::other("talent receipt directory disappeared"))
}

fn identity(directory: &Directory) -> solstone_core_journal_io::ObjectIdentity {
    #[cfg(unix)]
    {
        directory.identity()
    }
    #[cfg(windows)]
    {
        directory.object_identity()
    }
}

/// Read one exact completed use, refusing aliases, races, oversized data and
/// malformed or unterminated rows. No active-file fallback proves completion.
pub fn read_completed_use_events(
    journal: &Path,
    name: &str,
    use_id: &str,
) -> io::Result<Vec<Value>> {
    check_portable_component(use_id)
        .map_err(|e| io::Error::other(format!("invalid receipt use: {e:?}")))?;
    let root = JournalRoot::open(journal).map_err(io::Error::other)?;
    let directory = acquire(&root, name)?;
    let leaf = format!("{use_id}.jsonl");
    let observed = read(&directory, leaf.as_ref(), 4 * 1024 * 1024)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("completed use disappeared"))?;
    let rebound = acquire(&root, name)?;
    let current = read(&rebound, leaf.as_ref(), 4 * 1024 * 1024)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("completed use disappeared"))?;
    if identity(&directory) != identity(&rebound)
        || current.entry != observed.entry
        || current.bytes != observed.bytes
    {
        return Err(io::Error::other("completed use changed during observation"));
    }
    root.revalidate().map_err(io::Error::other)?;
    if !observed.bytes.ends_with(b"\n") {
        return Err(io::Error::other("unterminated completed receipt"));
    }
    let events = observed.bytes[..observed.bytes.len() - 1]
        .split(|byte| *byte == b'\n')
        .map(serde_json::from_slice::<Value>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(io::Error::other)?;
    if events.iter().any(|event| !event.is_object())
        || events.first().is_none_or(|request| {
            request["name"] != name || request["use_id"] != use_id || request.get("event").is_some()
        })
    {
        return Err(io::Error::other("completed use request identity changed"));
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn strict_receipt_refuses_damaged_rows_and_changed_identity() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("talents/screen");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("u.jsonl");
        let valid = b"{\"name\":\"screen\",\"use_id\":\"u\"}\n{\"event\":\"start\"}\n{\"event\":\"finish\"}\n";
        fs::write(&path, valid).unwrap();
        assert_eq!(
            read_completed_use_events(root.path(), "screen", "u")
                .unwrap()
                .len(),
            3
        );
        for bad in [
            valid[..valid.len() - 1].to_vec(),
            [valid.as_slice(), b"malformed\n"].concat(),
            [valid.as_slice(), b"\n"].concat(),
            [valid.as_slice(), b"null\n"].concat(),
        ] {
            fs::write(&path, bad).unwrap();
            assert!(read_completed_use_events(root.path(), "screen", "u").is_err());
        }
        fs::write(&path, b"{\"name\":\"other\",\"use_id\":\"u\"}\n").unwrap();
        assert!(read_completed_use_events(root.path(), "screen", "u").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn strict_receipt_refuses_file_and_parent_aliases() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let bytes = b"{\"name\":\"screen\",\"use_id\":\"u\"}\n";
        fs::write(outside.path().join("u.jsonl"), bytes).unwrap();
        fs::create_dir(root.path().join("talents")).unwrap();
        let directory = root.path().join("talents/screen");
        symlink(outside.path(), &directory).unwrap();
        assert!(read_completed_use_events(root.path(), "screen", "u").is_err());
        fs::remove_file(&directory).unwrap();
        fs::create_dir(&directory).unwrap();
        symlink(outside.path().join("u.jsonl"), directory.join("u.jsonl")).unwrap();
        assert!(read_completed_use_events(root.path(), "screen", "u").is_err());
        assert_eq!(fs::read(outside.path().join("u.jsonl")).unwrap(), bytes);
    }
}
