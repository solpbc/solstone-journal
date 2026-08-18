// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{DirEntryKind, list_dir_entries};

use super::super::history::load_history;
use super::super::paths::history_dir;
use super::super::reload::load_observers;
use super::types::Refusal;

/// Resolve the one observer that owns `stream`, refusing on no owner or an
/// ambiguous one. A stream name is a label, not an identity: one device may
/// own several streams, so a locked `stream` field wins outright, and
/// otherwise every unrevoked observer's history is scanned for a reference.
pub fn observer_prefix_for_stream(journal: &Path, stream: &str) -> Result<String, Refusal> {
    let observers = load_observers(journal).unwrap_or_default();
    let locked: Vec<&str> = observers
        .iter()
        .filter(|record| {
            !record.revoked()
                && record.value().get("stream").and_then(Value::as_str) == Some(stream)
        })
        .map(|record| {
            record
                .value()
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
        })
        .collect();
    if locked.len() == 1 {
        return Ok(locked[0].chars().take(8).collect());
    }
    if locked.len() > 1 {
        return Err(Refusal::new(
            stream,
            "observer-attribution",
            Some("apps/observer/observers".to_owned()),
            "multiple active observers own this locked stream; reconcile observers first",
        ));
    }
    let mut prefixes = std::collections::BTreeSet::new();
    for record in &observers {
        if record.revoked() {
            continue;
        }
        let prefix = record.prefix();
        let hist_dir = history_dir(journal, &prefix);
        let Ok(entries) = list_dir_entries(&hist_dir) else {
            continue;
        };
        for entry in entries {
            if entry.kind != DirEntryKind::File {
                continue;
            }
            let Some(day) = entry
                .name
                .to_str()
                .and_then(|name| name.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let history = load_history(&hist_dir.join(format!("{day}.jsonl")));
            if history.stopped.is_some() {
                return Err(super::history::torn_history_refusal(day, stream));
            }
            if history
                .records
                .iter()
                .any(|record| record.get("stream").and_then(Value::as_str) == Some(stream))
            {
                prefixes.insert(prefix.clone());
                break;
            }
        }
    }
    if prefixes.len() == 1 {
        return Ok(prefixes.into_iter().next().expect("nonempty"));
    }
    if prefixes.is_empty() {
        return Err(Refusal::new(
            stream,
            "observer-attribution",
            Some("apps/observer/observers".to_owned()),
            "no observer owns or references this stream; create an unambiguous observer history first",
        ));
    }
    Err(Refusal::new(
        stream,
        "observer-attribution",
        Some("apps/observer/observers/*/hist".to_owned()),
        "multiple observer histories reference this stream; reconcile ownership first",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::record::ObserverRecord;
    use crate::store::write::save_observer;
    use crate::test_support::reserve_temp_path;
    use serde_json::json;
    use std::fs;

    fn root(name: &str) -> std::path::PathBuf {
        reserve_temp_path(&format!("observer-prune-attribution-{name}"))
    }

    fn seed(root: &Path, key: &str, stream: Option<&str>) {
        let mut value = json!({"key": key, "name": key});
        if let Some(stream) = stream {
            value["stream"] = json!(stream);
        }
        let record = ObserverRecord::from_value(value).expect("record");
        save_observer(root, &record).expect("save");
    }

    #[test]
    fn no_owner_refuses() {
        let root = root("no-owner");
        assert!(observer_prefix_for_stream(&root, "workstation").is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn locked_stream_field_wins_outright() {
        let root = root("locked");
        seed(&root, "abcdefgh1", Some("workstation"));
        assert_eq!(
            observer_prefix_for_stream(&root, "workstation").unwrap(),
            "abcdefgh"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ambiguous_locked_owners_refuse() {
        let root = root("ambiguous-locked");
        seed(&root, "abcdefgh1", Some("workstation"));
        seed(&root, "ijklmnop2", Some("workstation"));
        let error = observer_prefix_for_stream(&root, "workstation").unwrap_err();
        assert_eq!(error.gate, "observer-attribution");
        assert!(error.resolution.contains("multiple active observers"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn torn_history_does_not_attribute() {
        use crate::store::paths::history_path;
        let root = root("torn-attr");
        seed(&root, "abcdefgh1", None);
        let path = history_path(&root, "abcdefgh", "20260101");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "{\"stream\":\"workstation\",\"segment\":\"090000_300\"}\n{broken}\n",
        )
        .unwrap();
        let error = observer_prefix_for_stream(&root, "workstation").unwrap_err();
        assert_eq!(error.gate, "sync-history");
        fs::write(
            &path,
            "{\"stream\":\"workstation\",\"segment\":\"090000_300\"}\n",
        )
        .unwrap();
        assert_eq!(
            observer_prefix_for_stream(&root, "workstation").unwrap(),
            "abcdefgh"
        );
        fs::remove_dir_all(&root).ok();
    }
}
