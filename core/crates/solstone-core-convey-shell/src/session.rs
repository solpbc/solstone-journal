// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Unestablished,
    Established,
    Corrupt { detail: String },
}

pub fn corrupt_config_detail(journal_root: &Path) -> String {
    format!(
        "your settings file at {}/config/journal.json couldn't be read. your settings were not changed. repair the file or restore config/journal.json from a backup, then try again.",
        journal_root.display()
    )
}

pub fn classify_session(journal_root: &Path) -> SessionState {
    if !journal_root.is_dir() {
        return SessionState::Unestablished;
    }
    let config_path = journal_root.join("config/journal.json");
    let bytes = match fs::read(&config_path) {
        Ok(bytes) => bytes,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.kind() == std::io::ErrorKind::NotADirectory =>
        {
            return SessionState::Unestablished;
        }
        Err(_) => {
            return SessionState::Corrupt {
                detail: corrupt_config_detail(journal_root),
            };
        }
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return SessionState::Corrupt {
                detail: corrupt_config_detail(journal_root),
            };
        }
    };
    let Some(object) = value.as_object() else {
        return SessionState::Corrupt {
            detail: corrupt_config_detail(journal_root),
        };
    };
    let completed_at = object
        .get("setup")
        .and_then(Value::as_object)
        .and_then(|setup| setup.get("completed_at"));
    match completed_at {
        Some(Value::Number(number)) if number.as_f64().is_some_and(|value| value > 0.0) => {
            SessionState::Established
        }
        _ => SessionState::Unestablished,
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionState, classify_session};
    use std::fs;

    struct TempDir(tempfile::TempDir);

    impl TempDir {
        fn new() -> Self {
            Self(tempfile::TempDir::new_in("/var/tmp").expect("temporary root creates"))
        }

        fn write(&self, bytes: &[u8]) {
            fs::create_dir_all(self.0.path().join("config")).expect("config directory creates");
            fs::write(self.0.path().join("config/journal.json"), bytes).expect("config writes");
        }
    }

    #[test]
    fn classifies_every_setup_completed_at_vector() {
        let vectors = [
            (
                br#"{"setup":{"completed_at":0}}"#.as_slice(),
                SessionState::Unestablished,
            ),
            (
                br#"{"setup":{"completed_at":-1}}"#.as_slice(),
                SessionState::Unestablished,
            ),
            (
                br#"{"setup":{"completed_at":true}}"#.as_slice(),
                SessionState::Unestablished,
            ),
            (
                br#"{"setup":{"completed_at":"1767225600"}}"#.as_slice(),
                SessionState::Unestablished,
            ),
            (br#"{"setup":{}}"#.as_slice(), SessionState::Unestablished),
            (br#"{}"#.as_slice(), SessionState::Unestablished),
            (
                br#"{"setup":{"completed_at":1}}"#.as_slice(),
                SessionState::Established,
            ),
            (
                br#"{"setup":{"completed_at":0.5}}"#.as_slice(),
                SessionState::Established,
            ),
        ];
        for (bytes, expected) in vectors {
            let temporary = TempDir::new();
            temporary.write(bytes);
            assert_eq!(classify_session(temporary.0.path()), expected);
        }
    }

    #[test]
    fn missing_config_is_unestablished_and_invalid_shapes_are_corrupt() {
        let missing = TempDir::new();
        assert_eq!(
            classify_session(missing.0.path()),
            SessionState::Unestablished
        );

        for bytes in [br#"[]"#.as_slice(), br#"{"setup": "bad""#.as_slice()] {
            let temporary = TempDir::new();
            temporary.write(bytes);
            assert!(matches!(
                classify_session(temporary.0.path()),
                SessionState::Corrupt { .. }
            ));
        }
    }
}
