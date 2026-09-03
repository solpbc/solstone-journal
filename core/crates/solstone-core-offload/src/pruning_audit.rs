// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
use std::path::Path;

use chrono::{DateTime, FixedOffset};
use serde_json::Value;
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, OplogWriter, create_oplog_at},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditOutcome {
    pub recording_failure: Option<String>,
}

/// One best-effort audit sink owned by a single offload invocation.
pub struct PruneAuditWriter {
    writer: Option<OplogWriter>,
    recording_failure: Option<String>,
}

pub fn open_prune_audit(journal: &Path, opened: DateTime<FixedOffset>) -> PruneAuditWriter {
    let writer = JournalRoot::open(journal)
        .map_err(|error| error.to_string())
        .and_then(|root| {
            create_oplog_at(
                root,
                "offload",
                "raw-media-offload",
                OplogFormat::Jsonl,
                opened,
            )
            .map_err(|error| error.to_string())
        });
    match writer {
        Ok(writer) => PruneAuditWriter {
            writer: Some(writer),
            recording_failure: None,
        },
        Err(error) => PruneAuditWriter {
            writer: None,
            recording_failure: Some(format!("failed to create offload audit oplog: {error}")),
        },
    }
}

impl PruneAuditWriter {
    pub fn recording_failure(&self) -> Option<String> {
        self.recording_failure.clone()
    }
}

/// Append one structured best-effort audit row for one marked segment.
pub fn write_prune_audit(
    audit: &mut PruneAuditWriter,
    record: &Value,
    day: &str,
    message: &str,
) -> AuditOutcome {
    if let Some(error) = audit.recording_failure() {
        return AuditOutcome {
            recording_failure: Some(error),
        };
    }
    let Value::Object(mut row) = record.clone() else {
        let error = "offload audit record is not an object".to_owned();
        audit.recording_failure = Some(error.clone());
        return AuditOutcome {
            recording_failure: Some(error),
        };
    };
    row.insert("day".to_owned(), Value::String(day.to_owned()));
    row.insert("message".to_owned(), Value::String(message.to_owned()));

    let result = (|| {
        let log = audit
            .writer
            .as_mut()
            .ok_or_else(|| std::io::Error::other("offload audit writer unavailable"))?;
        serde_json::to_writer(&mut *log, &Value::Object(row)).map_err(std::io::Error::other)?;
        log.write_all(b"\n")?;
        log.flush()
    })();
    match result {
        Ok(()) => AuditOutcome::default(),
        Err(error) => {
            let error = format!("offload audit row write failed: {error}");
            audit.writer = None;
            audit.recording_failure = Some(error.clone());
            AuditOutcome {
                recording_failure: Some(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use chrono::{FixedOffset, TimeZone};
    use solstone_core_journal_io::{
        JournalRoot, MalformedPolicy,
        operational_log::{OplogFormat, catalog_oplogs},
        readers::read_jsonl_with_report,
    };

    use super::*;

    #[test]
    fn concurrent_offload_invocations_create_distinct_intact_oplogs() {
        let journal = tempfile::tempdir().unwrap();
        let root = Arc::new(journal.path().to_path_buf());
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
            .single()
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let workers = (0..2)
            .map(|worker| {
                let root = Arc::clone(&root);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut audit = open_prune_audit(&root, opened);
                    let outcome = write_prune_audit(
                        &mut audit,
                        &serde_json::json!({"event": "raw_media_offload", "worker": worker}),
                        "20260807",
                        "offload complete",
                    );
                    assert_eq!(outcome, AuditOutcome::default());
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }

        let day = opened.date_naive();
        let snapshot = catalog_oplogs(JournalRoot::open(journal.path()).unwrap(), &[day]).unwrap();
        let entries = snapshot
            .entries()
            .iter()
            .filter(|entry| {
                entry.name().source().display_slug() == "offload"
                    && entry.name().run().display_slug() == "raw-media-offload"
                    && entry.name().format() == OplogFormat::Jsonl
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        let mut workers = entries
            .iter()
            .flat_map(|entry| {
                read_jsonl_with_report::<Value>(
                    journal
                        .path()
                        .join("chronicle")
                        .join(entry.day())
                        .join("health")
                        .join(entry.leaf()),
                    Vec::new(),
                    MalformedPolicy::Raise,
                )
                .unwrap()
                .records
                .into_iter()
                .map(|record| record.value)
                .collect::<Vec<_>>()
            })
            .map(|record| record["worker"].as_i64().unwrap())
            .collect::<Vec<_>>();
        workers.sort_unstable();
        assert_eq!(workers, vec![0, 1]);
    }
}
