// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use chrono::NaiveDateTime;
use solstone_core_journal_io::day_path;
use solstone_core_system::operational_log_parse::ParsedHealthLogRow;
use solstone_core_system::operational_log_parse::parse_health_log_row;
use solstone_core_system_health::GrepPattern;

use crate::error::CollectError;
use crate::read::{
    DayLogDirectoryOps, HealthDirectoryState, ProbeOps, TailFileOpener, list_day_log_symlinks,
    probe_health_directory, tail_ordinary_text, tail_reverse_text, tail_slice,
};

/// Fully resolved options for a one-shot operational-log read.
#[derive(Debug, Clone)]
pub struct HealthLogsQuery {
    pub count: i64,
    pub since: Option<NaiveDateTime>,
    pub service: Option<String>,
    pub grep: Option<GrepPattern>,
}

/// Collect and order today's operational logs without rendering them.
pub fn collect_health_logs(
    journal_root: &Path,
    now: NaiveDateTime,
    query: &HealthLogsQuery,
    probe_ops: &dyn ProbeOps,
    tail_opener: &dyn TailFileOpener,
    dir_ops: &dyn DayLogDirectoryOps,
) -> Result<Vec<ParsedHealthLogRow>, CollectError> {
    let day = now.format("%Y%m%d").to_string();
    let health_dir = day_path(journal_root, Some(&day), false)
        .expect("derived day is always a valid YYYYMMDD key")
        .join("health");
    let has_filters = query.since.is_some()
        || query
            .service
            .as_deref()
            .is_some_and(|service| !service.is_empty())
        || query.grep.is_some();
    let mut rows = Vec::new();

    match probe_health_directory(&health_dir, probe_ops)
        .map_err(CollectError::HealthDirectoryProbe)?
    {
        HealthDirectoryState::Directory => {
            for name in
                list_day_log_symlinks(&health_dir, dir_ops).map_err(CollectError::Enumeration)?
            {
                let count = if has_filters { 0 } else { query.count };
                for raw in tail_ordinary_text(&health_dir.join(name), count, tail_opener)
                    .map_err(CollectError::InvalidUtf8)?
                {
                    if let Some(row) = parse_health_log_row(&raw)
                        && (!has_filters || matches_filters(&row, query))
                    {
                        rows.push(row);
                    }
                }
            }
        }
        HealthDirectoryState::Absent | HealthDirectoryState::NotADirectory => {}
    }

    if !has_filters {
        let supervisor_path = journal_root.join("health").join("supervisor.log");
        match probe_health_directory(&supervisor_path, probe_ops)
            .map_err(CollectError::SupervisorProbe)?
        {
            HealthDirectoryState::Absent => {}
            HealthDirectoryState::Directory | HealthDirectoryState::NotADirectory => {
                for raw in tail_reverse_text(&supervisor_path, query.count, tail_opener) {
                    if let Some(row) = parse_health_log_row(&raw) {
                        rows.push(row);
                    }
                }
            }
        }
    }

    rows.sort_by_key(|row| row.timestamp);
    Ok(if has_filters {
        tail_slice(rows, query.count)
    } else {
        rows
    })
}

fn matches_filters(row: &ParsedHealthLogRow, query: &HealthLogsQuery) -> bool {
    if let Some(since) = query.since
        && row.timestamp < since
    {
        return false;
    }
    if let Some(service) = query.service.as_deref()
        && !service.is_empty()
        && row.service != service
    {
        return false;
    }
    if let Some(grep) = &query.grep
        && !grep.is_match(&row.raw)
    {
        return false;
    }
    true
}

#[cfg(all(test, unix))]
mod tests {
    use std::cell::RefCell;
    use std::io;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    use chrono::NaiveDateTime;
    use solstone_core_system::operational_log_parse::parse_health_log_since;
    use solstone_core_system_health::compile_grep_pattern;
    use tempfile::TempDir;

    use super::*;
    use crate::read::{DayLogEntry, ProbeKind, StdDayLogDirectoryOps, StdTailFileOpener};

    #[derive(Clone, Copy)]
    enum ProbeResponse {
        Directory,
        Other,
        Absent,
        Error,
    }

    struct RecordingProbe {
        day: ProbeResponse,
        supervisor: ProbeResponse,
        paths: RefCell<Vec<PathBuf>>,
    }

    impl RecordingProbe {
        fn paths(&self) -> Vec<PathBuf> {
            self.paths.borrow().clone()
        }
    }

    impl ProbeOps for RecordingProbe {
        fn metadata_kind(&self, path: &Path) -> io::Result<ProbeKind> {
            self.paths.borrow_mut().push(path.to_path_buf());
            let response = if path
                .file_name()
                .is_some_and(|name| name == "supervisor.log")
            {
                self.supervisor
            } else {
                self.day
            };
            match response {
                ProbeResponse::Directory => Ok(ProbeKind::Directory),
                ProbeResponse::Other => Ok(ProbeKind::Other),
                ProbeResponse::Absent => Err(io::Error::new(io::ErrorKind::NotFound, "absent")),
                ProbeResponse::Error => Err(io::Error::other("metadata failed")),
            }
        }
    }

    struct FailingDirectoryOps;

    impl DayLogDirectoryOps for FailingDirectoryOps {
        fn read_dir(
            &self,
            _path: &Path,
        ) -> io::Result<Box<dyn Iterator<Item = io::Result<DayLogEntry>>>> {
            Err(io::Error::other("enumeration failed"))
        }

        fn is_symlink(&self, _entry: &DayLogEntry) -> io::Result<bool> {
            unreachable!("read_dir always fails")
        }
    }

    fn now() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-01-01 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn row(timestamp: &str, service: &str, message: &str) -> String {
        format!("{timestamp} [{service}:stdout] {message}")
    }

    fn health_dir(root: &Path, now: NaiveDateTime) -> PathBuf {
        root.join("chronicle")
            .join(now.format("%Y%m%d").to_string())
            .join("health")
    }

    fn symlink_log(root: &Path, health: &Path, name: &str, content: &[u8]) {
        let source = root.join(format!("source-{name}"));
        std::fs::write(&source, content).unwrap();
        symlink(source, health.join(name)).unwrap();
    }

    fn query(count: i64) -> HealthLogsQuery {
        HealthLogsQuery {
            count,
            since: None,
            service: None,
            grep: None,
        }
    }

    fn standard_probe() -> RecordingProbe {
        RecordingProbe {
            day: ProbeResponse::Directory,
            supervisor: ProbeResponse::Absent,
            paths: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn service_truthiness_controls_tail_count_and_supervisor_inclusion() {
        let root = TempDir::new().unwrap();
        let health = health_dir(root.path(), now());
        std::fs::create_dir_all(&health).unwrap();
        symlink_log(
            root.path(),
            &health,
            "day.log",
            format!(
                "{}\n{}\n{}\n",
                row("2026-01-01 10:00:00", "x", "first"),
                row("2026-01-01 10:01:00", "x", "second"),
                row("2026-01-01 10:02:00", "other", "excluded")
            )
            .as_bytes(),
        );
        let supervisor = root.path().join("health");
        std::fs::create_dir_all(&supervisor).unwrap();
        std::fs::write(
            supervisor.join("supervisor.log"),
            format!(
                "{}\n{}\n",
                row("2026-01-01 10:02:00", "supervisor", "first"),
                row("2026-01-01 10:03:00", "supervisor", "second")
            ),
        )
        .unwrap();

        for service in [None, Some(String::new())] {
            let probe = RecordingProbe {
                day: ProbeResponse::Directory,
                supervisor: ProbeResponse::Other,
                paths: RefCell::new(Vec::new()),
            };
            let mut options = query(1);
            options.service = service;
            let rows = collect_health_logs(
                root.path(),
                now(),
                &options,
                &probe,
                &StdTailFileOpener,
                &StdDayLogDirectoryOps,
            )
            .unwrap();
            assert_eq!(
                rows.iter()
                    .map(|row| row.message.as_str())
                    .collect::<Vec<_>>(),
                ["excluded", "second"]
            );
            assert!(
                probe
                    .paths()
                    .iter()
                    .any(|path| path.ends_with("health/supervisor.log"))
            );
        }

        let probe = RecordingProbe {
            day: ProbeResponse::Directory,
            supervisor: ProbeResponse::Error,
            paths: RefCell::new(Vec::new()),
        };
        let mut options = query(1);
        options.service = Some("x".to_owned());
        let rows = collect_health_logs(
            root.path(),
            now(),
            &options,
            &probe,
            &StdTailFileOpener,
            &StdDayLogDirectoryOps,
        )
        .unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.message.as_str())
                .collect::<Vec<_>>(),
            ["second"]
        );
        assert!(
            !probe
                .paths()
                .iter()
                .any(|path| path.ends_with("health/supervisor.log"))
        );
    }

    #[test]
    fn filters_read_day_logs_in_full_and_apply_each_predicate_to_raw_rows() {
        let root = TempDir::new().unwrap();
        let health = health_dir(root.path(), now());
        std::fs::create_dir_all(&health).unwrap();
        let content = format!(
            "{}\n{}\n{}\n{}\n",
            row("2026-01-01 09:00:00", "x", "needle old"),
            row("2026-01-01 10:00:00", "x", "needle new"),
            row("2026-01-01 10:01:00", "other", "needle other"),
            row("2026-01-01 10:02:00", "x", "plain new")
        );
        symlink_log(root.path(), &health, "day.log", content.as_bytes());

        let cases = [
            (
                HealthLogsQuery {
                    count: 0,
                    since: Some(
                        NaiveDateTime::parse_from_str("2026-01-01 10:00:00", "%Y-%m-%d %H:%M:%S")
                            .unwrap(),
                    ),
                    service: None,
                    grep: None,
                },
                vec!["needle new", "needle other", "plain new"],
            ),
            (
                HealthLogsQuery {
                    count: 0,
                    since: None,
                    service: Some("x".to_owned()),
                    grep: None,
                },
                vec!["needle old", "needle new", "plain new"],
            ),
            (
                HealthLogsQuery {
                    count: 0,
                    since: None,
                    service: None,
                    grep: Some(compile_grep_pattern("needle").unwrap()),
                },
                vec!["needle old", "needle new", "needle other"],
            ),
            (
                HealthLogsQuery {
                    count: 0,
                    since: Some(
                        NaiveDateTime::parse_from_str("2026-01-01 10:00:00", "%Y-%m-%d %H:%M:%S")
                            .unwrap(),
                    ),
                    service: Some("x".to_owned()),
                    grep: Some(compile_grep_pattern("needle").unwrap()),
                },
                vec!["needle new"],
            ),
        ];

        for (options, expected) in cases {
            let probe = standard_probe();
            let rows = collect_health_logs(
                root.path(),
                now(),
                &options,
                &probe,
                &StdTailFileOpener,
                &StdDayLogDirectoryOps,
            )
            .unwrap();
            assert_eq!(
                rows.iter()
                    .map(|row| row.message.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                probe.paths().len(),
                1,
                "filtered path must not probe supervisor"
            );
        }
    }

    #[test]
    fn supervisor_gate_covers_absent_present_metadata_error_and_disappearance() {
        let root = TempDir::new().unwrap();
        let health = health_dir(root.path(), now());
        std::fs::create_dir_all(&health).unwrap();
        let supervisor_dir = root.path().join("health");
        std::fs::create_dir_all(&supervisor_dir).unwrap();
        std::fs::write(
            supervisor_dir.join("supervisor.log"),
            format!("{}\n", row("2026-01-01 10:00:00", "supervisor", "present")),
        )
        .unwrap();

        for response in [ProbeResponse::Directory, ProbeResponse::Other] {
            let probe = RecordingProbe {
                day: ProbeResponse::Directory,
                supervisor: response,
                paths: RefCell::new(Vec::new()),
            };
            let rows = collect_health_logs(
                root.path(),
                now(),
                &query(10),
                &probe,
                &StdTailFileOpener,
                &StdDayLogDirectoryOps,
            )
            .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].message, "present");
        }

        let absent = RecordingProbe {
            day: ProbeResponse::Directory,
            supervisor: ProbeResponse::Absent,
            paths: RefCell::new(Vec::new()),
        };
        assert!(
            collect_health_logs(
                root.path(),
                now(),
                &query(10),
                &absent,
                &StdTailFileOpener,
                &StdDayLogDirectoryOps,
            )
            .unwrap()
            .is_empty()
        );

        let error = RecordingProbe {
            day: ProbeResponse::Directory,
            supervisor: ProbeResponse::Error,
            paths: RefCell::new(Vec::new()),
        };
        assert!(matches!(
            collect_health_logs(
                root.path(),
                now(),
                &query(10),
                &error,
                &StdTailFileOpener,
                &StdDayLogDirectoryOps,
            ),
            Err(CollectError::SupervisorProbe(_))
        ));

        std::fs::remove_file(supervisor_dir.join("supervisor.log")).unwrap();
        let disappeared = RecordingProbe {
            day: ProbeResponse::Directory,
            supervisor: ProbeResponse::Other,
            paths: RefCell::new(Vec::new()),
        };
        assert!(
            collect_health_logs(
                root.path(),
                now(),
                &query(10),
                &disappeared,
                &StdTailFileOpener,
                &StdDayLogDirectoryOps,
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn supplied_now_selects_the_same_day_as_pre_resolved_since() {
        let root = TempDir::new().unwrap();
        let supplied_now =
            NaiveDateTime::parse_from_str("2026-01-01 23:59:59", "%Y-%m-%d %H:%M:%S").unwrap();
        let since = parse_health_log_since("2h", supplied_now).unwrap();
        assert_eq!(
            since,
            NaiveDateTime::parse_from_str("2026-01-01 21:59:59", "%Y-%m-%d %H:%M:%S").unwrap()
        );
        let probe = standard_probe();
        let mut options = query(0);
        options.since = Some(since);
        let rows = collect_health_logs(
            root.path(),
            supplied_now,
            &options,
            &probe,
            &StdTailFileOpener,
            &StdDayLogDirectoryOps,
        )
        .unwrap();
        assert!(rows.is_empty());
        assert!(probe.paths()[0].ends_with("chronicle/20260101/health"));
    }

    #[test]
    fn malformed_rows_are_skipped_and_timestamp_ties_keep_insertion_order() {
        let root = TempDir::new().unwrap();
        let health = health_dir(root.path(), now());
        std::fs::create_dir_all(&health).unwrap();
        symlink_log(
            root.path(),
            &health,
            "a.log",
            format!(
                "garbage\n{}\n{}\n",
                row("2026-01-01 10:00:00", "a", "a-one"),
                row("2026-01-01 10:00:00", "a", "a-two")
            )
            .as_bytes(),
        );
        symlink_log(
            root.path(),
            &health,
            "b.log",
            format!("{}\n", row("2026-01-01 10:00:00", "b", "b-one")).as_bytes(),
        );
        let supervisor = root.path().join("health");
        std::fs::create_dir_all(&supervisor).unwrap();
        std::fs::write(
            supervisor.join("supervisor.log"),
            format!("{}\n", row("2026-01-01 10:00:00", "supervisor", "sup-one")),
        )
        .unwrap();
        let probe = RecordingProbe {
            day: ProbeResponse::Directory,
            supervisor: ProbeResponse::Other,
            paths: RefCell::new(Vec::new()),
        };
        let rows = collect_health_logs(
            root.path(),
            now(),
            &query(10),
            &probe,
            &StdTailFileOpener,
            &StdDayLogDirectoryOps,
        )
        .unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.message.as_str())
                .collect::<Vec<_>>(),
            ["a-one", "a-two", "b-one", "sup-one"]
        );
    }

    #[test]
    fn enumeration_and_invalid_utf8_fail_before_rendering() {
        let root = TempDir::new().unwrap();
        let probe = standard_probe();
        assert!(matches!(
            collect_health_logs(
                root.path(),
                now(),
                &query(0),
                &probe,
                &StdTailFileOpener,
                &FailingDirectoryOps,
            ),
            Err(CollectError::Enumeration(_))
        ));

        let health = health_dir(root.path(), now());
        std::fs::create_dir_all(&health).unwrap();
        symlink_log(root.path(), &health, "bad.log", b"\xff");
        let probe = standard_probe();
        assert!(matches!(
            collect_health_logs(
                root.path(),
                now(),
                &query(0),
                &probe,
                &StdTailFileOpener,
                &StdDayLogDirectoryOps,
            ),
            Err(CollectError::InvalidUtf8(_))
        ));
    }
}
