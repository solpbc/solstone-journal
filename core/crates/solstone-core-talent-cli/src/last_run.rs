// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use solstone_core_journal_io::cortex_use::{
    census_cortex_namespace, create_or_admit_cortex_namespace, talent_directory_name,
};
use solstone_core_journal_io::{JournalEntryKind, JournalRoot};

const MAXIMUM_CENSUS_ENTRIES: usize = 64 * 1024;

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum LastRunOutcome {
    NoRuns,
    Unavailable,
    Found { display: String, failed: bool },
}

pub(crate) fn format_last_run(key: &str, journal_root: &Path, now: SystemTime) -> LastRunOutcome {
    if !is_real_directory(&journal_root.join("talents")) {
        return LastRunOutcome::NoRuns;
    }
    // Cortex admission creates both fixed directories on miss. A talent directory
    // without health is not an admitted Cortex namespace, so do not let a list
    // read create the missing sibling.
    if !is_real_directory(&journal_root.join("health")) {
        return LastRunOutcome::Unavailable;
    }
    let Ok(root) = JournalRoot::open(journal_root) else {
        return LastRunOutcome::Unavailable;
    };
    let Ok(authority) = create_or_admit_cortex_namespace(root) else {
        return LastRunOutcome::Unavailable;
    };
    let Ok(census) = census_cortex_namespace(authority, MAXIMUM_CENSUS_ENTRIES) else {
        return LastRunOutcome::Unavailable;
    };
    let directory_name = talent_directory_name(key);
    let Some(talent) = census
        .talents()
        .iter()
        .find(|talent| talent.name() == OsStr::new(&directory_name))
    else {
        return LastRunOutcome::NoRuns;
    };
    let Some((_, leaf)) = talent
        .entries()
        .iter()
        .filter_map(|entry| {
            if entry.kind() != JournalEntryKind::RegularFile
                // The lifecycle parser also gives an active filename a completed
                // projection (`<id>_active`), so reject active leaves explicitly.
                || entry.projections().active().is_some()
            {
                return None;
            }
            let id = entry.projections().completed()?.parse::<u64>().ok()?;
            Some((id, entry.name()))
        })
        .max_by_key(|(id, _)| *id)
    else {
        return LastRunOutcome::NoRuns;
    };
    let path = journal_root
        .join("talents")
        .join(&directory_name)
        .join(leaf);
    match format_run(&path, now) {
        Some((display, failed)) => LastRunOutcome::Found { display, failed },
        None => LastRunOutcome::Unavailable,
    }
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

fn format_run(path: &Path, now: SystemTime) -> Option<(String, bool)> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.split_inclusive('\n');
    let first_line = lines.next()?;
    let first = serde_json::from_str::<Value>(first_line).ok()?;
    let first_ts = first.get("ts")?.as_f64()?;
    let now_seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs_f64();
    let mut age = format_age(now_seconds - first_ts / 1000.0);
    let last_line = lines.next_back();
    let mut failed = false;
    if let Some(last_line) = last_line {
        let last = serde_json::from_str::<Value>(last_line).ok()?;
        let last_ts = last.get("ts")?.as_f64()?;
        failed = last.get("event").and_then(Value::as_str) == Some("error");
        age.push_str(&format!(
            " ({})",
            format_duration(last_ts / 1000.0 - first_ts / 1000.0)
        ));
    }
    Some((age, failed))
}

fn format_age(seconds: f64) -> String {
    let value = if seconds < 60.0 {
        format!("{}s", seconds as i64)
    } else if seconds < 3600.0 {
        format!("{}m", (seconds / 60.0) as i64)
    } else if seconds < 86400.0 {
        format!("{}h", (seconds / 3600.0) as i64)
    } else {
        format!("{}d", (seconds / 86400.0) as i64)
    };
    format!("{value} ago")
}

fn format_duration(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{}s", seconds as i64)
    } else if seconds < 3600.0 {
        format!("{}m", (seconds / 60.0) as i64)
    } else {
        format!("{}h", (seconds / 3600.0) as i64)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    #[test]
    fn boundaries_failures_and_single_events_match_reference_shapes() {
        let root = tempfile::tempdir().expect("tempdir");
        let now = UNIX_EPOCH + Duration::from_secs(100_000);
        write_run(
            root.path(),
            "agent",
            "1",
            "{\"ts\":99400000}\n{\"ts\":99459000,\"event\":\"error\"}\n",
        )
        .expect("log");
        assert_eq!(
            format_last_run("agent", root.path(), now),
            LastRunOutcome::Found {
                display: "10m ago (59s)".to_owned(),
                failed: true,
            }
        );
        write_run(root.path(), "single", "1", "{\"ts\":99400000}\n").expect("single");
        assert_eq!(
            format_last_run("single", root.path(), now),
            LastRunOutcome::Found {
                display: "10m ago".to_owned(),
                failed: false,
            }
        );
        write_run(root.path(), "app:agent", "1", "{\"ts\":99400000}\n").expect("app log");
        assert_eq!(
            format_last_run("app:agent", root.path(), now),
            LastRunOutcome::Found {
                display: "10m ago".to_owned(),
                failed: false,
            }
        );
        write_run(root.path(), "long", "1", "{\"ts\":0}\n{\"ts\":90000000}\n")
            .expect("long duration log");
        assert_eq!(
            format_last_run("long", root.path(), now),
            LastRunOutcome::Found {
                display: "1d ago (25h)".to_owned(),
                failed: false,
            }
        );
        write_run(root.path(), "bad", "1", "not json\n").expect("bad log");
        assert_eq!(
            format_last_run("bad", root.path(), now),
            LastRunOutcome::Unavailable
        );
        write_run(root.path(), "missing-ts", "1", "{}\n").expect("missing timestamp");
        assert_eq!(
            format_last_run("missing-ts", root.path(), now),
            LastRunOutcome::Unavailable
        );
        for (name, seconds, expected) in [
            ("seconds", 59, "59s ago"),
            ("minutes", 60, "1m ago"),
            ("hours", 3_600, "1h ago"),
            ("days", 86_400, "1d ago"),
        ] {
            write_run(
                root.path(),
                name,
                "1",
                format!(r#"{{"ts":{}}}"#, (100_000 - seconds) * 1_000),
            )
            .expect("boundary log");
            assert_eq!(
                format_last_run(name, root.path(), now),
                LastRunOutcome::Found {
                    display: expected.to_owned(),
                    failed: false,
                }
            );
        }
    }

    #[test]
    fn selects_the_greatest_completed_numeric_run() {
        let root = tempfile::tempdir().expect("tempdir");
        let now = UNIX_EPOCH + Duration::from_secs(100_000);
        write_run(root.path(), "agent", "10", "{\"ts\":1000}\n").expect("lower run");
        write_run(root.path(), "agent", "20", "{\"ts\":99000000}\n").expect("latest run");
        write_run(root.path(), "agent", "30_active", "{\"ts\":0}\n").expect("active run");
        write_run(root.path(), "agent", "not-numeric", "{\"ts\":0}\n").expect("other leaf");

        assert_eq!(
            format_last_run("agent", root.path(), now),
            LastRunOutcome::Found {
                display: "16m ago".to_owned(),
                failed: false,
            }
        );
    }

    #[test]
    fn malformed_greatest_run_is_unavailable_without_falling_back() {
        let root = tempfile::tempdir().expect("tempdir");
        let now = UNIX_EPOCH + Duration::from_secs(100_000);
        write_run(root.path(), "agent", "10", "{\"ts\":99000000}\n").expect("lower run");
        write_run(root.path(), "agent", "20", "not json\n").expect("malformed run");

        assert_eq!(
            format_last_run("agent", root.path(), now),
            LastRunOutcome::Unavailable
        );
    }

    #[test]
    fn missing_talent_directory_is_no_runs() {
        let root = tempfile::tempdir().expect("tempdir");
        prepare_namespace(root.path());

        assert_eq!(
            format_last_run("agent", root.path(), UNIX_EPOCH),
            LastRunOutcome::NoRuns
        );
    }

    #[test]
    fn missing_talents_directory_does_not_create_a_cortex_namespace() {
        let root = tempfile::tempdir().expect("tempdir");

        assert_eq!(
            format_last_run("agent", root.path(), UNIX_EPOCH),
            LastRunOutcome::NoRuns
        );
        assert!(!root.path().join("talents").exists());
        assert!(!root.path().join("health").exists());
    }

    fn prepare_namespace(root: &Path) {
        fs::create_dir_all(root.join("health")).expect("health");
        fs::create_dir_all(root.join("talents")).expect("talents");
    }

    fn write_run(
        root: &Path,
        key: &str,
        id: &str,
        contents: impl AsRef<[u8]>,
    ) -> std::io::Result<()> {
        prepare_namespace(root);
        let directory = root.join("talents").join(talent_directory_name(key));
        fs::create_dir_all(&directory)?;
        fs::write(directory.join(format!("{id}.jsonl")), contents)
    }
}
