// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Facet names that no longer belong to a live facet.
//!
//! A facet's directory name is fixed when it is created and is never reused.
//! When a facet is deleted or merged away, its name is recorded in
//! `facets/retired.json` together with the facet's id and, for a merge, the id
//! of the facet it was merged into. Stored references to the name (segment
//! assignments and per-segment `talents/<name>/` output) resolve through that
//! record, and creating a new facet under the name is refused.
//!
//! The file sits directly under `facets/` and is not a directory, so every
//! reader that lists facet directories ignores it.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_journal_io::{JsonWriteOptions, write_json};

use super::declaration::read_facet_declaration;
use super::error::FacetWriteError;
use super::facet_id::is_well_formed_facet_id;
use super::paths::facets_dir;

pub const RETIRED_FACETS_FILE: &str = "retired.json";
const PREVIOUS_SUFFIX: &str = ".prev";

/// Why a facet name was retired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetiredFacetState {
    Merged,
    Renamed,
    Deleted,
}

impl RetiredFacetState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Renamed => "renamed",
            Self::Deleted => "deleted",
        }
    }
}

/// One retired facet name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetiredFacet {
    pub state: RetiredFacetState,
    /// The retired facet's own id, when it had one.
    pub id: Option<String>,
    /// For a merge or rename, the id of the live facet references now resolve to.
    pub successor: Option<String>,
    pub title: Option<String>,
    pub at: Option<String>,
}

impl RetiredFacet {
    pub fn deleted(id: Option<String>, title: Option<String>) -> Self {
        Self {
            state: RetiredFacetState::Deleted,
            id,
            successor: None,
            title,
            at: Some(Utc::now().to_rfc3339()),
        }
    }

    pub fn merged(id: Option<String>, successor: String, title: Option<String>) -> Self {
        Self {
            state: RetiredFacetState::Merged,
            id,
            successor: Some(successor),
            title,
            at: Some(Utc::now().to_rfc3339()),
        }
    }

    pub fn renamed(id: Option<String>, successor: String) -> Self {
        Self {
            state: RetiredFacetState::Renamed,
            id,
            successor: Some(successor),
            title: None,
            at: Some(Utc::now().to_rfc3339()),
        }
    }

    /// Entries are compared on what resolution depends on; title and time are
    /// informational.
    pub fn same_identity(&self, other: &Self) -> bool {
        self.state == other.state && self.id == other.id && self.successor == other.successor
    }

    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            "state".to_owned(),
            Value::String(self.state.as_str().to_owned()),
        );
        object.insert(
            "id".to_owned(),
            self.id.clone().map_or(Value::Null, Value::String),
        );
        if let Some(successor) = &self.successor {
            object.insert("successor".to_owned(), Value::String(successor.clone()));
        }
        if let Some(title) = &self.title {
            object.insert("title".to_owned(), Value::String(title.clone()));
        }
        if let Some(at) = &self.at {
            object.insert("at".to_owned(), Value::String(at.clone()));
        }
        Value::Object(object)
    }
}

/// What `facets/retired.json` currently holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetiredFacets {
    Absent,
    Loaded(BTreeMap<String, RetiredFacet>),
    /// The file was read but is not a valid record.
    Malformed(String),
    /// The file exists but could not be read.
    Unreadable(String),
}

impl RetiredFacets {
    /// Entries a reader may rely on. A damaged file yields none, which fails
    /// closed: every name that is not live then resolves to no facet.
    pub fn entries(&self) -> BTreeMap<String, RetiredFacet> {
        match self {
            Self::Loaded(entries) => entries.clone(),
            _ => BTreeMap::new(),
        }
    }

    /// Entries a writer may build on. A damaged file is never overwritten.
    pub fn entries_for_write(self) -> Result<BTreeMap<String, RetiredFacet>, FacetWriteError> {
        match self {
            Self::Absent => Ok(BTreeMap::new()),
            Self::Loaded(entries) => Ok(entries),
            Self::Malformed(detail) | Self::Unreadable(detail) => {
                Err(FacetWriteError::RetiredFileDamaged { detail })
            }
        }
    }
}

pub fn retired_facets_path(journal_root: &Path) -> Result<PathBuf, FacetWriteError> {
    Ok(facets_dir(journal_root)
        .map_err(FacetWriteError::Read)?
        .join(RETIRED_FACETS_FILE))
}

/// Read `facets/retired.json`.
///
/// Keep the parse behaviorally identical to the indexer's reader in
/// `solstone-core-indexer-store` (`classification.rs` `read_retired_names`);
/// both are checked against one shared fixture corpus.
pub fn read_retired_facets(journal_root: &Path) -> RetiredFacets {
    let Ok(path) = retired_facets_path(journal_root) else {
        return RetiredFacets::Absent;
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return RetiredFacets::Absent,
        Err(error) => return RetiredFacets::Unreadable(error.to_string()),
    };
    parse_retired_facets(&text)
}

pub fn parse_retired_facets(text: &str) -> RetiredFacets {
    let root = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(root)) => root,
        Ok(_) => return RetiredFacets::Malformed("not a JSON object".to_owned()),
        Err(error) => return RetiredFacets::Malformed(error.to_string()),
    };
    let Some(Value::Object(names)) = root.get("names") else {
        return RetiredFacets::Malformed("missing names object".to_owned());
    };
    let well_formed = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .filter(|id| is_well_formed_facet_id(id))
            .map(str::to_owned)
    };
    let mut entries = BTreeMap::new();
    for (name, entry) in names {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        // An unknown state reserves the name like a deletion: no successor.
        let state = match entry.get("state").and_then(Value::as_str) {
            Some("merged") => RetiredFacetState::Merged,
            Some("renamed") => RetiredFacetState::Renamed,
            _ => RetiredFacetState::Deleted,
        };
        let successor = match state {
            RetiredFacetState::Merged | RetiredFacetState::Renamed => {
                well_formed(entry.get("successor"))
            }
            RetiredFacetState::Deleted => None,
        };
        entries.insert(
            name.clone(),
            RetiredFacet {
                state,
                id: well_formed(entry.get("id")),
                successor,
                title: entry
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                at: entry.get("at").and_then(Value::as_str).map(str::to_owned),
            },
        );
    }
    RetiredFacets::Loaded(entries)
}

/// The retired entry for `name`, refusing when the file is damaged.
pub fn retired_facet_entry(
    journal_root: &Path,
    name: &str,
) -> Result<Option<RetiredFacet>, FacetWriteError> {
    Ok(read_retired_facets(journal_root)
        .entries_for_write()?
        .remove(name))
}

/// The id of the live facet at `name`, when its declaration carries one.
fn live_id(journal_root: &Path, name: &str) -> Result<Option<String>, FacetWriteError> {
    Ok(
        read_facet_declaration(journal_root, name)?.and_then(|snapshot| {
            snapshot
                .value()
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| is_well_formed_facet_id(id))
                .map(str::to_owned)
        }),
    )
}

/// Record a retired name under the facet trust lock (re-entrant for callers
/// that already hold it).
///
/// Writing an entry with the same identity is a no-op. An existing entry may
/// be replaced only when it is a leftover of an operation on the live facet
/// at the same name that did not complete, and the new entry is written by
/// that same facet: every other difference is refused, because a committed
/// entry is permanent.
pub fn record_retired_facet(
    journal_root: &Path,
    name: &str,
    entry: RetiredFacet,
) -> Result<(), FacetWriteError> {
    let _trust = crate::hold_facet_trust_lock(journal_root)?;
    let mut entries = read_retired_facets(journal_root).entries_for_write()?;
    if let Some(existing) = entries.get(name) {
        if existing.same_identity(&entry) {
            return Ok(());
        }
        let live = live_id(journal_root, name)?;
        let leftover = match (&live, &existing.id) {
            (Some(live), Some(existing)) => live == existing,
            // An id-less leftover of a facet that has been given an id since.
            (Some(_), None) => true,
            (None, _) => false,
        };
        if !(leftover && live.is_some() && live == entry.id) {
            return Err(FacetWriteError::RetiredEntryConflict {
                name: name.to_owned(),
            });
        }
    }
    entries.insert(name.to_owned(), entry);
    write_retired_facets(journal_root, &entries)
}

/// The exact bytes of the retired record and its last-good copy, taken before
/// an operation writes to them, so a rollback can put both back as they were.
#[derive(Clone, Debug)]
pub struct RetiredFilesSnapshot {
    record: Option<Vec<u8>>,
    previous: Option<Vec<u8>>,
}

fn previous_path(path: &Path) -> PathBuf {
    let mut previous = path.to_path_buf().into_os_string();
    previous.push(PREVIOUS_SUFFIX);
    PathBuf::from(previous)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, FacetWriteError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(FacetWriteError::RetiredFileDamaged {
            detail: error.to_string(),
        }),
    }
}

pub fn snapshot_retired_files(
    journal_root: &Path,
) -> Result<RetiredFilesSnapshot, FacetWriteError> {
    let path = retired_facets_path(journal_root)?;
    Ok(RetiredFilesSnapshot {
        record: read_optional(&path)?,
        previous: read_optional(&previous_path(&path))?,
    })
}

/// Put the retired record back exactly as a snapshot found it.
pub fn restore_retired_files(
    journal_root: &Path,
    snapshot: &RetiredFilesSnapshot,
) -> Result<(), FacetWriteError> {
    let _trust = crate::hold_facet_trust_lock(journal_root)?;
    let path = retired_facets_path(journal_root)?;
    for (target, bytes) in [
        (path.clone(), &snapshot.record),
        (previous_path(&path), &snapshot.previous),
    ] {
        match bytes {
            Some(bytes) => solstone_core_journal_io::atomic_replace(
                &target,
                bytes,
                solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
            )
            .map_err(FacetWriteError::DeclarationWrite)?,
            None => match fs::remove_file(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(FacetWriteError::RetiredFileDamaged {
                        detail: error.to_string(),
                    });
                }
            },
        }
    }
    Ok(())
}

fn write_retired_facets(
    journal_root: &Path,
    entries: &BTreeMap<String, RetiredFacet>,
) -> Result<(), FacetWriteError> {
    let path = retired_facets_path(journal_root)?;
    let options = JsonWriteOptions {
        mode: Some(0o600),
        indent: Some(2),
        sort_keys: false,
    };
    // Keep the last good record beside the new one, so a later damaged write
    // can be recovered without freeing any name.
    if let Ok(previous) = fs::read_to_string(&path)
        && let RetiredFacets::Loaded(_) = parse_retired_facets(&previous)
        && let Ok(value) = serde_json::from_str::<Value>(&previous)
    {
        write_json(previous_path(&path), &value, options)
            .map_err(FacetWriteError::DeclarationWrite)?;
    }
    let names = entries
        .iter()
        .map(|(name, entry)| (name.clone(), entry.to_value()))
        .collect::<Map<_, _>>();
    let mut root = Map::new();
    root.insert("names".to_owned(), Value::Object(names));
    write_json(&path, &Value::Object(root), options).map_err(FacetWriteError::DeclarationWrite)
}

/// Whether `name` is free for a new facet: no directory of any kind and no
/// retired entry. A damaged retired file refuses.
pub fn facet_name_is_free(journal_root: &Path, name: &str) -> Result<bool, FacetWriteError> {
    let dir = facets_dir(journal_root)
        .map_err(FacetWriteError::Read)?
        .join(name);
    if solstone_core_journal_io::path_lexists(&dir)
        .map_err(|error| FacetWriteError::Read(super::error::FacetStoreError::from(error)))?
    {
        return Ok(false);
    }
    Ok(retired_facet_entry(journal_root, name)?.is_none())
}

/// The first free name among `base`, `base-2`, `base-3`, ...
pub fn first_free_facet_name(journal_root: &Path, base: &str) -> Result<String, FacetWriteError> {
    if facet_name_is_free(journal_root, base)? {
        return Ok(base.to_owned());
    }
    for suffix in 2..10_000 {
        let candidate = format!("{base}-{suffix}");
        if facet_name_is_free(journal_root, &candidate)? {
            return Ok(candidate);
        }
    }
    Err(FacetWriteError::NameRetired {
        name: base.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::{RetiredFacets, parse_retired_facets};
    use serde_json::Value;

    #[test]
    fn retired_record_parse_matches_the_shared_corpus() {
        let corpus: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/retired-facets-corpus.json"),
            )
            .expect("corpus"),
        )
        .expect("corpus json");
        for case in corpus["cases"].as_array().expect("cases") {
            let input = case["input"].as_str().expect("input");
            let parsed = match parse_retired_facets(input) {
                RetiredFacets::Loaded(entries) => Some(entries),
                _ => None,
            };
            match case["expected"].as_object() {
                None => assert!(parsed.is_none(), "{input}"),
                Some(expected) => {
                    let parsed = parsed.expect("loaded");
                    assert_eq!(parsed.len(), expected.len(), "{input}");
                    for (name, entry) in expected {
                        let got = &parsed[name];
                        assert_eq!(got.id.as_deref(), entry["id"].as_str(), "{input}");
                        assert_eq!(
                            got.successor.as_deref(),
                            entry["successor"].as_str(),
                            "{input}"
                        );
                    }
                }
            }
        }
    }
}
