// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};
use solstone_core_cli_copy as copy;

use solstone_core_ingest::{DeviceDayReadError, device_day_listing_checked};
use solstone_core_journal_io::ReadError;
use solstone_core_segment::SegmentError;

/// Newest chronicle days the listing is run over, per bound device stream.
const RECENT_DAYS: usize = 14;

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.journal.absent"),
            None::<String>,
        ));
    }
    match device_day_listing_checked(&context.journal_path, RECENT_DAYS) {
        Err(error) => Ok(make_result(
            check,
            Status::Skip,
            unavailable(error),
            None::<String>,
        )),
        Ok(None) => Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.device.day.listing.streams.unbound"),
            None::<String>,
        )),
        Ok(Some(faults)) if faults.is_empty() => Ok(make_result(
            check,
            Status::Ok,
            copy::display(
                "journal.cli.doctor.device.day.listing.readable",
                [("days", &RECENT_DAYS)],
            ),
            None::<String>,
        )),
        Ok(Some(faults)) => {
            let detail = faults
                .iter()
                .map(|fault| {
                    copy::display(
                        "journal.cli.doctor.device.day.listing.fault",
                        [
                            ("day", &fault.day),
                            ("stream", &fault.stream),
                            ("device", &fault.cid),
                            ("reason", &fault.reason_code),
                        ],
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            Ok(make_result(
                check,
                Status::Warn,
                truncate(&detail, 400),
                Some(copy::text("journal.cli.doctor.device.day.listing.repair")),
            ))
        }
    }
}

fn unavailable(error: DeviceDayReadError) -> String {
    use DeviceDayReadError::{Chronicle, Continuity, Registry};
    match error {
        Registry(SegmentError::MalformedStreamRecord {
            path,
            source: ReadError::Malformed(data),
        }) => {
            let record = path.display();
            let source = data.path.display();
            if let Some(line) = data.line {
                copy::render(
                    "journal.cli.doctor.device.day.listing.record.line",
                    &[
                        ("record", copy::Value::String(&record.to_string())),
                        ("path", copy::Value::String(&source.to_string())),
                        ("line", copy::Value::Integer(line as i64)),
                    ],
                )
            } else {
                copy::display(
                    "journal.cli.doctor.device.day.listing.record.malformed",
                    [("record", &record), ("path", &source)],
                )
            }
        }
        Continuity(SegmentError::StreamInput("continuity record is malformed or unsupported")) => {
            copy::text("journal.cli.doctor.device.day.listing.continuity.malformed")
        }
        Registry(cause) => copy::display(
            "journal.cli.doctor.device.day.listing.registry.error",
            [("error", &cause)],
        ),
        Continuity(cause) => copy::display(
            "journal.cli.doctor.device.day.listing.continuity.error",
            [("error", &cause)],
        ),
        Chronicle(cause) => copy::display(
            "journal.cli.doctor.device.day.listing.chronicle.error",
            [("error", &cause)],
        ),
    }
}
