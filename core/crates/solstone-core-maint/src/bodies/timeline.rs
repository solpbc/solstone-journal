// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::registry::{MaintBodyContext, MaintBodyResult};
use serde_json::json;
use solstone_core_system::schedule::{ScheduleMutation, mutate_schedule_entries};

pub fn migrate_rollup_schedules(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    let path = c.journal.join("config/schedules.json");
    if !path.exists() {
        return MaintBodyResult {
            stdout: vec![
                "Summary".into(),
                "  removed:   0".into(),
                "  preserved: 0".into(),
                "  absent:    2".into(),
            ],
            exit_code: 0,
        };
    }
    let expected = [
        (
            "timeline-rollup-day",
            json!({"cmd":["sol","call","timeline","rollup-day"],"every":"daily","max_runtime":"30m"}),
        ),
        (
            "timeline-rollup-master",
            json!({"cmd":["sol","call","timeline","rollup-master"],"every":"daily","max_runtime":"30m"}),
        ),
    ];
    match mutate_schedule_entries(&path, |raw| {
        let mut removed = 0;
        let mut preserved = 0;
        let mut absent = 0;
        for (name, value) in &expected {
            match raw.get(*name) {
                None => absent += 1,
                Some(existing) if existing == value => {
                    removed += 1;
                    if !c.dry_run {
                        raw.remove(*name);
                    }
                }
                Some(_) => preserved += 1,
            }
        }
        ScheduleMutation {
            changed: !c.dry_run && removed > 0,
            value: (removed, preserved, absent),
        }
    }) {
        Ok((removed, preserved, absent)) => MaintBodyResult {
            stdout: vec![
                "Summary".into(),
                format!("  removed:   {removed}"),
                format!("  preserved: {preserved}"),
                format!("  absent:    {absent}"),
            ],
            exit_code: 0,
        },
        Err(error) => MaintBodyResult {
            stdout: vec![error.to_string()],
            exit_code: 1,
        },
    }
}

pub fn retired_segment_summary_model(_: &MaintBodyContext<'_>) -> MaintBodyResult {
    MaintBodyResult {
        stdout: vec!["Skipped retired migration.".into()],
        exit_code: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;
    #[test]
    fn retired_body_is_unconditional() {
        let context = MaintBodyContext {
            journal: Path::new("/unused"),
            dry_run: false,
            verbose: false,
            task_name: Some("timeline:002_register_segment_summary_model"),
        };
        assert_eq!(retired_segment_summary_model(&context).exit_code, 0);
    }

    #[test]
    fn rollup_migration_removes_exact_entry_and_preserves_owner_divergence() {
        let journal = tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/schedules.json"),
            serde_json::to_vec(&json!({
                "timeline-rollup-day":{"cmd":["sol","call","timeline","rollup-day"],"every":"daily","max_runtime":"30m"},
                "timeline-rollup-master":{"cmd":["custom","rollup"],"every":"weekly"},
                "unrelated":{"keep":true}
            })).unwrap(),
        ).unwrap();
        let result = migrate_rollup_schedules(&MaintBodyContext {
            journal: journal.path(),
            dry_run: false,
            verbose: false,
            task_name: None,
        });
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.iter().any(|line| line == "  removed:   1"));
        assert!(result.stdout.iter().any(|line| line == "  preserved: 1"));
        let stored: serde_json::Value = serde_json::from_slice(
            &std::fs::read(journal.path().join("config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert!(stored.get("timeline-rollup-day").is_none());
        assert_eq!(
            stored["timeline-rollup-master"]["cmd"],
            json!(["custom", "rollup"])
        );
        assert_eq!(stored["unrelated"], json!({"keep":true}));
    }
}
