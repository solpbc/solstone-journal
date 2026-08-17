// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{DirEntryKind, list_dir_entries};

use super::paths::observer_path;
use super::record::ObserverRecord;

/// One canonical registry traversal.  `regular_json_entries` counts only
/// regular `*.json` siblings: it intentionally follows the same kind-before-
/// extension ordering as the loader.
#[derive(Debug)]
pub struct ObserverLoad {
    pub records: Vec<ObserverRecord>,
    pub regular_json_entries: usize,
}

#[derive(Debug)]
pub enum ReloadError {
    Directory(String),
}

impl std::fmt::Display for ReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Directory(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ReloadError {}

/// Canonical registry scan. Invalid siblings are intentionally skipped, just
/// like Python's ObserverRegistry reload path.
pub fn load_observers(journal_root: &Path) -> Result<Vec<ObserverRecord>, ReloadError> {
    Ok(load_observers_with_inventory(journal_root)?.records)
}

/// Load usable records and retain the raw regular-json denominator for callers
/// that must fail closed when an invalid registry sibling was skipped.
pub fn load_observers_with_inventory(journal_root: &Path) -> Result<ObserverLoad, ReloadError> {
    let directory = super::paths::observers_dir(journal_root);
    let entries =
        list_dir_entries(&directory).map_err(|error| ReloadError::Directory(error.to_string()))?;
    let mut records = Vec::new();
    let mut regular_json_entries = 0;
    for entry in entries {
        if entry.kind != DirEntryKind::File {
            continue;
        }
        if !entry.name.to_string_lossy().ends_with(".json") {
            continue;
        }
        regular_json_entries += 1;
        let Ok(bytes) = fs::read(&entry.path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Ok(record) = ObserverRecord::from_value(value) else {
            continue;
        };
        if entry.name.to_string_lossy() != format!("{}.json", record.prefix()) {
            continue;
        }
        records.push(record);
    }
    records.sort_by_key(|record| std::cmp::Reverse(record.created_at().unwrap_or(0)));
    Ok(ObserverLoad {
        records,
        regular_json_entries,
    })
}

pub fn find_observer(
    journal_root: &Path,
    identifier: &str,
) -> Result<Option<ObserverRecord>, ReloadError> {
    validate_safe_identifier(identifier)?;
    let records = load_observers(journal_root)?;
    if let Some(record) = records
        .into_iter()
        .find(|record| record.name() == Some(identifier))
    {
        return Ok(Some(record));
    }
    if !is_prefix(identifier) {
        return Ok(None);
    }
    // Deliberately does not compare filename with key[:8]: this is Python's
    // raw-prefix fallback asymmetry, after the safe-prefix gate above.
    let path = observer_path(journal_root, identifier);
    let Ok(bytes) = fs::read(path) else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    Ok(ObserverRecord::from_value(value).ok())
}

pub fn validate_safe_identifier(identifier: &str) -> Result<(), ReloadError> {
    if identifier.is_empty()
        || identifier.contains('/')
        || identifier.contains('\\')
        || identifier.contains("..")
    {
        return Err(ReloadError::Directory(
            "invalid observer identifier".to_owned(),
        ));
    }
    Ok(())
}

pub fn is_prefix(identifier: &str) -> bool {
    identifier.len() == 8
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;
    use serde_json::json;
    use std::fs;

    fn root(name: &str) -> std::path::PathBuf {
        let root = reserve_temp_path(&format!("observer-reload-{name}"));
        fs::create_dir_all(super::super::paths::observers_dir(&root)).expect("directory");
        root
    }
    fn write(root: &Path, name: &str, value: Value) {
        fs::write(
            super::super::paths::observers_dir(root).join(name),
            value.to_string(),
        )
        .expect("write");
    }
    fn valid(key: &str, name: &str, created_at: i64) -> Value {
        json!({"key":key,"name":name,"created_at":created_at})
    }

    #[test]
    fn reload_skips_invalid_siblings_and_sorts_descending() {
        let root = root("reload");
        write(&root, "abcdefgh.json", valid("abcdefgh-more", "older", 1));
        write(&root, "ijklmnop.json", valid("ijklmnop-more", "newer", 2));
        write(&root, "bad.json", json!({"key":"bad-key"}));
        write(&root, "qrstuvwx.json", json!(["not", "object"]));
        fs::write(
            super::super::paths::observers_dir(&root).join("broken.json"),
            "{",
        )
        .expect("broken");
        let records = load_observers(&root).expect("load");
        assert_eq!(
            records
                .iter()
                .map(|record| record.name())
                .collect::<Vec<_>>(),
            vec![Some("newer"), Some("older")]
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn inventory_counts_only_regular_json_entries() {
        let root = root("inventory");
        write(&root, "abcdefgh.json", valid("abcdefgh-more", "visible", 1));
        fs::create_dir(super::super::paths::observers_dir(&root).join("x.json"))
            .expect("json-named directory");
        write(&root, "broken.json", json!({"key": "wrong-prefix-more"}));

        let loaded = load_observers_with_inventory(&root).expect("load inventory");
        assert_eq!(loaded.regular_json_entries, 2);
        assert_eq!(loaded.records.len(), 1);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn name_lookup_is_canonical_but_raw_prefix_fallback_is_not() {
        let root = root("asymmetry");
        write(&root, "wrongkey.json", valid("abcdefgh-more", "hidden", 1));
        assert!(find_observer(&root, "hidden").expect("find").is_none());
        write(&root, "abcdefgh.json", valid("abcdefgh-more", "visible", 2));
        assert_eq!(
            find_observer(&root, "visible")
                .expect("find")
                .expect("record")
                .name(),
            Some("visible")
        );
        write(
            &root,
            "ijklmnop.json",
            valid("qrstuvwx-more", "raw-only", 3),
        );
        assert_eq!(
            find_observer(&root, "ijklmnop")
                .expect("find")
                .expect("record")
                .name(),
            Some("raw-only")
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn unsafe_identifier_is_rejected_before_raw_path_lookup() {
        let root = root("unsafe");
        assert!(find_observer(&root, "../outside").is_err());
        assert!(find_observer(&root, "abc/def").is_err());
        assert!(find_observer(&root, "abc\\def").is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
