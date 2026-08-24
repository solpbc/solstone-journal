// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Persist the journal-local paired-device door port.

use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::LockOptions;

use crate::{
    ConfigMutationError, JournalConfigMutation, JournalConfigTransaction, mutate_journal_config,
};

/// Write `pairing.direct_port` when it differs from `port`.
pub fn persist_direct_door_port(
    journal: &Path,
    port: u16,
) -> Result<JournalConfigTransaction<()>, ConfigMutationError> {
    mutate_journal_config(journal, LockOptions::default(), |config| {
        let pairing = object_at(config, "pairing");
        let next = json!(port);
        let changed = pairing.get("direct_port") != Some(&next);
        pairing.insert("direct_port".to_owned(), next);
        JournalConfigMutation { changed, value: () }
    })
}

fn object_at<'a>(parent: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !parent.get(key).is_some_and(Value::is_object) {
        parent.insert(key.to_owned(), Value::Object(Map::new()));
    }
    parent
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object inserted")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use solstone_core_journal_config::get_journal_config_path;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn writes_port_and_is_idempotent() {
        let temporary = TempDir::new();
        let first = persist_direct_door_port(temporary.path(), 9000).unwrap();
        assert!(first.changed);
        assert!(first.written);
        let path = get_journal_config_path(temporary.path());
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["pairing"]["direct_port"], json!(9000));

        let second = persist_direct_door_port(temporary.path(), 9000).unwrap();
        assert!(!second.changed);
        assert!(!second.written);
    }
}
