// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Parakeet CoreML cache-layout authority.
//!
//! | concern                            | resolves to                                                        |
//! |------------------------------------|--------------------------------------------------------------------|
//! | sentinel *location*                | default_parakeet_coreml_cache_dir(home)/.install-complete —        |
//! |                                    | always, ignores config                                             |
//! | model tree *destination*           | parakeet_coreml_model_root(parakeet_coreml_cache_dir(config, home)) |
//! |                                    | — follows config                                                   |
//! | sentinel's recorded `cache_dir`    | parakeet_coreml_cache_dir(config, home) — follows config,          |
//! | field                              | byte-for-byte what the backend passes as --cache-dir               |
//!
//! Recording the default while the backend resolves an override makes readiness
//! compute a model root the helper never uses, report ready, and trigger
//! FluidAudio's delete-and-re-download path.

use std::path::{Path, PathBuf};

use crate::JournalConfigRead;

/// Default CoreML Parakeet cache directory beneath an explicit home directory.
pub fn default_parakeet_coreml_cache_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/solstone/parakeet/models")
}

/// Configured CoreML cache directory, falling back to the default on absence,
/// emptiness, or an invalid value.
pub fn parakeet_coreml_cache_dir(config: &JournalConfigRead, home: &Path) -> PathBuf {
    config
        .config
        .as_ref()
        .and_then(|root| root.get("transcribe"))
        .and_then(|transcribe| transcribe.as_object())
        .and_then(|transcribe| transcribe.get("parakeet"))
        .and_then(|parakeet| parakeet.as_object())
        .and_then(|parakeet| parakeet.get("cache_dir"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_parakeet_coreml_cache_dir(home))
}

/// Model tree paired with a cache directory.
pub fn parakeet_coreml_model_root(cache_dir: &Path) -> PathBuf {
    cache_dir
        .parent()
        .unwrap_or(cache_dir)
        .join("parakeet-tdt-0.6b-v3")
}

#[cfg(test)]
mod tests {
    use super::{default_parakeet_coreml_cache_dir, parakeet_coreml_cache_dir};
    use crate::JournalConfigRead;
    use serde_json::{Map, Value, json};
    use std::path::PathBuf;

    fn config(value: Value) -> JournalConfigRead {
        JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(value.as_object().unwrap().clone()),
        }
    }

    #[test]
    fn cache_dir_uses_explicit_home_and_only_a_nonempty_string_override() {
        let home = PathBuf::from("/test/home");
        let configured = config(json!({"transcribe": {"parakeet": {"cache_dir": "/cache"}}}));
        assert_eq!(
            parakeet_coreml_cache_dir(&configured, &home),
            PathBuf::from("/cache")
        );

        for value in [Value::Null, json!(""), json!(42)] {
            let mut parakeet = Map::new();
            parakeet.insert("cache_dir".to_owned(), value);
            let mut transcribe = Map::new();
            transcribe.insert("parakeet".to_owned(), Value::Object(parakeet));
            let mut root = Map::new();
            root.insert("transcribe".to_owned(), Value::Object(transcribe));
            let invalid = JournalConfigRead {
                present: true,
                sha256: None,
                config: Some(root),
            };
            assert_eq!(
                parakeet_coreml_cache_dir(&invalid, &home),
                default_parakeet_coreml_cache_dir(&home)
            );
        }
    }
}
