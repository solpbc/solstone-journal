// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the owner's calendar had scheduled while an activity ran. It sits
//! beside the activity and never changes it: an activity keeps its own title,
//! and a meeting on the calendar says nothing about who was there.

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_indexer_store::calendar_schedule::scheduled_on_day;

pub use solstone_core_indexer_store::calendar_schedule::{InvitedPerson, ScheduledEvent};

/// The events the owner's calendar has on `day`, or none when the index
/// can't say. Read once per day and match each activity against them.
pub fn calendar_on_day(journal: &Path, day: &str) -> Vec<ScheduledEvent> {
    scheduled_on_day(journal, day).unwrap_or_else(|error| {
        log::warn!(target: "solstone::activities", "calendar for {day} not read: {error}");
        Vec::new()
    })
}

/// The events scheduled during an activity on `day` that ran over `span`, as
/// `{title, start, end, people}` with clock times and the names of the known
/// people on each invite.
pub fn calendar_beside(calendar: &[ScheduledEvent], day: &str, span: (u32, u32)) -> Vec<Value> {
    calendar
        .iter()
        .filter(|event| event.scheduled_during(day, span))
        .map(|event| {
            json!({
                "title": event.title,
                "start": event.start.map(|at| at.format("%H:%M").to_string()),
                "end": event.end.map(|at| at.format("%H:%M").to_string()),
                "people": event.people.iter().map(|person| person.name.as_str()).collect::<Vec<_>>(),
            })
        })
        .collect()
}
