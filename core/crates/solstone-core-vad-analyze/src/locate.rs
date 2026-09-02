// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure path resolution for the native VAD helper binary.
//!
//! Resolution never spawns a subprocess: the caller supplies the base
//! directory it expects the helper to live in, and the environment lookup is
//! injected so tests never mutate process-global environment state.

use std::path::{Path, PathBuf};

/// File name of the native VAD helper binary.
pub const VAD_BINARY_NAME: &str = "solstone-core-vad-analyze";

/// Environment variable that pins an explicit helper binary path.
pub const VAD_BINARY_ENV: &str = "SOLSTONE_VAD_BINARY";

/// Resolves the helper binary path from `base_dir`, honoring an explicit
/// `SOLSTONE_VAD_BINARY` override read through `lookup_env`.
pub fn resolve_vad_binary<F>(base_dir: &Path, lookup_env: F) -> PathBuf
where
    F: Fn(&str) -> Option<String>,
{
    match lookup_env(VAD_BINARY_ENV) {
        Some(override_path) if !override_path.is_empty() => PathBuf::from(override_path),
        _ => base_dir.join(VAD_BINARY_NAME),
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    #[test]
    fn explicit_env_override_is_returned_verbatim() {
        let base = PathBuf::from("base");
        let override_path = PathBuf::from("elsewhere/custom-vad");

        let resolved = resolve_vad_binary(&base, |name| {
            assert_eq!(name, VAD_BINARY_ENV);
            Some(override_path.to_string_lossy().into_owned())
        });

        assert_eq!(resolved, override_path);
    }

    #[test]
    fn unset_env_joins_base_dir_and_binary_name() {
        let base = PathBuf::from("base");

        let resolved = resolve_vad_binary(&base, |_name| None);

        assert_eq!(resolved, base.join(VAD_BINARY_NAME));
    }

    #[test]
    fn empty_env_override_falls_back_to_base_dir() {
        let base = PathBuf::from("base");

        let resolved = resolve_vad_binary(&base, |_name| Some(String::new()));

        assert_eq!(resolved, base.join(VAD_BINARY_NAME));
    }
}
