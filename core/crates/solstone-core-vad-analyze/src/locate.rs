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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir {
        root: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "solstone-vad-locate-test-{}-{nonce}",
                process::id()
            ));
            fs::create_dir(&root).expect("create test dir");
            Self { root }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn explicit_env_override_is_returned_verbatim() {
        let dir = TestDir::new();
        let override_path = dir.root.join("elsewhere/custom-vad");

        let resolved = resolve_vad_binary(&dir.root, |name| {
            assert_eq!(name, VAD_BINARY_ENV);
            Some(override_path.to_string_lossy().into_owned())
        });

        assert_eq!(resolved, override_path);
    }

    #[test]
    fn unset_env_joins_base_dir_and_binary_name() {
        let dir = TestDir::new();

        let resolved = resolve_vad_binary(&dir.root, |_name| None);

        assert_eq!(resolved, dir.root.join(VAD_BINARY_NAME));
    }

    #[test]
    fn empty_env_override_falls_back_to_base_dir() {
        let dir = TestDir::new();

        let resolved = resolve_vad_binary(&dir.root, |_name| Some(String::new()));

        assert_eq!(resolved, dir.root.join(VAD_BINARY_NAME));
    }
}
