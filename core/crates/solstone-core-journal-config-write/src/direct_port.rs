// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Persist the journal-local paired-device door port.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{JournalRoot, LockOptions};

use crate::{
    BoundConfigRefusal, BoundConfigTransaction, ConfigMutationError, JournalConfigMutation,
    JournalConfigTransaction, mutate_journal_config, mutate_journal_config_at_root_checked,
};

/// Write `pairing.direct_port` when it differs from `port`.
pub fn persist_direct_door_port(
    journal: &Path,
    port: u16,
) -> Result<JournalConfigTransaction<()>, ConfigMutationError> {
    mutate_journal_config(journal, LockOptions::default(), |config| {
        set_port(config, port)
    })
}

/// Persist the port through the retained root and the normal config owner.
pub fn persist_direct_door_port_at_root_checked(
    root: &Arc<JournalRoot>,
    port: u16,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<BoundConfigTransaction<()>, BoundConfigRefusal> {
    mutate_journal_config_at_root_checked(root, LockOptions::default(), checkpoint, |config| {
        set_port(config, port)
    })
}

fn set_port(config: &mut Map<String, Value>, port: u16) -> JournalConfigMutation<()> {
    let pairing = object_at(config, "pairing");
    let next = json!(port);
    let changed = pairing.get("direct_port") != Some(&next);
    pairing.insert("direct_port".to_owned(), next);
    JournalConfigMutation { changed, value: () }
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
