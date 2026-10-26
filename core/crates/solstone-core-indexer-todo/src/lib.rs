// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Producers publish create-only hints. Only a drain holds claims or writes the index.

pub mod producers;

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_format::paths::{relative_to_journal, resolve_journal_path};
use solstone_core_journal_io::atomic::{AtomicWriteOptions, atomic_replace, write_bytes_exclusive};
use solstone_core_journal_io::durability::{ArtifactId, DurableRead, read_json_durable};
use solstone_core_journal_io::errors::AtomicWriteError;
use solstone_core_journal_io::paths::realpath_non_strict;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Token {
    ScanFull,
    Reclassify,
    EntitySearch,
}

impl Token {
    fn key(self) -> &'static str {
        match self {
            Self::ScanFull => "@scan-full",
            Self::Reclassify => "@reclassify",
            Self::EntitySearch => "@entity-search",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub key: String,
    pub kind: String,
    pub enqueued_ms: u64,
    pub attempts: u32,
    pub not_before_ms: u64,
}

impl Entry {
    pub fn rank(&self) -> Option<u8> {
        match (self.kind.as_str(), self.key.as_str()) {
            ("path", key) if !key.starts_with('@') && valid_key(key) => Some(0),
            ("scan-full", "@scan-full") => Some(1),
            ("reclassify", "@reclassify") => Some(2),
            ("entity-search", "@entity-search") => Some(3),
            _ => None,
        }
    }

    pub fn filename(&self) -> String {
        format!("{:x}.json", Sha256::digest(self.key.as_bytes()))
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && !key.contains('\\')
        && !key
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        && Path::new(key)
            .components()
            .all(|p| matches!(p, Component::Normal(_)))
}

fn path_key(journal: &Path, path: &Path) -> io::Result<String> {
    let key = if path.is_absolute() || path.starts_with(journal) {
        relative_to_journal(journal, path).or_else(|| resolved_relative(journal, path))
    } else {
        path.to_str().map(str::to_owned)
    }
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "index path is outside the journal or is not UTF-8",
        )
    })?;
    let key = key.strip_prefix("chronicle/").unwrap_or(&key).to_owned();
    if !valid_key(&key) || key.starts_with('@') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "index path must be journal-relative",
        ));
    }
    Ok(key)
}

/// Writers that resolve their target through `contained_path` hold its
/// realpath. On Windows that is the verbatim `\\?\C:\...` form while the
/// configured journal root is not, so the lexical comparison misses; compare
/// both resolved before refusing.
fn resolved_relative(journal: &Path, path: &Path) -> Option<String> {
    let journal = realpath_non_strict(journal).ok()?;
    let path = realpath_non_strict(path).ok()?;
    relative_to_journal(&journal, &path)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn root(journal: &Path) -> PathBuf {
    journal.join("indexer/todo")
}

/// Save a hint after the source write, move or removal becomes visible.
/// Does not open SQLite, acquire its lease, or replace an existing hint.
pub fn enqueue_path(journal: &Path, path: &Path) -> io::Result<()> {
    enqueue(journal, path_key(journal, path)?, "path")
}

/// Publish the maintained path hint beneath a retained journal identity.
/// This keeps create-only publication and existing valid-hint deduplication.
pub fn enqueue_path_bound(
    namespace: &solstone_core_journal_io::journal_directory::JournalNamespace,
    path: &Path,
) -> io::Result<()> {
    namespace.revalidate()?;
    let owner = namespace.root();
    let entry = Entry {
        key: path_key(owner.canonical_path(), path)?,
        kind: "path".to_owned(),
        enqueued_ms: now_ms(),
        attempts: 0,
        not_before_ms: 0,
    };
    let directory = namespace
        .directory(Path::new("indexer/todo"), true)?
        .expect("created index hint directory");
    let name = entry.filename();
    let bytes = serde_json::to_vec(&entry)?;
    loop {
        namespace.revalidate()?;
        let result = solstone_core_journal_io::write_bytes_exclusive_bound_detailed(
            directory.as_ref(),
            std::ffi::OsStr::new(&name),
            &bytes,
            0o600,
        );
        match result {
            Ok(published) => {
                use solstone_core_journal_io::{
                    FinalNameConfirmation, MetadataDurability, StageCleanup,
                };
                let final_ok = matches!(
                    published.final_name,
                    FinalNameConfirmation::Confirmed { .. }
                );
                let cleanup_ok = matches!(&published.cleanup, StageCleanup::Removed);
                let durable = match &published.durability {
                    MetadataDurability::Confirmed => true,
                    #[cfg(windows)]
                    MetadataDurability::Unproven { source: None } => true,
                    _ => false,
                };
                if !final_ok || !cleanup_ok || !durable {
                    // Do not erase committed/cleanup facts or log hint contents.
                    return Err(io::Error::other(HintPublicationUncertain {
                        path: root(owner.canonical_path()).join(&name),
                        publication: published,
                    }));
                }
                namespace.revalidate()?;
                return Ok(());
            }
            Err(error) if error.source.kind() == io::ErrorKind::AlreadyExists => {
                #[cfg(unix)]
                let existing = solstone_core_journal_io::flat_directory::read_observed_file_bounded(
                    directory.as_ref(),
                    std::ffi::OsStr::new(&name),
                    usize::MAX,
                );
                #[cfg(windows)]
                let existing =
                    solstone_core_journal_io::windows_sync_dir::read_windows_observed_file_bounded(
                        directory.as_ref(),
                        std::ffi::OsStr::new(&name),
                        usize::MAX,
                    );
                match existing.map_err(io::Error::other)? {
                    Some(existing) => {
                        serde_json::from_slice::<serde_json::Value>(&existing.bytes)
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                        namespace.revalidate()?;
                        return Ok(());
                    }
                    None => continue,
                }
            }
            Err(error) => return Err(io::Error::other(error)),
        }
    }
}

#[derive(Debug)]
struct HintPublicationUncertain {
    path: PathBuf,
    publication: solstone_core_journal_io::ExclusivePublication,
}
impl std::fmt::Display for HintPublicationUncertain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use solstone_core_journal_io::{FinalNameConfirmation, MetadataDurability, StageCleanup};
        write!(
            formatter,
            "index hint {} was published",
            self.path.display()
        )?;
        match &self.publication.final_name {
            FinalNameConfirmation::Confirmed { .. } => write!(formatter, "; final name confirmed")?,
            FinalNameConfirmation::Unverified { reason, .. } => {
                write!(formatter, "; final name unverified: {reason}")?
            }
        }
        match &self.publication.cleanup {
            StageCleanup::Removed => write!(formatter, "; stage removed")?,
            StageCleanup::Retained { stage, source } => {
                write!(formatter, "; retained stage {:?}: {source}", stage)?
            }
        }
        match &self.publication.durability {
            MetadataDurability::Confirmed => write!(formatter, "; durability confirmed"),
            MetadataDurability::Unproven {
                source: Some(source),
            } => write!(formatter, "; durability unproven: {source}"),
            MetadataDurability::Unproven { source: None } => {
                write!(formatter, "; platform has no directory flush")
            }
        }
    }
}
impl std::error::Error for HintPublicationUncertain {}

pub fn enqueue_token(journal: &Path, token: Token) -> io::Result<()> {
    enqueue(journal, token.key().to_owned(), &token.key()[1..])
}

fn enqueue(journal: &Path, key: String, kind: &str) -> io::Result<()> {
    let entry = Entry {
        key,
        kind: kind.to_owned(),
        enqueued_ms: now_ms(),
        attempts: 0,
        not_before_ms: 0,
    };
    publish_if_absent(&root(journal).join(entry.filename()), &entry)
}

fn atomic_error(error: AtomicWriteError) -> io::Error {
    match error {
        AtomicWriteError::Io { source, .. } => source,
        #[cfg(windows)]
        other => io::Error::other(other),
    }
}

pub fn publish_if_absent(path: &Path, entry: &Entry) -> io::Result<()> {
    let bytes = serde_json::to_vec(entry)?;
    loop {
        match write_bytes_exclusive(path, &bytes, AtomicWriteOptions::default()) {
            Ok(()) => return Ok(()),
            Err(AtomicWriteError::Io { source, .. })
                if source.kind() == io::ErrorKind::AlreadyExists =>
            {
                match fs::read(path) {
                    Ok(existing) => {
                        // Valid future entries stay untouched. A broken directory
                        // or corrupt hint must not certify successful publication.
                        serde_json::from_slice::<serde_json::Value>(&existing)
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                        return Ok(());
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(atomic_error(error)),
        }
    }
}

pub fn rewrite(path: &Path, entry: &Entry) -> io::Result<()> {
    atomic_replace(
        path,
        &serde_json::to_vec(entry)?,
        AtomicWriteOptions::default(),
    )
    .map_err(atomic_error)
}

pub fn entry_paths(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // Temp files, set-asides and the claims directory never count as work.
        if name.len() == 69
            && name.ends_with(".json")
            && name.as_bytes()[..64].iter().all(u8::is_ascii_hexdigit)
            && entry.file_type()?.is_file()
        {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

fn decode(value: serde_json::Value, path: &Path) -> Option<Entry> {
    // Unknown fields and kinds belong to a newer binary. Leave its bytes alone.
    let entry: Entry = serde_json::from_value(value).ok()?;
    (entry.rank().is_some() && path.file_name()?.to_str()? == entry.filename()).then_some(entry)
}

pub fn read_entry(path: &Path, repair: bool) -> io::Result<Option<Entry>> {
    let value = if repair {
        match read_json_durable::<serde_json::Value>(ArtifactId::IndexTodo, path)? {
            DurableRead::Present(value) => value,
            DurableRead::Unreadable { error, .. } => return Err(io::Error::other(error)),
            _ => return Ok(None),
        }
    } else {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let Ok(value) = serde_json::from_slice(&bytes) else {
            return Ok(None);
        };
        value
    };
    Ok(decode(value, path))
}

/// Read-only: a future kind, delayed retry, or torn temp cannot trigger a task.
pub fn has_due(journal: &Path, at_ms: u64) -> io::Result<bool> {
    for path in entry_paths(&root(journal))? {
        if read_entry(&path, false)?.is_some_and(|entry| entry.not_before_ms <= at_ms) {
            return Ok(true);
        }
    }
    // A crashed drain may have left the only hint in claimed/.
    for path in entry_paths(&root(journal).join("claimed"))? {
        if read_entry(&path, false)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Clone, Debug, Default, Serialize, Eq, PartialEq)]
pub struct TodoStatus {
    pub pending: usize,
    pub claimed: usize,
    pub failed_deletions: usize,
    pub keys: Vec<String>,
}

/// Read the actual list, including claims, without changing it.
pub fn inspect(journal: &Path) -> io::Result<TodoStatus> {
    let mut status = TodoStatus::default();
    for (directory, claimed) in [
        (root(journal), false),
        (root(journal).join("claimed"), true),
    ] {
        for path in entry_paths(&directory)? {
            if let Some(entry) = read_entry(&path, false)? {
                if claimed {
                    status.claimed += 1
                } else {
                    status.pending += 1
                }
                if entry.kind == "path" && entry.attempts >= 3 && path_absent(journal, &entry.key) {
                    status.failed_deletions += 1;
                }
                status.keys.push(entry.key);
            }
        }
    }
    status.keys.sort();
    status.keys.dedup();
    Ok(status)
}

pub fn path_absent(journal: &Path, key: &str) -> bool {
    resolve_journal_path(journal, key).ok().is_some_and(
        |path| matches!(fs::metadata(path), Err(error) if error.kind() == io::ErrorKind::NotFound),
    )
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
