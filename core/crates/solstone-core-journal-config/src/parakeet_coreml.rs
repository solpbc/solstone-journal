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

use std::fs;
use std::path::{Path, PathBuf};

use crate::JournalConfigRead;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default CoreML Parakeet cache directory beneath an explicit home directory.
pub fn default_parakeet_coreml_cache_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/solstone/parakeet/models")
}

/// The fixed sentinel path, independent of a configured cache override.
pub fn parakeet_coreml_sentinel_path(home: &Path) -> PathBuf {
    default_parakeet_coreml_cache_dir(home).join(".install-complete")
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

/// The sentinel contract shared by the installer and readiness checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParakeetCoremlSentinel {
    schema_version: u8,
    backend: String,
    variant: String,
    model_version: String,
    quantization: String,
    fluidaudio_version: Value,
    platform: ParakeetCoremlSentinelPlatform,
    cache_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ParakeetCoremlSentinelPlatform {
    os: String,
    arch: String,
}

impl ParakeetCoremlSentinel {
    pub fn new(cache_dir: PathBuf, os_name: &str, arch: &str, fluidaudio_version: &str) -> Self {
        Self {
            schema_version: 1,
            backend: "parakeet".to_owned(),
            variant: "coreml".to_owned(),
            model_version: "v3".to_owned(),
            quantization: "fp32".to_owned(),
            fluidaudio_version: Value::String(fluidaudio_version.to_owned()),
            platform: ParakeetCoremlSentinelPlatform {
                os: os_name.to_owned(),
                arch: arch.to_owned(),
            },
            cache_dir,
        }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    fn is_valid_for(&self, os_name: &str, arch: &str) -> bool {
        self.schema_version == 1
            && self.backend == "parakeet"
            && self.variant == "coreml"
            && self.model_version == "v3"
            && self.quantization == "fp32"
            && !self.fluidaudio_version.is_null()
            && self.platform.os == os_name
            && self.platform.arch == arch
            && self.cache_dir.exists()
    }
}

/// Read a sentinel only when it satisfies the shared installed-state contract.
pub fn read_valid_parakeet_coreml_sentinel(
    home: &Path,
    os_name: &str,
    arch: &str,
) -> Option<ParakeetCoremlSentinel> {
    let sentinel = fs::read(parakeet_coreml_sentinel_path(home)).ok()?;
    let sentinel = serde_json::from_slice::<ParakeetCoremlSentinel>(&sentinel).ok()?;
    sentinel.is_valid_for(os_name, arch).then_some(sentinel)
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
