// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::*;
use crate::db::{db_path, prune_by_paths};
use crate::scan::{rebuild_edges, rescan_file};

const GOLDEN: &str = include_str!("../../../fixtures/calendar_notes/golden.jsonl");
const CALENDAR: &str = "phone_calendar";
const MEETING: &str = "6CF25C80-68D2-44D9-B52C-6CAD5BEC8C10";

/// Golden line 0 (a meeting on 2026-10-27, 10:00–11:00 Denver, organized by
/// Alex, with the owner invited) with some keys replaced.
fn meeting(changes: Value) -> String {
    let mut record: Map<String, Value> =
        serde_json::from_str(GOLDEN.lines().next().unwrap()).unwrap();
    for (key, value) in changes.as_object().unwrap() {
        record.insert(key.clone(), value.clone());
    }
    Value::Object(record).to_string()
}

fn attendee(name: Option<&str>, email: Option<&str>, status: &str, you: bool) -> Value {
    json!({"name": name, "email": email, "status": status, "you": you})
}

struct Journal(PathBuf);

impl Journal {
    fn new() -> Self {
        let root = crate::test_support::reserve_temp_path("index-calendar-schedule");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            json!({"identity": {"timezone": "America/Denver", "email_addresses": ["owner@example.com"]}})
                .to_string(),
        )
        .unwrap();
        fs::create_dir_all(root.join("streams")).unwrap();
        fs::write(
            root.join("streams").join(format!("{CALENDAR}.json")),
            json!({
                "name": CALENDAR, "kind": "observer", "host": null, "platform": null,
                "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
                "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "source": "calendar",
            })
            .to_string(),
        )
        .unwrap();
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn person(&self, id: &str, name: &str, emails: &[&str], principal: bool) {
        let dir = self.0.join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        let mut identity = json!({"id": id, "name": name, "type": "Person", "emails": emails});
        if principal {
            identity["is_principal"] = json!(true);
        }
        fs::write(dir.join("entity.json"), identity.to_string()).unwrap();
    }

    /// The owner, Alex, and two people only a first name could confuse.
    fn people(&self) {
        self.person("owner", "Jordan Lee", &["owner@example.com"], true);
        self.person("alex_rivera", "Alex Rivera", &["alex@example.com"], false);
        self.person("sam_rivera", "Sam Rivera", &[], false);
        self.person("dana_ortiz", "Dana Ortiz", &["dana@example.com"], false);
    }

    /// Store one calendar note in its own segment and index it, returning
    /// the file's index path.
    fn save(&self, day: &str, segment: &str, line: &str) -> String {
        let dir = self
            .0
            .join("chronicle")
            .join(day)
            .join(CALENDAR)
            .join(segment);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("stream.json"),
            json!({"stream": CALENDAR, "prev_day": null, "prev_segment": null, "seq": 1})
                .to_string(),
        )
        .unwrap();
        let bytes = format!("{line}\n").into_bytes();
        let name = format!("calendar_{:x}.jsonl", Sha256::digest(&bytes));
        fs::write(dir.join(&name), bytes).unwrap();
        let rel = format!("{day}/{CALENDAR}/{segment}/{name}");
        rescan_file(self.path(), Path::new(&rel)).unwrap();
        rel
    }

    /// Remove a note's segment and tell the index, as an owner's delete does.
    fn delete_segment_of(&self, rel: &str) {
        let segment = rel.rsplit_once('/').unwrap().0;
        fs::remove_dir_all(self.0.join("chronicle").join(segment)).unwrap();
        prune_by_paths(self.path(), &[segment]).unwrap();
    }

    /// The people links calendar notes hold: (src, dst, day, label).
    fn links(&self) -> Vec<(String, String, String, String)> {
        let conn = Connection::open(db_path(self.path())).unwrap();
        let mut statement = conn
            .prepare(
                "SELECT src, dst, day, label FROM edges WHERE source=? AND kind='scheduled-with' \
                 ORDER BY src, dst",
            )
            .unwrap();
        statement
            .query_map([CALENDAR_NOTE_EDGE_SOURCE], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn link(dst: &str, label: &str) -> (String, String, String, String) {
    (
        "owner".to_owned(),
        dst.to_owned(),
        "20261027".to_owned(),
        label.to_owned(),
    )
}

fn at(text: &str) -> Option<NaiveDateTime> {
    Some(NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap())
}

fn event(start: &str, end: &str) -> ScheduledEvent {
    ScheduledEvent {
        title: "weekly 1:1".to_owned(),
        all_day: false,
        start: at(start),
        end: at(end),
        declined: false,
        people: Vec::new(),
    }
}

fn clock(hours: u32, minutes: u32) -> u32 {
    hours * 3600 + minutes * 60
}

#[test]
fn a_day_reads_each_event_as_the_calendar_has_it_now_with_the_known_people_on_it() {
    let journal = Journal::new();
    journal.people();
    journal.save(
        "20261019",
        "094200_300",
        &meeting(json!({"attendees": [
            attendee(Some("Alex Rivera"), Some("alex@example.com"), "accepted", false),
            attendee(Some("Sam"), None, "accepted", false),
            attendee(None, Some("stranger@example.com"), "pending", false),
            attendee(Some("Jordan Lee"), Some("owner@example.com"), "tentative", true),
        ]})),
    );
    journal.save(
        "20261020",
        "120000_300",
        &meeting(json!({
            "changed_at": "2026-10-20T12:00:00-06:00",
            "title": "design review (moved)",
            "start": "2026-10-27T13:00:00-06:00",
            "end": "2026-10-27T13:30:00-06:00",
        })),
    );

    let events = scheduled_on_day(journal.path(), "20261027").unwrap();

    assert_eq!(
        events,
        vec![ScheduledEvent {
            title: "design review (moved)".to_owned(),
            all_day: false,
            start: at("2026-10-27 13:00"),
            end: at("2026-10-27 13:30"),
            declined: false,
            people: vec![InvitedPerson {
                entity_id: "alex_rivera".to_owned(),
                name: "Alex Rivera".to_owned(),
            }],
        }]
    );
    assert!(
        scheduled_on_day(journal.path(), "20261028")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_invitee_is_known_only_by_email_or_their_whole_name_never_a_first_name() {
    let journal = Journal::new();
    journal.people();
    journal.save(
        "20261019",
        "094200_300",
        &meeting(json!({
            "organizer": null,
            "attendees": [
                attendee(Some("Sam"), None, "accepted", false),
                attendee(Some("someone else"), Some("DANA@example.com"), "accepted", false),
                attendee(Some("sam rivera"), None, "accepted", false),
            ],
        })),
    );

    let people = scheduled_on_day(journal.path(), "20261027").unwrap()[0]
        .people
        .iter()
        .map(|person| person.entity_id.clone())
        .collect::<Vec<_>>();

    assert_eq!(people, ["dana_ortiz", "sam_rivera"]);
}

#[test]
fn a_meeting_sits_beside_an_activity_only_when_one_covers_half_the_other() {
    let day = "20261027";
    let one_to_one = event("2026-10-27 10:00", "2026-10-27 10:30");
    // A recorded stretch that ran a little past the meeting.
    assert!(one_to_one.scheduled_during(day, (clock(10, 2), clock(10, 41))));
    // A long stretch the whole meeting sits inside.
    assert!(one_to_one.scheduled_during(day, (clock(9, 0), clock(12, 0))));
    // A stretch that only grazes the meeting's end.
    assert!(!one_to_one.scheduled_during(day, (clock(10, 25), clock(11, 30))));
    // Earlier and later stretches.
    assert!(!one_to_one.scheduled_during(day, (clock(8, 0), clock(10, 0))));
    assert!(!one_to_one.scheduled_during(day, (clock(10, 30), clock(11, 0))));

    let late = event("2026-10-27 23:30", "2026-10-28 00:30");
    assert!(late.scheduled_during("20261028", (clock(0, 0), clock(0, 20))));

    let all_day = ScheduledEvent {
        all_day: true,
        start: None,
        end: None,
        ..event("2026-10-27 00:00", "2026-10-28 00:00")
    };
    assert!(!all_day.scheduled_during(day, (clock(10, 0), clock(11, 0))));
    let declined = ScheduledEvent {
        declined: true,
        ..one_to_one.clone()
    };
    assert!(!declined.scheduled_during(day, (clock(10, 0), clock(10, 30))));
    let offsite = event("2026-10-27 09:00", "2026-10-27 13:01");
    assert!(!offsite.scheduled_during(day, (clock(10, 0), clock(11, 0))));
    let no_end = ScheduledEvent {
        end: None,
        ..one_to_one
    };
    assert!(!no_end.scheduled_during(day, (clock(10, 0), clock(10, 30))));
}

#[test]
fn people_links_follow_the_latest_version_and_leave_with_its_segment() {
    let journal = Journal::new();
    journal.people();
    let first = journal.save("20261019", "094200_300", &meeting(json!({})));
    assert_eq!(journal.links(), [link("alex_rivera", "design review")]);

    let second = journal.save(
        "20261020",
        "120000_300",
        &meeting(json!({
            "changed_at": "2026-10-20T12:00:00-06:00",
            "organizer": null,
            "attendees": [
                attendee(Some("Dana Ortiz"), Some("dana@example.com"), "accepted", false),
                attendee(None, Some("owner@example.com"), "accepted", true),
            ],
        })),
    );
    assert_eq!(journal.links(), [link("dana_ortiz", "design review")]);

    journal.delete_segment_of(&second);
    assert_eq!(journal.links(), [link("alex_rivera", "design review")]);
    journal.delete_segment_of(&first);
    assert!(journal.links().is_empty());
}

#[test]
fn a_declined_or_cancelled_meeting_links_no_one() {
    let journal = Journal::new();
    journal.people();
    journal.save(
        "20261019",
        "094200_300",
        &meeting(json!({"attendees": [
            attendee(Some("Alex Rivera"), Some("alex@example.com"), "accepted", false),
            attendee(None, Some("owner@example.com"), "declined", true),
        ]})),
    );
    assert!(journal.links().is_empty());
    assert!(scheduled_on_day(journal.path(), "20261027").unwrap()[0].declined);

    let journal = Journal::new();
    journal.people();
    journal.save("20261019", "094200_300", &meeting(json!({})));
    journal.save(
        "20261020",
        "120000_300",
        &meeting(json!({
            "change": "removed", "event": MEETING, "changed_at": "2026-10-20T12:00:00-06:00",
            "all_day": null, "start": null, "end": null, "time_zone": null,
        })),
    );
    assert!(journal.links().is_empty());
}

#[test]
fn a_journal_without_its_owner_known_links_no_one_but_still_names_the_invite() {
    let journal = Journal::new();
    journal.person("alex_rivera", "Alex Rivera", &["alex@example.com"], false);
    journal.save("20261019", "094200_300", &meeting(json!({})));
    assert!(journal.links().is_empty());
    let named = scheduled_on_day(journal.path(), "20261027").unwrap()[0]
        .people
        .clone();
    assert_eq!(
        named,
        [InvitedPerson {
            entity_id: "alex_rivera".to_owned(),
            name: "Alex Rivera".to_owned(),
        }]
    );
}

#[test]
fn a_person_blocked_after_the_invite_came_in_is_no_longer_named() {
    let journal = Journal::new();
    journal.people();
    journal.save("20261019", "094200_300", &meeting(json!({})));
    let identity = journal.path().join("entities/alex_rivera/entity.json");
    let mut value: Value = serde_json::from_str(&fs::read_to_string(&identity).unwrap()).unwrap();
    value["blocked"] = json!(true);
    fs::write(&identity, value.to_string()).unwrap();
    assert!(
        scheduled_on_day(journal.path(), "20261027").unwrap()[0]
            .people
            .is_empty()
    );
}

#[test]
fn unreadable_people_link_no_one_and_never_fail_the_index_write() {
    let journal = Journal::new();
    // A file where the people folder should be.
    fs::write(journal.path().join("entities"), "not a folder").unwrap();
    let rel = journal.save("20261019", "094200_300", &meeting(json!({})));
    assert!(journal.links().is_empty());
    assert_eq!(
        scheduled_on_day(journal.path(), "20261027").unwrap().len(),
        1
    );
    journal.delete_segment_of(&rel);
    assert!(
        scheduled_on_day(journal.path(), "20261027")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn rebuilding_the_edges_keeps_the_people_links() {
    let journal = Journal::new();
    journal.people();
    journal.save("20261019", "094200_300", &meeting(json!({})));
    rebuild_edges(journal.path()).unwrap();
    assert_eq!(journal.links(), [link("alex_rivera", "design review")]);
}

#[test]
fn on_a_shared_calendar_the_owner_is_found_by_address_not_by_the_phones_mark() {
    let journal = Journal::new();
    journal.people();
    // The phone marks the calendar's owner, Alex, as you; the journal's owner
    // is an unmarked invitee who declined.
    journal.save(
        "20261019",
        "094200_300",
        &meeting(json!({"organizer": null, "attendees": [
            attendee(Some("Alex Rivera"), Some("alex@example.com"), "accepted", true),
            attendee(None, Some("owner@example.com"), "declined", false),
        ]})),
    );
    assert!(scheduled_on_day(journal.path(), "20261027").unwrap()[0].declined);
    assert!(journal.links().is_empty());

    // Accepted instead: Alex is on the meeting's people, the owner is not.
    let journal = Journal::new();
    journal.people();
    journal.save(
        "20261019",
        "094200_300",
        &meeting(json!({"organizer": null, "attendees": [
            attendee(Some("Alex Rivera"), Some("alex@example.com"), "accepted", true),
            attendee(None, Some("owner@example.com"), "accepted", false),
        ]})),
    );
    let events = scheduled_on_day(journal.path(), "20261027").unwrap();
    assert!(!events[0].declined);
    assert_eq!(journal.links(), [link("alex_rivera", "design review")]);
}

#[test]
fn a_meeting_gone_from_disk_loses_its_links_even_while_settings_cannot_be_read() {
    let journal = Journal::new();
    journal.people();
    let rel = journal.save("20261019", "094200_300", &meeting(json!({})));
    assert_eq!(journal.links(), [link("alex_rivera", "design review")]);
    let settings = journal.path().join("config/journal.json");
    fs::write(&settings, r#"{"identity":{"timezone":NaN}}"#).unwrap();
    // The segment leaves disk without the index being told; the next scan finds it gone.
    let segment = rel.rsplit_once('/').unwrap().0;
    fs::remove_dir_all(journal.path().join("chronicle").join(segment)).unwrap();
    crate::scan::scan_journal(journal.path(), true).unwrap();
    assert!(journal.links().is_empty(), "{:?}", journal.links());
}
