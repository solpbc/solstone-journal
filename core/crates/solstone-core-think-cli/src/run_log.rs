// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone};
use serde_json::{Map, Value};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, OplogWriter, create_oplog_at},
};

use crate::args::ThinkArgs;

pub(crate) fn mode(args: &ThinkArgs) -> &'static str {
    if args.activity.is_some() {
        "activity"
    } else if args.flush {
        "flush"
    } else if args.segments {
        "segments"
    } else if args.segment.is_some() {
        "segment"
    } else if args.weekly {
        "weekly"
    } else if args.cadence {
        "cadence"
    } else {
        "daily"
    }
}

/// Best-effort structured diagnostic writer for one think invocation.
///
/// Clones share the same append-only oplog and serialize each complete record.
pub(crate) struct RunLogWriter {
    sink: Option<Arc<Mutex<OplogWriter>>>,
    display_path: PathBuf,
    pub(crate) skip_count: usize,
}

impl RunLogWriter {
    pub(crate) fn open(journal: &Path, day: &str, run: &str) -> Self {
        let display_path = journal.join("chronicle").join(day).join("health");
        let sink = match open_oplog(journal, day, run) {
            Ok(writer) => Some(Arc::new(Mutex::new(writer))),
            Err(error) => {
                log::warn!("Failed to open think operational log for {run} on {day}: {error}");
                None
            }
        };
        Self {
            sink,
            display_path,
            skip_count: 0,
        }
    }

    /// Make another in-process handle to this invocation's same oplog.
    pub(crate) fn clone_for_shared_writes(&self) -> Self {
        Self {
            sink: self.sink.clone(),
            display_path: self.display_path.clone(),
            skip_count: 0,
        }
    }

    pub(crate) fn log(&mut self, event: &str, now_ms: i64, fields: Map<String, Value>) {
        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        if event == "talent.skip" {
            self.skip_count += 1;
        }
        let mut data = fields;
        data.insert("event".to_owned(), Value::String(event.to_owned()));
        data.insert("ts".to_owned(), Value::from(now_ms));
        let result: Result<(), Box<dyn std::error::Error>> = (|| {
            let mut record = serde_json::to_vec(&data)?;
            record.push(b'\n');
            let mut sink = sink.lock().expect("think run log lock");
            sink.write_all(&record)?;
            sink.flush()?;
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!(
                "Failed to write think operational log {}: {error}",
                self.display_path.display()
            );
        }
    }

    pub(crate) fn summary(&mut self, now_ms: i64, message: String) {
        self.log(
            "run.summary",
            now_ms,
            Map::from_iter([("message".to_owned(), Value::String(message))]),
        );
    }
}

fn open_oplog(journal: &Path, day: &str, run: &str) -> Result<OplogWriter, String> {
    create_oplog_at(
        JournalRoot::open(journal).map_err(|error| error.to_string())?,
        "think",
        run,
        OplogFormat::Jsonl,
        current_time_on_local_day(day)?,
    )
    .map_err(|error| error.to_string())
}

fn current_time_on_local_day(day: &str) -> Result<DateTime<FixedOffset>, String> {
    let day = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|error| error.to_string())?;
    let now = Local::now().fixed_offset();
    let offset = now.offset().to_owned();
    offset
        .from_local_datetime(&day.and_time(now.time()))
        .single()
        .ok_or_else(|| "invalid local day".to_owned())
}
