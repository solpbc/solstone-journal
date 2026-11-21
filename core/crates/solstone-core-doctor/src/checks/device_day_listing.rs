// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};
use solstone_core_cli_copy as copy;

use solstone_core_ingest::device_day_listing_faults;

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
    match device_day_listing_faults(&context.journal_path, RECENT_DAYS) {
        Err(error) => Ok(make_result(
            check,
            Status::Skip,
            copy::display(
                "journal.cli.doctor.device.day.listing.streams.unavailable",
                [("error", &error)],
            ),
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
