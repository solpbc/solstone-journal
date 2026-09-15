// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only device-day listing health, for `journal doctor`.
//!
//! The device manifest tells a syncing device that a day is unreadable by
//! answering `{"<day>": {"error": …}}` for it, and the device then reports
//! itself offline and stops at that day. Nothing on the journal side otherwise
//! records that a day is being refused. This runs the same per-day listing the
//! manifest runs, for every bound device stream, and names each refused day.

use std::path::Path;

use solstone_core_segment::{list_days, list_stream_bindings};

use crate::listing::{ListingError, merge_day_listing, native_events};
use crate::model::ReasonCode;

/// One day the device manifest would refuse for one bound device stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceDayFault {
    pub day: String,
    pub stream: String,
    pub cid: String,
    /// The manifest's own reason token for the refusal.
    pub reason_code: &'static str,
}

/// Run the device manifest's per-day listing for every chain-advanced device
/// stream over the newest `recent_days` chronicle days.
///
/// `Ok(None)` means no device stream is bound, so there is nothing to judge.
pub fn device_day_listing_faults(
    journal_root: &Path,
    recent_days: usize,
) -> Result<Option<Vec<DeviceDayFault>>, String> {
    let bindings = list_stream_bindings(journal_root)
        .map_err(|error| format!("stream registry unreadable: {error}"))?
        .into_iter()
        .filter(|binding| binding.seq > 0)
        .collect::<Vec<_>>();
    if bindings.is_empty() {
        return Ok(None);
    }
    let mut days = list_days(journal_root)
        .map_err(|error| format!("chronicle unreadable: {error}"))?
        .into_iter()
        .map(|(day, _)| day)
        .collect::<Vec<_>>();
    days.sort();
    let mut faults = Vec::new();
    for day in days.into_iter().rev().take(recent_days) {
        for binding in &bindings {
            let listing = native_events(
                journal_root,
                &day,
                Some(&binding.name),
                &binding.cid,
                &binding.source,
            )
            .and_then(|events| merge_day_listing(journal_root, &day, events));
            if let Err(error) = listing {
                faults.push(DeviceDayFault {
                    day: day.clone(),
                    stream: binding.name.clone(),
                    cid: binding.cid.clone(),
                    reason_code: day_read_reason(error).as_str(),
                });
            }
        }
    }
    Ok(Some(faults))
}

/// The manifest's reason token for a listing failure; shared with the routes.
pub(crate) fn day_read_reason(error: ListingError) -> ReasonCode {
    match error {
        ListingError::AmbiguousName => ReasonCode::AmbiguousSegmentFileName,
        ListingError::JournalRead => ReasonCode::JournalReadFailed,
    }
}
