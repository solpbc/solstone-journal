// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

pub(crate) fn format_last_run(key: &str, journal_root: &Path, now: SystemTime) -> (String, bool) {
    let safe_name = key.replace(':', "--");
    let path = journal_root
        .join("talents")
        .join(format!("{safe_name}.log"));
    let result = (|| {
        let text = fs::read_to_string(path).ok()?;
        let mut lines = text.split_inclusive('\n');
        let first_line = lines.next()?;
        let first = serde_json::from_str::<Value>(first_line).ok()?;
        let first_ts = first.get("ts")?.as_f64()?;
        let now_seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs_f64();
        let mut age = format_seconds(now_seconds - first_ts / 1000.0, true);
        let last_line = lines.next_back();
        let mut failed = false;
        if let Some(last_line) = last_line {
            let last = serde_json::from_str::<Value>(last_line).ok()?;
            let last_ts = last.get("ts")?.as_f64()?;
            failed = last.get("event").and_then(Value::as_str) == Some("error");
            age.push_str(&format!(
                " ({})",
                format_seconds(last_ts / 1000.0 - first_ts / 1000.0, false)
            ));
        }
        Some((age, failed))
    })();
    result.unwrap_or_else(|| ("-".to_owned(), false))
}

fn format_seconds(seconds: f64, age: bool) -> String {
    let value = if seconds < 60.0 {
        format!("{}s", seconds as i64)
    } else if seconds < 3600.0 {
        format!("{}m", (seconds / 60.0) as i64)
    } else if seconds < 86400.0 {
        format!("{}h", (seconds / 3600.0) as i64)
    } else {
        format!("{}d", (seconds / 86400.0) as i64)
    };
    if age { format!("{value} ago") } else { value }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    #[test]
    fn boundaries_failures_and_single_events_match_reference_shapes() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join("talents")).expect("logs");
        let now = UNIX_EPOCH + Duration::from_secs(100_000);
        fs::write(
            root.path().join("talents/agent.log"),
            "{\"ts\":99400000}\n{\"ts\":99459000,\"event\":\"error\"}\n",
        )
        .expect("log");
        assert_eq!(
            format_last_run("agent", root.path(), now),
            ("10m ago (59s)".to_owned(), true)
        );
        fs::write(
            root.path().join("talents/single.log"),
            "{\"ts\":99400000}\n",
        )
        .expect("single");
        assert_eq!(
            format_last_run("single", root.path(), now),
            ("10m ago".to_owned(), false)
        );
        fs::write(
            root.path().join("talents/app--agent.log"),
            "{\"ts\":99400000}\n",
        )
        .expect("app log");
        assert_eq!(format_last_run("app:agent", root.path(), now).0, "10m ago");
        fs::write(root.path().join("talents/bad.log"), "not json\n").expect("bad log");
        assert_eq!(
            format_last_run("bad", root.path(), now),
            ("-".to_owned(), false)
        );
        fs::write(root.path().join("talents/missing-ts.log"), "{}\n").expect("missing timestamp");
        assert_eq!(
            format_last_run("missing-ts", root.path(), now),
            ("-".to_owned(), false)
        );
        for (name, seconds, expected) in [
            ("seconds", 59, "59s ago"),
            ("minutes", 60, "1m ago"),
            ("hours", 3_600, "1h ago"),
            ("days", 86_400, "1d ago"),
        ] {
            fs::write(
                root.path().join("talents").join(format!("{name}.log")),
                format!(r#"{{"ts":{}}}"#, (100_000 - seconds) * 1_000),
            )
            .expect("boundary log");
            assert_eq!(
                format_last_run(name, root.path(), now),
                (expected.to_owned(), false)
            );
        }
    }
}
