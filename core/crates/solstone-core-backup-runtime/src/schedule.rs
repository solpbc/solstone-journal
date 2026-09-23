// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The backup schedule an owner is shown.

use std::path::Path;

use serde_json::{Value, json};

/// The effective backup cadence, read from the scheduler.
///
/// The scheduler owns effective cadence. Backup configuration owns retention,
/// credentials and operation history, but cannot report whether a job will run:
/// its `schedule` field is a default that no scheduler reads. `Null` means the
/// schedule could not be read, which is not the same as disabled.
pub fn effective_backup_schedule(journal: &Path) -> Value {
    match solstone_core_system::schedule::read_enabled_schedule_entry(
        &journal.join("config/schedules.json"),
        "maintenance:backup:run",
    ) {
        Ok((Some(entry), _)) => json!({"enabled": true, "every": entry.every}),
        Ok((None, diagnostics)) if diagnostics.is_empty() => {
            json!({"enabled": false, "every": null})
        }
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn effective_schedule_comes_from_scheduler_and_unknown_is_not_disabled() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            super::effective_backup_schedule(root.path()),
            serde_json::json!({"enabled":false,"every":null})
        );
        assert!(
            !root.path().join("config").exists(),
            "status must not initialize schedules"
        );
        std::fs::create_dir(root.path().join("config")).unwrap();
        let path = root.path().join("config/schedules.json");
        for (raw, expected, stays_in_place) in [
            (
                r#"{"maintenance:backup:run":{"cmd":["journal","maintenance","run","backup:run"],"every":"hourly","enabled":true}}"#,
                serde_json::json!({"enabled":true,"every":"hourly"}),
                true,
            ),
            (
                r#"{"maintenance:backup:run":{"cmd":["journal","maintenance","run","backup:run"],"every":"hourly","enabled":false}}"#,
                serde_json::json!({"enabled":false,"every":null}),
                true,
            ),
            ("broken", serde_json::Value::Null, false),
        ] {
            std::fs::write(&path, raw).unwrap();
            assert_eq!(super::effective_backup_schedule(root.path()), expected);
            if stays_in_place {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
            }
        }
    }
}
