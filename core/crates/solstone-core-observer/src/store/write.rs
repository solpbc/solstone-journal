// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

use super::paths::observer_path;
use super::record::ObserverRecord;

pub fn save_observer(journal_root: &Path, record: &ObserverRecord) -> Result<(), String> {
    let contents =
        serde_json::to_string_pretty(record.value()).map_err(|error| error.to_string())?;
    atomic_replace(
        observer_path(journal_root, &record.prefix()),
        contents.as_bytes(),
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::paths::observer_path;
    use crate::test_support::reserve_temp_path;
    use serde_json::json;
    use std::fs;

    #[test]
    fn save_is_pretty_has_no_trailing_newline_and_preserves_fields() {
        let root = reserve_temp_path("observer-write");
        let record = ObserverRecord::from_value(
            json!({"key":"abcdefgh123", "name":"one", "unknown":{"keep":true}}),
        )
        .expect("record");
        save_observer(&root, &record).expect("save");
        let bytes = fs::read(observer_path(&root, "abcdefgh")).expect("read");
        assert!(!bytes.ends_with(b"\n"));
        assert!(
            String::from_utf8(bytes)
                .expect("utf8")
                .contains("  \"unknown\"")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(observer_path(&root, "abcdefgh"))
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).expect("cleanup");
    }
}
