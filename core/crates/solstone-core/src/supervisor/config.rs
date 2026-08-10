// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Narrow supervisor-only reads from the journal configuration.

use std::path::Path;

use serde_json::Value;
use solstone_core_journal_config::read_journal_config;

pub(crate) fn processing_is_deferred(journal: &Path) -> bool {
    let Some(config) = read_config(journal) else {
        return false;
    };
    config
        .get("processing")
        .and_then(Value::as_object)
        .and_then(|processing| processing.get("mode"))
        .and_then(Value::as_str)
        == Some("deferred")
}

pub(crate) fn no_thinking_engine_chosen(journal: &Path) -> bool {
    let Some(config) = read_config(journal) else {
        return true;
    };
    !config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("active").and_then(Value::as_object))
        .and_then(|active| active.get("provider"))
        .and_then(Value::as_str)
        .is_some_and(|provider| !provider.trim().is_empty())
}

fn read_config(journal: &Path) -> Option<serde_json::Map<String, Value>> {
    read_journal_config(journal).ok()?.config
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

    fn journal(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "solstone-supervisor-config-{name}-{}",
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("config")).expect("config directory");
        path
    }

    #[test]
    fn missing_or_malformed_config_uses_safe_defaults() {
        let path = journal("missing");
        assert!(!processing_is_deferred(&path));
        assert!(no_thinking_engine_chosen(&path));
        fs::write(path.join("config/journal.json"), b"{").expect("malformed config");
        assert!(!processing_is_deferred(&path));
        assert!(no_thinking_engine_chosen(&path));
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn predicates_handle_wrong_types_and_valid_values() {
        let path = journal("values");
        fs::write(
            path.join("config/journal.json"),
            br#"{"processing":{"mode":true},"providers":{"active":{"provider":"  "}}}"#,
        )
        .expect("wrong type config");
        assert!(!processing_is_deferred(&path));
        assert!(no_thinking_engine_chosen(&path));
        fs::write(
            path.join("config/journal.json"),
            br#"{"processing":{"mode":"deferred"},"providers":{"active":{"provider":"local"}}}"#,
        )
        .expect("valid config");
        assert!(processing_is_deferred(&path));
        assert!(!no_thinking_engine_chosen(Path::new(&path)));
        let _ = fs::remove_dir_all(path);
    }
}
