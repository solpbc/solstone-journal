// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use solstone_core_import_sources::ics;
use zip::ZipWriter;

static NEXT_TREE: AtomicUsize = AtomicUsize::new(0);
const ORACLE: &str = include_str!("../../../fixtures/import_source_preview_oracle.json");

#[test]
fn ics_oracle_detect_and_preview_match_fixture() {
    let tree = Tree::new();
    let calendar = tree.file("cal.ics", oracle_calendar());
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    let expected = &oracle["cases"]["ics"];

    assert_eq!(
        ics::detect(&calendar),
        expected["detect"].as_bool().unwrap()
    );
    let preview = ics::preview(&calendar).unwrap();
    assert_eq!(
        preview.date_range.0,
        expected["preview"]["date_range"][0].as_str().unwrap()
    );
    assert_eq!(
        preview.date_range.1,
        expected["preview"]["date_range"][1].as_str().unwrap()
    );
    assert_eq!(
        preview.item_count,
        expected["preview"]["item_count"].as_u64().unwrap()
    );
    assert_eq!(
        preview.entity_count,
        expected["preview"]["entity_count"].as_u64().unwrap()
    );
    assert_eq!(
        preview.summary,
        expected["preview"]["summary"].as_str().unwrap()
    );
}

#[test]
fn ics_preview_uses_utc_creation_days() {
    let tree = Tree::new();
    let calendar = tree.file(
        "utc.ics",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20260311T235900Z\r\nLAST-MODIFIED:20260311T235900Z\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20260312T000100Z\r\nCREATED:20260312T000100Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    let preview = ics::preview(&calendar).unwrap();
    assert_eq!(
        preview.date_range,
        ("20260311".to_owned(), "20260312".to_owned())
    );
}

#[test]
fn tzid_dates_are_resolved_before_calendar_entry_facts() {
    let tree = Tree::new();
    let calendar = tree.file(
        "tzid.ics",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Asia/Kolkata:20260311T001500\r\nDTEND;TZID=Asia/Kolkata:20260311T014500\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    let entries = ics::parse_events(&calendar).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].create_ts.to_rfc3339(),
        "2026-03-10T18:45:00+00:00"
    );
    assert_eq!(entries[0].day, "20260310");
    assert_eq!(entries[0].ts.as_deref(), Some("2026-03-11T00:15:00+05:30"));
    assert_eq!(
        entries[0].end_ts.as_deref(),
        Some("2026-03-11T01:45:00+05:30")
    );
    assert_eq!(entries[0].duration_minutes, Some(90));
}

#[test]
fn ics_preview_distinguishes_missing_data_from_empty_calendar() {
    let tree = Tree::new();
    let archive = tree.path().join("empty.zip");
    ZipWriter::new(fs::File::create(&archive).unwrap())
        .finish()
        .unwrap();
    let empty_calendar = tree.file("empty.ics", "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n");

    assert_eq!(ics::preview(&archive).unwrap().summary, "No ICS data found");
    assert_eq!(
        ics::preview(&empty_calendar).unwrap().summary,
        "No events found in ICS data"
    );
}

#[test]
fn ics_preview_skips_malformed_calendar_data() {
    let tree = Tree::new();
    let calendar = tree.file("malformed.ics", "this is not a calendar");

    let preview = ics::preview(&calendar).unwrap();
    assert_eq!(preview.item_count, 0);
    assert_eq!(preview.summary, "No events found in ICS data");
}

#[test]
fn duration_uses_wall_time_for_mixed_awareness() {
    let tree = Tree::new();
    let calendar = tree.file(
        "mixed-awareness.ics",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Asia/Kolkata:20260311T090000\r\nDTEND:20260311T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    let entries = ics::parse_events(&calendar).unwrap();
    assert_eq!(entries[0].duration_minutes, Some(60));
}

#[test]
fn parse_events_uses_creation_timestamp_priority_and_computes_duration() {
    let tree = Tree::new();
    let calendar = tree.file(
        "priority.ics",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Last modified wins\r\nDTSTART:20260304T100000Z\r\nCREATED:20260303T100000Z\r\nLAST-MODIFIED:20260302T100000Z\r\nDTEND:20260304T103000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nSUMMARY:Created wins\r\nDTSTART:20260306T100000Z\r\nCREATED:20260305\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nSUMMARY:Start fallback\r\nDTSTART:20260307\r\nDTEND:20260308\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    let entries = ics::parse_events(&calendar).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].day, "20260302");
    assert_eq!(entries[0].duration_minutes, Some(30));
    assert_eq!(entries[1].day, "20260305");
    assert_eq!(entries[2].day, "20260307");
    assert_eq!(entries[2].duration_minutes, Some(24 * 60));
}

#[test]
fn calendar_attendee_entities_require_name_and_email() {
    let tree = Tree::new();
    let calendar = tree.file(
        "attendees.ics",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20260310T100000Z\r\nORGANIZER;CN=Organizer Placeholder:mailto:ORGANIZER@example.test\r\nATTENDEE;CN=Duplicate Organizer:mailto:organizer@example.test\r\nATTENDEE;CN=Named Placeholder:mailto:named@example.test\r\nATTENDEE:mailto:unnamed@example.test\r\nATTENDEE;CN=No Address:invalid\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    let entries = ics::parse_events(&calendar).unwrap();
    assert_eq!(entries[0].attendees.len(), 3);
    let entities = ics::attendee_entities(&entries);
    assert_eq!(
        entities
            .iter()
            .map(|entity| {
                (
                    entity.name.as_str(),
                    entity.email.as_str(),
                    entity.entity_type.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("Organizer Placeholder", "organizer@example.test", "Person"),
            ("Named Placeholder", "named@example.test", "Person"),
        ]
    );
}

#[test]
fn calendar_entries_expose_writer_day() {
    let tree = Tree::new();
    let calendar = tree.file(
        "writer-day.ics",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20260401T010000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    let entries = ics::parse_events(&calendar).unwrap();
    assert_eq!(
        entries[0].create_ts.format("%Y%m%d").to_string(),
        entries[0].day
    );
}

fn oracle_calendar() -> &'static str {
    "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Attended event\r\nDTSTART:20260315T100000Z\r\nDTEND:20260315T103000Z\r\nLAST-MODIFIED:20260311T120000Z\r\nATTENDEE;CN=Calendar Placeholder:mailto:calendar@example.test\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nSUMMARY:Solo event\r\nDTSTART:20260316T100000Z\r\nCREATED:20260312T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
}

struct Tree(PathBuf);

impl Tree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-core-import-sources-ics-{}-{}",
            std::process::id(),
            NEXT_TREE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.path());
    }
}
