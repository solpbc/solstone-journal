// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::day_dirs;
use solstone_core_processing_record::vocab;
use solstone_core_system_health::sanitize_str_for_terminal_bounded;
use solstone_core_system_health::{FilesystemSegmentSource, SegmentSource};

use crate::context::CheckContext;
use crate::vocabulary::{Check, RunnerResult, Status, make_result, truncate};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnretryableScan {
    Counted(BTreeSet<(String, String, String)>),
    CannotDetermine(String),
}

enum SidecarReadOutcome {
    Skip,
    Count,
    CannotDetermine(String),
}

fn is_audio_sidecar_name(name: &str) -> bool {
    name == "audio.jsonl" || name.ends_with("_audio.jsonl") || name.ends_with("_transcript.jsonl")
}

fn read_audio_sidecar_first_row(path: &Path) -> SidecarReadOutcome {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return SidecarReadOutcome::CannotDetermine(format!(
                "cannot open {}: {e}",
                path.display()
            ));
        }
    };
    let mut window = Vec::with_capacity(vocab::MAX_FIRST_ROW_BYTES + 1);
    if let Err(e) = file
        .by_ref()
        .take((vocab::MAX_FIRST_ROW_BYTES + 1) as u64)
        .read_to_end(&mut window)
    {
        return SidecarReadOutcome::CannotDetermine(format!("cannot read {}: {e}", path.display()));
    }
    let Some(newline) = window.iter().position(|byte| *byte == b'\n') else {
        return SidecarReadOutcome::CannotDetermine(format!(
            "first row in {} cannot be established or exceeds byte limit",
            path.display()
        ));
    };
    if newline > vocab::MAX_FIRST_ROW_BYTES {
        return SidecarReadOutcome::CannotDetermine(format!(
            "first row in {} exceeds byte limit",
            path.display()
        ));
    }
    let header_str = match std::str::from_utf8(&window[..newline]) {
        Ok(s) => s,
        Err(e) => {
            return SidecarReadOutcome::CannotDetermine(format!(
                "first row in {} is not valid utf-8: {e}",
                path.display()
            ));
        }
    };
    let json_val: Value = match serde_json::from_str(header_str) {
        Ok(v) => v,
        Err(e) => {
            return SidecarReadOutcome::CannotDetermine(format!(
                "first row in {} is not valid json: {e}",
                path.display()
            ));
        }
    };
    let Value::Object(header_obj) = json_val else {
        return SidecarReadOutcome::Skip;
    };
    let Some(record_val) = header_obj.get("_solstone_processing") else {
        return SidecarReadOutcome::Skip;
    };
    let Value::Object(record_obj) = record_val else {
        return SidecarReadOutcome::Skip;
    };
    let Some(handler) = record_obj.get("handler").and_then(Value::as_str) else {
        return SidecarReadOutcome::CannotDetermine(format!(
            "processing record in {} missing handler field",
            path.display()
        ));
    };
    if handler != vocab::HANDLER_TRANSCRIBE {
        return SidecarReadOutcome::Skip;
    }
    let state = record_obj.get("state").and_then(Value::as_str);
    if state != Some(vocab::STATE_FAILED) {
        return SidecarReadOutcome::Skip;
    }
    let reason_code = record_obj.get("reason_code").and_then(Value::as_str);
    if reason_code == Some(vocab::REASON_CORRUPT_INPUT) {
        SidecarReadOutcome::Count
    } else {
        SidecarReadOutcome::Skip
    }
}

pub(crate) fn scan(journal: &Path) -> UnretryableScan {
    let days_map = match day_dirs(journal) {
        Ok(d) => d,
        Err(e) => {
            return UnretryableScan::CannotDetermine(format!("failed to read chronicle days: {e}"));
        }
    };
    let segment_source = FilesystemSegmentSource;
    let mut identities = BTreeSet::new();
    let mut day_keys = days_map.into_keys().collect::<Vec<_>>();
    day_keys.sort();

    for day in day_keys {
        let segments = match segment_source.segments(journal, &day) {
            Ok(s) => s,
            Err(e) => {
                return UnretryableScan::CannotDetermine(format!(
                    "failed to list segments for day {day}: {e}"
                ));
            }
        };
        for segment in segments {
            let record_id = match segment.record_identity() {
                Ok(id) => id,
                Err(e) => {
                    return UnretryableScan::CannotDetermine(format!(
                        "invalid segment identity in {}: {e}",
                        segment.path().display()
                    ));
                }
            };
            let stream = record_id.stream;
            let key = record_id.key;
            let entries = match std::fs::read_dir(segment.path()) {
                Ok(e) => e,
                Err(e) => {
                    return UnretryableScan::CannotDetermine(format!(
                        "cannot read segment dir {}: {e}",
                        segment.path().display()
                    ));
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        return UnretryableScan::CannotDetermine(format!(
                            "cannot read segment entry in {}: {e}",
                            segment.path().display()
                        ));
                    }
                };
                if let Some(name) = entry.file_name().to_str() {
                    if !is_audio_sidecar_name(name) || !entry.path().is_file() {
                        continue;
                    }
                    match read_audio_sidecar_first_row(&entry.path()) {
                        SidecarReadOutcome::Count => {
                            identities.insert((day.clone(), stream.to_owned(), key.to_owned()));
                        }
                        SidecarReadOutcome::Skip => {}
                        SidecarReadOutcome::CannotDetermine(reason) => {
                            return UnretryableScan::CannotDetermine(reason);
                        }
                    }
                }
            }
        }
    }
    UnretryableScan::Counted(identities)
}

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    match scan(&context.journal_path) {
        UnretryableScan::Counted(identities) if identities.is_empty() => Ok(make_result(
            check,
            Status::Ok,
            format!(
                "0 recordings the journal will not retry on its own (handler: {}, reason_code: {})",
                vocab::HANDLER_TRANSCRIBE,
                vocab::REASON_CORRUPT_INPUT
            ),
            None::<String>,
        )),
        UnretryableScan::Counted(identities) => {
            let formatted_ids = identities
                .iter()
                .map(|(day, stream, key)| {
                    format!(
                        "{}/{}/{}",
                        sanitize_str_for_terminal_bounded(day),
                        sanitize_str_for_terminal_bounded(stream),
                        sanitize_str_for_terminal_bounded(key)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let detail_raw = format!(
                "{} recordings the journal will not retry on its own (handler: {}, reason_code: {}): {}",
                identities.len(),
                vocab::HANDLER_TRANSCRIBE,
                vocab::REASON_CORRUPT_INPUT,
                formatted_ids
            );
            let detail = truncate(&detail_raw, 400);
            Ok(make_result(
                check,
                Status::Warn,
                detail,
                Some("journal transcribe --redo"),
            ))
        }
        UnretryableScan::CannotDetermine(reason) => Ok(make_result(
            check,
            Status::Warn,
            format!(
                "could not determine how many recordings the journal will not retry on its own ({reason})"
            ),
            None::<String>,
        )),
    }
}
