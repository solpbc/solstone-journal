// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the owner's calendar had scheduled, read from the listing, and the
//! people links calendar notes add.
//!
//! An event is read only as the calendar has it now: the latest note of each
//! identity, as the listing holds it. Nothing here says anyone went to
//! anything. A scheduled event sits beside the memory of that time, and a
//! person on its invite is linked to the owner as scheduled with them, never
//! as having attended.
//!
//! People are matched only to entities the journal already knows, and only
//! with high confidence: by email, or by the name on the invite as written,
//! ignoring case, or as the entity's id. Nobody new is created and nothing is
//! learned about anyone.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime};
use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};
use solstone_core_format::calendar::{OwnerAddresses, When, parse_calendar_note};
use solstone_core_indexer::edges::candidates::KnownPeople;

use crate::StoreError;
use crate::calendar::{
    Identity, LISTING_IDX_BASE, LISTING_IDX_STRIDE, listing_zone, owner_addresses,
};
use crate::db::{db_path, open_index_reader};

/// The source of the people links calendar notes add. Calendars read off the
/// screen use their own.
pub const CALENDAR_NOTE_EDGE_SOURCE: &str = "calendar-note";

/// An event longer than this never sits beside an activity: it says little
/// about any one stretch of the day.
const LONGEST_ATTACHED_SECONDS: i64 = 4 * 60 * 60;

/// A person the journal already knows, on an event's invite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitedPerson {
    pub entity_id: String,
    pub name: String,
}

/// One event the owner's calendar has on a day, as it has it now.
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledEvent {
    pub title: String,
    pub all_day: bool,
    /// Wall-clock start and end in the journal's zone. None for an all-day
    /// event; the end is None when the calendar gave none.
    pub start: Option<NaiveDateTime>,
    pub end: Option<NaiveDateTime>,
    /// The owner answered no.
    pub declined: bool,
    /// The known people on the invite, the owner left out, in invite order.
    pub people: Vec<InvitedPerson>,
}

impl ScheduledEvent {
    /// Whether this event was scheduled during an activity on `day` that ran
    /// over `span`, half-open seconds from that day's midnight.
    ///
    /// It does when either covers at least half of the other. An all-day
    /// event, one with no end, one longer than four hours and one the owner
    /// declined sit beside nothing.
    pub fn scheduled_during(&self, day: &str, span: (u32, u32)) -> bool {
        if self.all_day || self.declined {
            return false;
        }
        let (Some(start), Some(end)) = (self.start, self.end) else {
            return false;
        };
        let Some(midnight) = NaiveDate::parse_from_str(day, "%Y%m%d")
            .ok()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
        else {
            return false;
        };
        let event_start = (start - midnight).num_seconds();
        let event_end = (end - midnight).num_seconds();
        let event_length = event_end - event_start;
        if event_length <= 0 || event_length > LONGEST_ATTACHED_SECONDS {
            return false;
        }
        let (activity_start, activity_end) = (i64::from(span.0), i64::from(span.1));
        let activity_length = activity_end - activity_start;
        if activity_length <= 0 {
            return false;
        }
        let overlap = activity_end.min(event_end) - activity_start.max(event_start);
        overlap > 0 && (overlap * 2 >= activity_length || overlap * 2 >= event_length)
    }
}

/// Who is on each listed event's invite, among the people the journal
/// already knows: written with the event's people links, from its latest
/// note, so reading a day never matches names again.
pub(crate) const CREATE_CALENDAR_PEOPLE: &str = "\
CREATE TABLE IF NOT EXISTS calendar_people(
event TEXT NOT NULL,
occurrence TEXT NOT NULL,
position INTEGER NOT NULL,
entity_id TEXT NOT NULL,
PRIMARY KEY(event, occurrence, position)
)";

/// Every event the owner's calendar has on `day` (`YYYYMMDD`), timed events
/// by start. A journal with no search index has none.
pub fn scheduled_on_day(journal: &Path, day: &str) -> Result<Vec<ScheduledEvent>, StoreError> {
    if !db_path(journal).is_file() {
        return Ok(Vec::new());
    }
    let conn = open_index_reader(journal)?;
    let notes = {
        let mut statement = conn.prepare(
            "SELECT DISTINCT n.event, n.occurrence, n.record FROM calendar_hits h \
             JOIN chunks c ON c.rowid=h.rowid AND c.path=h.path \
             JOIN calendar_notes n ON n.path=h.path AND n.line=(h.idx - ?1) / ?2 \
             WHERE h.version='latest' AND c.day=?3",
        )?;
        statement
            .query_map(params![LISTING_IDX_BASE, LISTING_IDX_STRIDE, day], |row| {
                Ok(((row.get(0)?, row.get(1)?), row.get::<_, String>(2)?))
            })?
            .collect::<Result<Vec<(Identity, String)>, _>>()?
    };
    if notes.is_empty() {
        return Ok(Vec::new());
    }
    let zone = listing_zone(journal)?;
    let owner = owner_addresses(journal)?;
    let mut invited = conn.prepare(
        "SELECT entity_id FROM calendar_people WHERE event=? AND occurrence=? ORDER BY position",
    )?;
    let mut events = Vec::new();
    for ((event_id, occurrence), record) in notes {
        let Some(mut event) = scheduled_event(&record, &zone, &owner) else {
            continue;
        };
        let entity_ids = invited
            .query_map(params![event_id, occurrence], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        event.people = entity_ids
            .into_iter()
            .filter_map(|entity_id| named_now(journal, entity_id))
            .collect();
        events.push(event);
    }
    events.sort_by(|left, right| {
        right
            .all_day
            .cmp(&left.all_day)
            .then_with(|| left.start.cmp(&right.start))
            .then_with(|| left.title.cmp(&right.title))
    });
    Ok(events)
}

/// A person as the journal names them now. One since blocked, merged away or
/// removed is no longer named.
fn named_now(journal: &Path, entity_id: String) -> Option<InvitedPerson> {
    let identity = solstone_core_entity::read_entity_identity(journal, &entity_id)
        .ok()
        .flatten()?;
    let value = identity.value();
    if value.get("blocked").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let name = value.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| InvitedPerson {
        entity_id,
        name: name.to_owned(),
    })
}

/// An event as its latest note has it, with no one on it yet.
fn scheduled_event(record: &str, zone: &Tz, owner: &OwnerAddresses) -> Option<ScheduledEvent> {
    let note = parse_calendar_note(record).ok()?;
    let schedule = note.schedule?;
    let wall = |when: When| match when {
        When::Zoned(instant) => Some(instant.with_timezone(zone).naive_local()),
        When::Floating(wall) => Some(wall),
        When::Date(_) => None,
    };
    Some(ScheduledEvent {
        title: text(&note.record, "title").unwrap_or_default(),
        all_day: schedule.all_day,
        start: wall(schedule.start),
        end: schedule.end.and_then(wall),
        declined: owner_declined(&note.record, owner),
        people: Vec::new(),
    })
}

fn text(record: &Map<String, Value>, key: &str) -> Option<String> {
    record
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn invitees(record: &Map<String, Value>) -> impl Iterator<Item = &Map<String, Value>> {
    record
        .get("attendees")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
}

/// The owner answered no. The owner is the attendee with one of the owner's
/// addresses, never whoever the phone marked.
fn owner_declined(record: &Map<String, Value>, owner: &OwnerAddresses) -> bool {
    invitees(record)
        .filter(|attendee| owner.is_owner(attendee))
        .any(|attendee| {
            text(attendee, "status").is_some_and(|status| status.eq_ignore_ascii_case("declined"))
        })
}

/// The journal's people and its principal, loaded once per pass and only
/// when an invite needs them. A journal whose people can't be read links no
/// one, rather than failing the index write that asked.
struct People<'a> {
    journal: &'a Path,
    owner: &'a OwnerAddresses,
    loaded: Option<(KnownPeople, Option<String>)>,
}

impl<'a> People<'a> {
    fn new(journal: &'a Path, owner: &'a OwnerAddresses) -> Self {
        Self {
            journal,
            owner,
            loaded: None,
        }
    }

    fn load(&mut self) -> &(KnownPeople, Option<String>) {
        self.loaded.get_or_insert_with(|| {
            let known = KnownPeople::load(self.journal).unwrap_or_else(|error| {
                log::warn!(target: "solstone::indexer", "calendar invites matched to no one: {error}");
                KnownPeople::new(Vec::new())
            });
            let principal = solstone_core_entity::journal_principal_entry(self.journal)
                .unwrap_or_else(|error| {
                    log::warn!(target: "solstone::indexer", "calendar invites linked to no owner: {error}");
                    None
                })
                .map(|(folder, _)| folder);
            (known, principal)
        })
    }

    /// The known people on an invite, its organizer included, anyone with
    /// one of the owner's addresses left out, each once, in invite order.
    fn on_invite(&mut self, record: &Map<String, Value>) -> Vec<String> {
        let organizer = record.get("organizer").and_then(Value::as_object);
        let invited = organizer
            .into_iter()
            .chain(invitees(record))
            .filter(|person| !self.owner.is_owner(person))
            .collect::<Vec<_>>();
        if invited.is_empty() {
            return Vec::new();
        }
        let (known, principal) = self.load();
        let mut seen = BTreeSet::new();
        let mut people = Vec::new();
        for person in invited {
            let name = text(person, "name");
            let email = text(person, "email");
            let Some(entity_id) = known
                .invitee(name.as_deref(), email.as_deref())
                .and_then(|candidate| candidate.id.clone())
            else {
                continue;
            };
            if principal.as_deref() != Some(entity_id.as_str()) && seen.insert(entity_id.clone()) {
                people.push(entity_id);
            }
        }
        people
    }
}

fn edge_anchor((event, occurrence): &Identity) -> String {
    format!("calendar:{event}|{occurrence}")
}

/// Replace who is on each identity's invite, and its people links, with
/// those of its latest note, inside the caller's index transaction.
///
/// Each known person on the invite is linked to the owner as scheduled with
/// them, on the event's first listed day, filed under the file holding that
/// note, so removing the note's segment removes them. An identity with no
/// listing, a removal, an event the owner declined, or a journal with no
/// principal links no one.
pub(crate) fn relink_people(
    conn: &Connection,
    journal: &Path,
    identities: &BTreeSet<Identity>,
) -> Result<(), StoreError> {
    // Unreadable settings leave the links as they are; the relist that called
    // this has already left those files for the next scan.
    let owner = match owner_addresses(journal) {
        Ok(owner) => owner,
        Err(error) => {
            log::warn!(target: "solstone::indexer", "calendar invites not relinked yet: {error}");
            return Ok(());
        }
    };
    let mut people = People::new(journal, &owner);
    for identity in identities {
        let anchor = edge_anchor(identity);
        conn.prepare_cached("DELETE FROM edges WHERE source=? AND anchor=?")?
            .execute(params![CALENDAR_NOTE_EDGE_SOURCE, anchor])?;
        conn.prepare_cached("DELETE FROM calendar_people WHERE event=? AND occurrence=?")?
            .execute(params![identity.0, identity.1])?;
        let latest: Option<(String, i64, String)> = conn
            .prepare_cached(
                "SELECT h.path, h.idx, c.day FROM calendar_hits h \
                 JOIN chunks c ON c.rowid=h.rowid AND c.path=h.path \
                 WHERE h.version='latest' AND h.event=? AND h.occurrence=? \
                 ORDER BY c.day ASC, h.idx ASC LIMIT 1",
            )?
            .query_row(params![identity.0, identity.1], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        let Some((path, idx, day)) = latest else {
            continue;
        };
        let record: Option<String> = conn
            .prepare_cached("SELECT record FROM calendar_notes WHERE path=? AND line=?")?
            .query_row(
                params![path, (idx - LISTING_IDX_BASE) / LISTING_IDX_STRIDE],
                |row| row.get(0),
            )
            .optional()?;
        let Some(record) = record else {
            continue;
        };
        // Only the title and the owner's answer matter here, not the clock.
        let Some(event) = scheduled_event(&record, &chrono_tz::UTC, &owner) else {
            continue;
        };
        let Ok(note) = serde_json::from_str::<Map<String, Value>>(&record) else {
            continue;
        };
        let invited = people.on_invite(&note);
        let mut remember = conn.prepare_cached(
            "INSERT INTO calendar_people(event, occurrence, position, entity_id) VALUES (?, ?, ?, ?)",
        )?;
        for (position, entity_id) in invited.iter().enumerate() {
            remember.execute(params![identity.0, identity.1, position as i64, entity_id])?;
        }
        if event.declined || invited.is_empty() {
            continue;
        }
        let Some(principal) = people.load().1.clone() else {
            continue;
        };
        let mut link = conn.prepare_cached(
            "INSERT INTO edges(src, dst, kind, directed, src_name, dst_name, day, facet, source, path, anchor, label, ts, weight) \
             VALUES (?, ?, 'scheduled-with', 0, NULL, NULL, ?, '', ?, ?, ?, ?, NULL, 1)",
        )?;
        for entity_id in &invited {
            link.execute(params![
                principal,
                entity_id,
                day,
                CALENDAR_NOTE_EDGE_SOURCE,
                path,
                anchor,
                event.title,
            ])?;
        }
    }
    Ok(())
}

/// Relink the people of every event the listing holds, after the edges have
/// been cleared or before calendar notes ever had people links.
pub(crate) fn relink_all_people(conn: &Connection, journal: &Path) -> Result<(), StoreError> {
    let identities = {
        let mut statement = conn.prepare(
            "SELECT DISTINCT event, occurrence FROM calendar_hits WHERE version='latest'",
        )?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<BTreeSet<Identity>, _>>()?
    };
    relink_people(conn, journal, &identities)
}

#[cfg(test)]
#[path = "calendar_schedule_tests.rs"]
mod tests;
