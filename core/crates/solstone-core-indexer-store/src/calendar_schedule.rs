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
use solstone_core_format::calendar::{When, parse_calendar_note};
use solstone_core_indexer::edges::candidates::{
    KnownPerson as EntityNameCandidate, invitee_match, journal_candidates,
};

use crate::StoreError;
use crate::calendar::{Identity, LISTING_IDX_BASE, LISTING_IDX_STRIDE, listing_zone};
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
    let mut linked =
        conn.prepare("SELECT dst FROM edges WHERE source=? AND anchor=? ORDER BY rowid")?;
    let mut events = Vec::new();
    for (identity, record) in notes {
        let Some(mut event) = scheduled_event(&record, &zone) else {
            continue;
        };
        // The people the index linked from this version, as they are named now.
        let linked_ids = linked
            .query_map(
                params![CALENDAR_NOTE_EDGE_SOURCE, edge_anchor(&identity)],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        event.people = linked_ids
            .into_iter()
            .filter_map(|entity_id| {
                let identity = solstone_core_entity::read_entity_identity(journal, &entity_id)
                    .ok()
                    .flatten()?;
                let name = identity.value().get("name")?.as_str()?.trim().to_owned();
                (!name.is_empty()).then_some(InvitedPerson { entity_id, name })
            })
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

/// An event as its latest note has it, with no one on it yet.
fn scheduled_event(record: &str, zone: &Tz) -> Option<ScheduledEvent> {
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
        declined: owner_declined(&note.record),
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

fn is_owner(attendee: &Map<String, Value>) -> bool {
    attendee.get("you").and_then(Value::as_bool) == Some(true)
}

fn owner_declined(record: &Map<String, Value>) -> bool {
    invitees(record)
        .filter(|attendee| is_owner(attendee))
        .any(|attendee| {
            text(attendee, "status").is_some_and(|status| status.eq_ignore_ascii_case("declined"))
        })
}

/// The journal's known people, loaded once and only when an invite needs them.
struct KnownPeople<'a> {
    journal: &'a Path,
    loaded: Option<(Vec<EntityNameCandidate>, Option<String>)>,
}

impl<'a> KnownPeople<'a> {
    fn new(journal: &'a Path) -> Self {
        Self {
            journal,
            loaded: None,
        }
    }

    fn load(&mut self) -> Result<&(Vec<EntityNameCandidate>, Option<String>), StoreError> {
        if self.loaded.is_none() {
            let candidates = journal_candidates(self.journal)?;
            let principal = solstone_core_entity::journal_principal_entry(self.journal)
                .map_err(|error| StoreError::Io(std::io::Error::other(error.to_string())))?
                .map(|(folder, _)| folder);
            self.loaded = Some((candidates, principal));
        }
        Ok(self.loaded.as_ref().expect("loaded above"))
    }

    fn principal(&mut self) -> Result<Option<String>, StoreError> {
        Ok(self.load()?.1.clone())
    }

    /// The known people on an invite, its organizer included, the owner and
    /// anyone answering as the owner left out, each once.
    fn on_invite(&mut self, record: &Map<String, Value>) -> Result<Vec<InvitedPerson>, StoreError> {
        let owner_emails = invitees(record)
            .filter(|attendee| is_owner(attendee))
            .filter_map(|attendee| text(attendee, "email"))
            .map(|email| email.to_lowercase())
            .collect::<BTreeSet<_>>();
        let organizer = record.get("organizer").and_then(Value::as_object);
        let invited = organizer
            .into_iter()
            .chain(invitees(record).filter(|attendee| !is_owner(attendee)))
            .filter(|person| {
                text(person, "email")
                    .is_none_or(|email| !owner_emails.contains(&email.to_lowercase()))
            })
            .collect::<Vec<_>>();
        if invited.is_empty() {
            return Ok(Vec::new());
        }
        let (candidates, principal) = self.load()?;
        let mut seen = BTreeSet::new();
        let mut people = Vec::new();
        for person in invited {
            let name = text(person, "name");
            let email = text(person, "email");
            let Some(candidate) = invitee_match(name.as_deref(), email.as_deref(), candidates)
            else {
                continue;
            };
            let Some(entity_id) = candidate.id.clone() else {
                continue;
            };
            if principal.as_deref() == Some(entity_id.as_str()) || !seen.insert(entity_id.clone()) {
                continue;
            }
            people.push(InvitedPerson {
                entity_id,
                name: candidate.name.clone(),
            });
        }
        Ok(people)
    }
}

fn edge_anchor((event, occurrence): &Identity) -> String {
    format!("calendar:{event}|{occurrence}")
}

/// Replace the people links of each identity with those of its latest note,
/// inside the caller's index transaction.
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
    if identities.is_empty() {
        return Ok(());
    }
    // The listing itself waits for a readable settings file; so do its links.
    let Ok(zone) = listing_zone(journal) else {
        return Ok(());
    };
    let mut people = KnownPeople::new(journal);
    for identity in identities {
        let anchor = edge_anchor(identity);
        conn.execute(
            "DELETE FROM edges WHERE source=? AND anchor=?",
            params![CALENDAR_NOTE_EDGE_SOURCE, anchor],
        )?;
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
        let Some(event) = scheduled_event(&record, &zone) else {
            continue;
        };
        if event.declined {
            continue;
        }
        let Ok(note) = serde_json::from_str::<Map<String, Value>>(&record) else {
            continue;
        };
        let invited = people.on_invite(&note)?;
        if invited.is_empty() {
            continue;
        }
        let Some(principal) = people.principal()? else {
            continue;
        };
        let mut insert = conn.prepare_cached(
            "INSERT INTO edges(src, dst, kind, directed, src_name, dst_name, day, facet, source, path, anchor, label, ts, weight) \
             VALUES (?, ?, 'scheduled-with', 0, NULL, NULL, ?, '', ?, ?, ?, ?, NULL, 1)",
        )?;
        for person in &invited {
            insert.execute(params![
                principal,
                person.entity_id,
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
