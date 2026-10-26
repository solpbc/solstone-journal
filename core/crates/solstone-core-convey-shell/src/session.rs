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
    let recovery_paused =
        solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(journal_root);
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
        Some(Value::Number(number))
            if !recovery_paused && number.as_f64().is_some_and(|value| value > 0.0) =>
        {
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

    #[test]
    fn pending_restore_does_not_promote_a_completed_config() {
        use solstone_core_journal_io::whole_journal_restore::{
            WholeJournalRestoreSource, publish_whole_journal_restore,
        };
        let temporary = TempDir::new();
        temporary.write(br#"{"setup":{"completed_at":1}}"#);
        publish_whole_journal_restore(
            temporary.0.path(),
            &WholeJournalRestoreSource::Byo {
                repository: "repo".into(),
                backend: "s3".into(),
            },
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "/journal",
        )
        .expect("obligation");
        assert_eq!(
            classify_session(temporary.0.path()),
            SessionState::Unestablished
        );
        let path = temporary.0.path().join("health/whole-journal-restore.json");
        let saved = fs::read(&path).expect("obligation bytes");
        fs::write(&path, b"{").expect("replace");
        assert_eq!(
            classify_session(temporary.0.path()),
            SessionState::Unestablished
        );
        fs::write(&path, saved).expect("restore obligation");
        fs::write(temporary.0.path().join("config/journal.json"), b"{").expect("corrupt");
        assert!(matches!(
            classify_session(temporary.0.path()),
            SessionState::Corrupt { .. }
        ));
    }

    #[test]
    fn held_target_does_not_promote_before_publication_and_resumes_after_release() {
        use solstone_core_journal_io::whole_journal_restore::{
            hold_target_for_restore, release_target_for_restore,
        };
        let temporary = TempDir::new();
        temporary.write(br#"{"setup":{"completed_at":1}}"#);
        hold_target_for_restore(temporary.0.path());
        assert_eq!(
            classify_session(temporary.0.path()),
            SessionState::Unestablished
        );
        release_target_for_restore(temporary.0.path());
        assert_eq!(
            classify_session(temporary.0.path()),
            SessionState::Established
        );
    }
}
