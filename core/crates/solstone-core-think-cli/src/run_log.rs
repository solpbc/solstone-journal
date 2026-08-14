// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::args::ThinkArgs;

pub(crate) fn mode(args: &ThinkArgs) -> &'static str {
    if args.activity.is_some() {
        "activity"
    } else if args.flush {
        "flush"
    } else if args.segments || args.segment.is_some() {
        "segment"
    } else if args.weekly {
        "weekly"
    } else if args.cadence {
        "cadence"
    } else {
        "daily"
    }
}

pub(crate) fn path(day: &Path, now_ms: i64, mode: &str) -> PathBuf {
    day.join("health").join(format!("{now_ms}_{mode}.jsonl"))
}

pub(crate) struct RunLogWriter<W: Write> {
    sink: Option<W>,
    display_path: PathBuf,
    pub(crate) skip_count: usize,
}

impl RunLogWriter<std::fs::File> {
    pub(crate) fn open(path: &Path) -> Self {
        let display_path = path.to_path_buf();
        let sink = match path.parent().map(std::fs::create_dir_all).transpose() {
            Ok(_) => match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                Ok(file) => Some(file),
                Err(error) => {
                    log::warn!(
                        "Failed to open think JSONL sidecar {}: {error}",
                        path.display()
                    );
                    None
                }
            },
            Err(error) => {
                log::warn!(
                    "Failed to open think JSONL sidecar {}: {error}",
                    path.display()
                );
                None
            }
        };
        Self {
            sink,
            display_path,
            skip_count: 0,
        }
    }
}

impl<W: Write> RunLogWriter<W> {
    pub(crate) fn with_sink(display_path: PathBuf, sink: W) -> Self {
        Self {
            sink: Some(sink),
            display_path,
            skip_count: 0,
        }
    }

    pub(crate) fn log(&mut self, event: &str, now_ms: i64, fields: Map<String, Value>) {
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        if event == "talent.skip" {
            self.skip_count += 1;
        }
        let mut data = fields;
        data.insert("event".to_owned(), Value::String(event.to_owned()));
        data.insert("ts".to_owned(), Value::from(now_ms));
        let result: Result<(), Box<dyn std::error::Error>> = (|| {
            serde_json::to_writer(&mut *sink, &data)?;
            sink.write_all(b"\n")?;
            sink.flush()?;
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!(
                "Failed to write think JSONL sidecar {}: {error}",
                self.display_path.display()
            );
        }
    }
}
