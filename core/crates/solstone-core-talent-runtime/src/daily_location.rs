// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whether a frozen daily packet's location words still match the journal.

use std::path::Path;

use serde_json::Value;
use solstone_core_indexer::daily_evidence::DAILY_INPUTS;
use solstone_core_journal_config::read_journal_config;
use solstone_core_location::daily_location_fingerprint;

/// How a stored daily packet stands against the location inputs it would read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DailyLocationFreshness {
    /// The stored fingerprint is a successful read of the inputs now.
    Reuse,
    /// An eligible packet has no fingerprint, or a different one.
    Stale,
    /// The location inputs could not be read.
    Unreadable,
    /// This hook or day does not receive location words.
    NotApplicable,
}

/// Compare `stored_packet` with a fresh location fingerprint.
///
/// `Err` is only a malformed cutoff on a recipient. A failed location read is
/// [`DailyLocationFreshness::Unreadable`].
pub fn daily_location_freshness(
    journal: &Path,
    hook: &str,
    day: &str,
    stored_packet: Option<&Value>,
) -> Result<DailyLocationFreshness, String> {
    if !location_applies(journal, hook, day)? {
        return Ok(DailyLocationFreshness::NotApplicable);
    }
    compare_fingerprint(journal, day, stored_packet)
}

/// Publication's two reads of the fingerprint, with the test barrier between them.
///
/// The second read is the one that refuses a change made while the admission
/// guard is held.
pub fn publication_location_freshness(
    journal: &Path,
    hook: &str,
    day: &str,
    stored_packet: Option<&Value>,
) -> Result<DailyLocationFreshness, String> {
    if !location_applies(journal, hook, day)? {
        return Ok(DailyLocationFreshness::NotApplicable);
    }
    let first = match daily_location_fingerprint(journal, day) {
        Ok(hex) => hex,
        Err(_) => return Ok(DailyLocationFreshness::Unreadable),
    };
    #[cfg(test)]
    PUBLICATION_LOCATION_BARRIER.with(|slot| {
        if let Some(barrier) = slot.borrow_mut().as_mut() {
            barrier();
        }
    });
    let second = match daily_location_fingerprint(journal, day) {
        Ok(hex) => hex,
        Err(_) => return Ok(DailyLocationFreshness::Unreadable),
    };
    if first != second {
        return Ok(DailyLocationFreshness::Stale);
    }
    Ok(match stored_fingerprint(stored_packet) {
        Some(stored) if stored == second => DailyLocationFreshness::Reuse,
        _ => DailyLocationFreshness::Stale,
    })
}

pub(crate) fn location_applies(journal: &Path, hook: &str, day: &str) -> Result<bool, String> {
    let input = DAILY_INPUTS
        .iter()
        .find(|input| input.id == "location")
        .expect("registered provider");
    if !input.recipients.contains(&hook) {
        return Ok(false);
    }
    let configured = read_journal_config(journal)
        .map_err(|error| error.to_string())?
        .config
        .unwrap_or_default();
    input.applies(hook, day, &configured)
}

/// Any started or committed owner action pins the attempt to its recovery protocol.
pub fn has_owner_actions(record: &solstone_core_journal_io::DailyUnitRecord) -> bool {
    record
        .receipts
        .iter()
        .any(|item| item.get("kind").and_then(Value::as_str) == Some("owner_action"))
}

fn compare_fingerprint(
    journal: &Path,
    day: &str,
    stored_packet: Option<&Value>,
) -> Result<DailyLocationFreshness, String> {
    let current = match daily_location_fingerprint(journal, day) {
        Ok(hex) => hex,
        Err(_) => return Ok(DailyLocationFreshness::Unreadable),
    };
    Ok(match stored_fingerprint(stored_packet) {
        Some(stored) if stored == current => DailyLocationFreshness::Reuse,
        _ => DailyLocationFreshness::Stale,
    })
}

fn stored_fingerprint(stored_packet: Option<&Value>) -> Option<&str> {
    stored_packet
        .and_then(|packet| packet.pointer("/prepared/config/_daily_location_fingerprint"))
        .and_then(Value::as_str)
}

#[cfg(test)]
use std::cell::RefCell;

#[cfg(test)]
thread_local! {
    static PUBLICATION_LOCATION_BARRIER: RefCell<Option<Box<dyn FnMut()>>> =
        const { RefCell::new(None) };
}

/// Run `barrier` between publication's two fingerprint reads.
#[cfg(test)]
pub(crate) fn set_publication_location_barrier(barrier: Option<Box<dyn FnMut()>>) {
    PUBLICATION_LOCATION_BARRIER.with(|slot| *slot.borrow_mut() = barrier);
}
