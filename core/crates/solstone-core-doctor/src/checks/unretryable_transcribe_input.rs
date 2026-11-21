// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cli_copy as copy;

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
            return SidecarReadOutcome::CannotDetermine(copy::display(
                "journal.cli.doctor.unretryable.transcribe.input.open",
                [("path", &path.display()), ("error", &e)],
            ));
        }
    };
    let mut window = Vec::with_capacity(vocab::MAX_FIRST_ROW_BYTES + 1);
    if let Err(e) = file
        .by_ref()
        .take((vocab::MAX_FIRST_ROW_BYTES + 1) as u64)
        .read_to_end(&mut window)
    {
        return SidecarReadOutcome::CannotDetermine(copy::display(
            "journal.cli.doctor.unretryable.transcribe.input.read",
            [("path", &path.display()), ("error", &e)],
        ));
    }
    let Some(newline) = window.iter().position(|byte| *byte == b'\n') else {
        return SidecarReadOutcome::CannotDetermine(copy::display(
            "journal.cli.doctor.unretryable.transcribe.input.row.unavailable",
            [("path", &path.display())],
        ));
    };
    if newline > vocab::MAX_FIRST_ROW_BYTES {
        return SidecarReadOutcome::CannotDetermine(copy::display(
            "journal.cli.doctor.unretryable.transcribe.input.row.large",
            [("path", &path.display())],
        ));
    }
    let header_str = match std::str::from_utf8(&window[..newline]) {
        Ok(s) => s,
        Err(e) => {
            return SidecarReadOutcome::CannotDetermine(copy::display(
                "journal.cli.doctor.unretryable.transcribe.input.row.encoding",
                [("path", &path.display()), ("error", &e)],
            ));
        }
    };
    let json_val: Value = match serde_json::from_str(header_str) {
        Ok(v) => v,
        Err(e) => {
            return SidecarReadOutcome::CannotDetermine(copy::display(
                "journal.cli.doctor.unretryable.transcribe.input.row.json",
                [("path", &path.display()), ("error", &e)],
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
        return SidecarReadOutcome::CannotDetermine(copy::display(
            "journal.cli.doctor.unretryable.transcribe.input.handler.missing",
            [("path", &path.display())],
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
            return UnretryableScan::CannotDetermine(copy::display(
                "journal.cli.doctor.unretryable.transcribe.input.days.unreadable",
                [("error", &e)],
            ));
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
                return UnretryableScan::CannotDetermine(copy::display(
                    "journal.cli.doctor.unretryable.transcribe.input.segments.unreadable",
                    [("day", &day), ("error", &e)],
                ));
            }
        };
        for segment in segments {
            let record_id = match segment.record_identity() {
                Ok(id) => id,
                Err(e) => {
                    return UnretryableScan::CannotDetermine(copy::display(
                        "journal.cli.doctor.unretryable.transcribe.input.identity.invalid",
                        [("path", &(segment.path().display())), ("error", &e)],
                    ));
                }
            };
            let stream = record_id.stream;
            let key = record_id.key;
            let entries = match std::fs::read_dir(segment.path()) {
                Ok(e) => e,
                Err(e) => {
                    return UnretryableScan::CannotDetermine(copy::display(
                        "journal.cli.doctor.unretryable.transcribe.input.directory.unreadable",
                        [("path", &(segment.path().display())), ("error", &e)],
                    ));
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        return UnretryableScan::CannotDetermine(copy::display(
                            "journal.cli.doctor.unretryable.transcribe.input.entry.unreadable",
                            [("path", &(segment.path().display())), ("error", &e)],
                        ));
                    }
                };
                if let Some(name) = entry.file_name().to_str() {
                    if !entry.path().is_file()
                        || !(is_audio_sidecar_name(name)
                            || solstone_core_processing_record::is_transcribe_output(&entry.path()))
                    {
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
            copy::display(
                "journal.cli.doctor.unretryable.transcribe.input.clear",
                [
                    ("handler", &(vocab::HANDLER_TRANSCRIBE)),
                    ("reason_code", &(vocab::REASON_CORRUPT_INPUT)),
                ],
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
            let detail_raw = context.report_form.counted(
                "journal.cli.doctor.transcription.unretryable",
                &[("quantity", identities.len() as u64)],
                &[
                    ("count", &identities.len()),
                    ("handler", &(vocab::HANDLER_TRANSCRIBE)),
                    ("reason_code", &(vocab::REASON_CORRUPT_INPUT)),
                    ("identities", &formatted_ids),
                ],
            );
            let detail = truncate(&detail_raw, 400);
            Ok(make_result(check, Status::Warn, detail, None::<String>))
        }
        UnretryableScan::CannotDetermine(reason) => Ok(make_result(
            check,
            Status::Warn,
            copy::display(
                "journal.cli.doctor.unretryable.transcribe.input.scan.failed",
                [("reason", &reason)],
            ),
            None::<String>,
        )),
    }
}
