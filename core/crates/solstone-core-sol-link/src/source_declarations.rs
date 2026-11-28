// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Full snapshots published by paired devices. The authorization ledger lock
//! serializes declaration changes with unpair and certificate replacement.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::client_description_store::read_descriptions;
use crate::ledger::{AuthorizedClientsRead, read_authorized_clients};
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};

const FILE: &str = "link/source-declarations.json";
pub const MAX_ENTRIES: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    On,
    Paused,
    NeedsAttention,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub source: String,
    pub state: State,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Declaration {
    pub sources: Vec<Source>,
    pub declared_at: String,
}

#[derive(Debug)]
pub enum DeclarationError {
    Invalid(&'static str),
    Revoked,
    Unavailable,
}

pub fn validate(mut sources: Vec<Source>) -> Result<Vec<Source>, DeclarationError> {
    use DeclarationError::Invalid;
    if sources.len() > MAX_ENTRIES {
        return Err(Invalid("Send at most 64 sources."));
    }
    let mut seen = BTreeSet::new();
    for entry in &sources {
        let key = entry.source.as_bytes();
        if key.len() > 48
            || !key.first().is_some_and(u8::is_ascii_lowercase)
            || !key
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
        {
            return Err(Invalid(
                "Use a lowercase source identifier of 1 to 48 ASCII letters, digits, underscores or hyphens, starting with a letter.",
            ));
        }
        if let Some(detail) = &entry.detail
            && (detail.trim().is_empty()
                || detail.len() > 256
                || detail.chars().any(char::is_control))
        {
            return Err(Invalid(
                "Use a nonblank detail of at most 256 UTF-8 bytes without control characters.",
            ));
        }
        if !seen.insert((&entry.source, &entry.detail)) {
            return Err(Invalid("Send each source and detail combination once."));
        }
    }
    sources.sort_by(|a, b| (&a.source, &a.detail).cmp(&(&b.source, &b.detail)));
    Ok(sources)
}

pub fn read(root: &Path) -> Result<BTreeMap<String, Declaration>, DeclarationError> {
    match fs::read(root.join(FILE)) {
        // A malformed snapshot is regenerable on reconnect. I/O failures
        // must not cause one writer to overwrite another device's snapshot.
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(_) => Err(DeclarationError::Unavailable),
    }
}

fn write(root: &Path, rows: &BTreeMap<String, Declaration>) -> Result<(), DeclarationError> {
    write_json(
        root.join(FILE),
        rows,
        JsonWriteOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )
    .map_err(|_| DeclarationError::Unavailable)
}

// Called while holding the same authorization lock as replace.
pub(crate) fn remove_locked(root: &Path, cid: &str) -> Result<(), DeclarationError> {
    let mut rows = read(root)?;
    if rows.remove(cid).is_some() {
        write(root, &rows)?;
    }
    Ok(())
}

pub fn replace(
    root: &Path,
    cid: &str,
    sources: Vec<Source>,
    now: String,
) -> Result<(), DeclarationError> {
    let sources = validate(sources)?;
    let auth = root.join("link/authorized_clients.json");
    let _guard = hold_lock(
        &auth,
        LockOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )
    .map_err(|_| DeclarationError::Unavailable)?;
    read_descriptions(root).map_err(|_| DeclarationError::Unavailable)?;
    match read_authorized_clients(&auth) {
        AuthorizedClientsRead::Present(entries) if entries.iter().any(|e| e.fingerprint == cid) => {
        }
        AuthorizedClientsRead::Present(_) | AuthorizedClientsRead::Missing => {
            return Err(DeclarationError::Revoked);
        }
        _ => return Err(DeclarationError::Unavailable),
    }
    let mut rows = read(root)?;
    if rows.get(cid).is_some_and(|row| row.sources == sources) {
        return Ok(());
    }
    rows.insert(
        cid.to_owned(),
        Declaration {
            sources,
            declared_at: now,
        },
    );
    write(root, &rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, detail: Option<&str>) -> Source {
        Source {
            source: key.into(),
            detail: detail.map(str::to_owned),
            state: State::On,
        }
    }

    #[test]
    fn declaration_keys_allow_future_sources_and_distinct_vaults() {
        let a = row("obsidian", Some("A"));
        let b = row("obsidian", Some("B"));
        let future = row("next_source-2", None);
        assert_eq!(
            validate(vec![b.clone(), future.clone(), a.clone()]).unwrap(),
            vec![future, a.clone(), b]
        );
        assert!(validate(vec![a.clone(), a]).is_err());
        assert!(validate(vec![]).unwrap().is_empty());
    }

    #[test]
    fn invalid_identifiers_and_details_are_rejected_as_a_unit() {
        for key in ["", "1source", "Upper", "a b", "日"] {
            assert!(validate(vec![row("calendar", None), row(key, None)]).is_err());
        }
        for detail in [" ", "a\nb", &"é".repeat(129)] {
            assert!(validate(vec![row("obsidian", Some(detail))]).is_err());
        }
        assert!(validate(vec![row("obsidian", None); 65]).is_err());
    }
}
