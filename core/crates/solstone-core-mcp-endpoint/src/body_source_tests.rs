// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A body source as an agent connection reaches it, through the real dispatch:
//! a search hit, the experience, and paged readings over a span.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows};

use crate::dispatch::run_mcp_probe;
use crate::permissions::{PermissionStore, ReadPermission, ReadScope};

const CONNECTION: &str = "bearer:body-test";
const HEALTH: &str = "123e4567-e89b-42d3-a456-426614174000";
const WORK: &str = "223e4567-e89b-42d3-a456-426614174000";
const DAY: &str = "20261010";
const STREAM: &str = "import.apple_health";
const LABEL: &str = "Apple Health · imported";

#[cfg(feature = "full-tests")]
#[test]
fn facet_siblings_page_unique_rows_and_cover_only_their_civil_hour() {
    let journal = journal();
    let root = journal.path();
    window(
        root,
        DAY,
        "060000_3600",
        &[reading("work-only", "2026-10-10 06:11:00 -0600", "9111")],
        Some("work"),
    );
    window(
        root,
        DAY,
        "060000_3602",
        &many_readings("first", 6, 31, 60),
        Some("health"),
    );
    window(
        root,
        DAY,
        "060000_3603",
        &many_readings("second", 6, 41, 60),
        Some("health"),
    );
    grant(
        root,
        ReadScope::Facets {
            ids: vec![HEALTH.to_owned()],
        },
    );
    let args =
        json!({"day": DAY, "source": LABEL, "time_from": "06:00", "time_to": "08:00", "limit": 1});
    let first = call(root, "list_transcripts", args.clone());
    assert_eq!(first["gaps"], json!([{"from": "07:00", "to": "08:00"}]));
    assert_eq!(first["transcripts"].as_array().unwrap().len(), 1);
    let mut next_args = args;
    next_args["cursor"] = first["next_cursor"].clone();
    let second = call(root, "list_transcripts", next_args);
    assert_eq!(second["gaps"], first["gaps"]);
    let (entries, pages) = every_entry(root, first["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 120);
    assert_eq!(pages, [100, 20]);
    assert!(entries.iter().all(|entry| !entry.contains("9111")));
    let after = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "07:00", "time_to": "07:05"}),
    );
    assert!(
        after["transcripts"].as_array().unwrap().is_empty(),
        "{after}"
    );
    assert_eq!(after["gaps"], json!([{"from": "07:00", "to": "07:05"}]));
    grant(
        root,
        ReadScope::Facets {
            ids: vec![WORK.to_owned()],
        },
    );
    let work = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "06:00", "time_to": "07:00"}),
    );
    let (entries, _) = every_entry(root, work["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 1);
    assert!(entries[0].contains("9111"));
}

fn journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("solstone-mcp-body-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        json!({"identity": {"timezone": "America/Denver"}}).to_string(),
    )
    .unwrap();
    for (name, id) in [("health", HEALTH), ("work", WORK)] {
        let facet = root.join("facets").join(name);
        fs::create_dir_all(&facet).unwrap();
        fs::write(
            facet.join("facet.json"),
            json!({"id": id, "title": name, "description": name}).to_string(),
        )
        .unwrap();
    }
    fs::create_dir_all(root.join("streams")).unwrap();
    fs::write(
        root.join("streams/import.apple_health.json"),
        json!({
            "name": STREAM, "kind": "import:apple_health", "host": null, "platform": null,
            "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
        })
        .to_string(),
    )
    .unwrap();
    journal
}

/// A connection that reads transcripts and body data.
fn grant(journal: &Path, scope: ReadScope) {
    grant_kinds(journal, &["transcripts", "body"], scope);
}

fn grant_kinds(journal: &Path, kinds: &[&str], scope: ReadScope) {
    PermissionStore::open(journal)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
                scope,
            },
        )
        .unwrap();
}

fn reading(id: &str, at: &str, value: &str) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Reading,
        id.into(),
        "apple_health",
        "HKQuantityTypeIdentifierHeartRate",
        "heart rate".into(),
    );
    row.at = Some(at.into());
    row.value = Some(value.into());
    row.unit = Some("count/min".into());
    row.by = Some("Synthetic Watch".into());
    row.fields
        .insert("sourceVersion".into(), json!("distinctive-version-11"));
    row
}

fn workout() -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Experience,
        "workout".into(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "running".into(),
    );
    row.experience = Some("workout".into());
    row.start = Some("2026-10-10 06:31:00 -0600".into());
    row.end = Some("2026-10-10 07:20:00 -0600".into());
    row.by = Some("Synthetic Watch".into());
    row.words = vec!["Douro Loop".into()];
    row.values.insert("distance".into(), "8.1 km".into());
    row.fields
        .insert("device".into(), json!("<<HKDevice>, uuid:ABC-DEF>"));
    row
}

/// A window as the real writer leaves it: file, lock and stream marker.
fn window(root: &Path, day: &str, key: &str, rows: &[BodyRow], facet: Option<&str>) {
    window_in(root, STREAM, day, key, rows, facet);
}

/// [`window`], in any body stream.
fn window_in(
    root: &Path,
    stream: &str,
    day: &str,
    key: &str,
    rows: &[BodyRow],
    facet: Option<&str>,
) {
    let dir = root.join("chronicle").join(day).join(stream).join(key);
    fs::create_dir_all(dir.join("talents")).unwrap();
    fs::write(dir.join("body.jsonl"), encode_rows(rows)).unwrap();
    fs::write(dir.join("body.jsonl.lock"), "").unwrap();
    fs::write(dir.join("stream.json"), "{}").unwrap();
    if let Some(facet) = facet {
        fs::write(
            dir.join("talents/facets.json"),
            json!([{ "facet": facet }]).to_string(),
        )
        .unwrap();
    }
}

fn index(journal: &Path) {
    let report = solstone_core_indexer_store::scan::scan_journal(journal, true).unwrap();
    assert_eq!(report.failed, 0, "{report:?}");
}

/// What an agent reads beside an empty result.
fn empty_note(journal: &Path, arguments: Value) -> String {
    let entry = crate::registry::tool_by_wire_name("list_transcripts").unwrap();
    crate::dispatch::dispatch_authenticated_tool_call(
        journal,
        crate::dispatch::DispatchPrincipal {
            connection: CONNECTION,
            agent_identity: CONNECTION,
        },
        entry.tool_name,
        Some(&arguments),
        jiff::Timestamp::now(),
    )
    .unwrap()
    .empty_note
    .unwrap_or_default()
}

fn call(journal: &Path, tool: &str, arguments: Value) -> Value {
    run_mcp_probe(journal, CONNECTION, tool, &arguments).unwrap()
}

fn many_readings(prefix: &str, hour: u32, minute: u32, count: u32) -> Vec<BodyRow> {
    (0..count)
        .map(|second| {
            reading(
                &format!("{prefix}{second:03}"),
                &format!("2026-10-10 {hour:02}:{minute:02}:{:02} -0600", second % 60),
                &format!("{}", 7000 + second),
            )
        })
        .collect()
}

fn every_entry(journal: &Path, span: &str) -> (Vec<String>, Vec<usize>) {
    let mut entries = Vec::new();
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..100 {
        let mut arguments = json!({ "reference": span });
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = call(journal, "get_transcript", arguments);
        let got = page["entries"].as_array().unwrap();
        pages.push(got.len());
        entries.extend(
            got.iter()
                .map(|entry| entry.as_str().unwrap().replace('\\', "")),
        );
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return (entries, pages),
        }
    }
    panic!("paging did not end");
}

#[test]
fn an_agent_goes_from_a_search_hit_to_the_experience_and_pages_a_span() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    let mut first = many_readings("a", 6, 31, 30);
    first.push(workout());
    window(root, DAY, "063000_300", &first, None);
    window(
        root,
        DAY,
        "063500_300",
        &many_readings("b", 6, 36, 80),
        None,
    );
    window(root, DAY, "070000_300", &many_readings("c", 7, 1, 5), None);
    fs::write(
        root.join("chronicle").join(DAY).join("body.md"),
        "# body on Saturday, October 10, 2026\n\n## Apple Health · imported\n\n- workout: running\n",
    )
    .unwrap();
    index(root);

    // Search finds the experience by the words a person would type.
    for query in ["running", "Douro Loop"] {
        let found = call(root, "search", json!({"query": query, "day": DAY}));
        let hit = found["results"].as_array().unwrap().iter().find(|result| {
            result["snippet"]
                .as_str()
                .unwrap_or_default()
                .contains("Douro")
        });
        assert!(hit.is_some(), "{query}: {found}");
    }
    // And the day, by its document.
    let day = call(
        root,
        "search",
        json!({"query": "Saturday October", "day": DAY}),
    );
    assert!(!day["results"].as_array().unwrap().is_empty(), "{day}");
    // Never a reading's value, a device string or a raw field.
    for query in ["7003", "ABC-DEF", "distinctive-version-11", "HKDevice"] {
        let found = call(root, "search", json!({"query": query, "day": DAY}));
        assert!(
            found["results"].as_array().unwrap().is_empty(),
            "{query}: {found}"
        );
    }

    // The hit's link reaches the experience with every field.
    let found = call(root, "search", json!({"query": "Douro Loop", "day": DAY}));
    let hit = &found["results"][0];
    let fetched = call(root, "fetch", json!({"reference": hit["reference"]}));
    assert!(
        fetched.to_string().replace('\\', "").contains("8.1 km"),
        "{fetched}"
    );
    let link = fetched["segments"][0]["reference"].as_str().unwrap();
    let tile = call(root, "get_transcript", json!({"reference": link}));
    let text = tile["entries"].to_string().replace('\\', "");
    assert!(text.contains("uuid:ABC-DEF"), "{text}");
    assert!(!tile.to_string().contains(STREAM));

    // A span of that source: windows, what no window covers, and a walk.
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "time_from": "06:00", "time_to": "08:00", "source": LABEL, "limit": 1}),
    );
    assert_eq!(list["transcripts"].as_array().unwrap().len(), 1);
    assert_eq!(
        list["gaps"],
        json!([
            {"from": "06:00", "to": "06:30"},
            {"from": "06:40", "to": "07:00"},
            {"from": "07:05", "to": "08:00"},
        ])
    );
    assert!(!list.to_string().contains(STREAM), "{list}");

    // The next list page states the same gaps, and its span walks the same windows.
    let next = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "time_from": "06:00", "time_to": "08:00", "source": LABEL, "limit": 1, "cursor": list["next_cursor"]}),
    );
    assert_eq!(next["gaps"], list["gaps"]);
    assert_eq!(next["transcripts"][0]["time"], "06:35-06:40");
    let (from_first, pages) = every_entry(root, list["span_reference"].as_str().unwrap());
    let (from_next, _) = every_entry(root, next["span_reference"].as_str().unwrap());
    assert_eq!(from_first, from_next);

    // 31 + 80 + 5 entries in time order, 100 to a page.
    assert_eq!(from_first.len(), 116);
    assert_eq!(pages, vec![100, 16]);
    let unique: std::collections::BTreeSet<_> = from_first.iter().collect();
    assert_eq!(unique.len(), from_first.len(), "each entry once");
    assert!(from_first[0].contains("Apple Health, Synthetic Watch"));
    assert!(
        from_first
            .iter()
            .all(|entry| entry.contains("Apple Health"))
    );
    assert!(from_first[0].contains("distinctive-version-11"));
    // The workout starts on the second of its first reading, and sorts beside it.
    assert!(from_first[1].contains("running"), "{:?}", &from_first[..3]);

    // A cursor can't change what it lists.
    for other in [
        json!({"day": DAY, "limit": 1}),
        json!({"day": DAY, "source": "Oura · imported", "limit": 1}),
    ] {
        let mut arguments = other;
        arguments["time_from"] = json!("06:00");
        arguments["time_to"] = json!("08:00");
        arguments["cursor"] = list["next_cursor"].clone();
        assert!(run_mcp_probe(root, CONNECTION, "list_transcripts", &arguments).is_err());
    }
}

#[test]
fn the_owners_log_records_the_source_and_a_span_reference_doesnt_read_as_a_change() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    window(root, DAY, "063000_300", &many_readings("a", 6, 31, 3), None);
    let first = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    let second = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert_ne!(first["span_reference"], second["span_reference"]);
    assert_eq!(
        crate::dispatch::canonical_content(&first),
        crate::dispatch::canonical_content(&second)
    );
}

#[test]
fn a_source_listing_says_only_what_it_knows() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    window(root, DAY, "063000_300", &many_readings("a", 6, 31, 3), None);
    window(root, DAY, "000000_86400", &[], None);

    // A whole-day window isn't coverage.
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert_eq!(
        list["gaps"],
        json!([{"from": "00:00", "to": "06:30"}, {"from": "06:35", "to": "24:00"}])
    );
    // A known source with nothing in the range: a whole-range gap.
    let empty = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "12:00", "time_to": "13:00"}),
    );
    assert_eq!(empty["gaps"], json!([{"from": "12:00", "to": "13:00"}]));

    // An unknown label: nothing stated about gaps, and not "a complete answer".
    let unknown = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": "Nothing · imported"}),
    );
    assert!(unknown.get("gaps").is_none(), "{unknown}");
    assert!(unknown.get("span_reference").is_none());
    let note = empty_note(root, json!({"day": DAY, "source": "Nothing · imported"}));
    assert!(
        !note.is_empty() && !note.contains("complete answer"),
        "{note}"
    );

    // Another stream's label lists that stream's segments, not this source's.
    let phone = root.join("chronicle").join(DAY).join("phone/090000_300");
    fs::create_dir_all(&phone).unwrap();
    fs::write(phone.join("audio.jsonl"), "{}\n{\"text\":\"hello\"}\n").unwrap();
    let plain = call(root, "list_transcripts", json!({"day": DAY}));
    let phone_label = plain["transcripts"][0]["source"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": phone_label}),
    );
    assert_eq!(other["transcripts"].as_array().unwrap().len(), 1, "{other}");
    assert_eq!(other["transcripts"][0]["source"], phone_label, "{other}");
    assert_eq!(
        other["gaps"],
        json!([{"from": "00:00", "to": "09:00"}, {"from": "09:05", "to": "24:00"}])
    );

    // Needs a day, refuses a facet, and says so.
    for arguments in [
        json!({"source": LABEL}),
        json!({"day": DAY, "source": LABEL, "facet": HEALTH}),
    ] {
        assert!(run_mcp_probe(root, CONNECTION, "list_transcripts", &arguments).is_err());
    }

    // Older content in the same stream is said, not hidden behind a gap.
    let card = root
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("000000_300");
    fs::create_dir_all(&card).unwrap();
    fs::write(card.join("day_summary_transcript.md"), "a card\n").unwrap();
    let with_card = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert!(with_card.get("gaps_note").is_some(), "{with_card}");

    // Once the owner deletes it there is nothing left for gaps_note to describe.
    fs::remove_file(card.join("day_summary_transcript.md")).unwrap();
    fs::write(
        card.join("tombstone.json"),
        r#"{"manifest":["a/day_summary_transcript.md"]}"#,
    )
    .unwrap();
    let deleted_card = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert!(deleted_card.get("gaps_note").is_none(), "{deleted_card}");
}

#[test]
fn an_unfiltered_list_leaves_body_windows_out_and_names_them() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    window(root, DAY, "063000_300", &many_readings("a", 6, 31, 3), None);
    let list = call(root, "list_transcripts", json!({"day": DAY}));
    assert!(list["transcripts"].as_array().unwrap().is_empty());
    assert_eq!(list["other_sources"], json!([LABEL]));
    let note = empty_note(root, json!({"day": DAY}));
    assert!(note.contains("other_sources"), "{note}");

    // A list with no day says there's more than transcripts, without naming a day.
    let any_day = call(root, "list_transcripts", json!({}));
    assert!(any_day["transcripts"].as_array().unwrap().is_empty());
    let note = empty_note(root, json!({}));
    assert!(note.contains("List a day"), "{note}");

    // Every window deleted: nothing to name.
    let dir = root
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("063000_300");
    fs::remove_file(dir.join("body.jsonl")).unwrap();
    fs::write(
        dir.join("tombstone.json"),
        r#"{"manifest":["a/body.jsonl"]}"#,
    )
    .unwrap();
    let list = call(root, "list_transcripts", json!({"day": DAY}));
    assert!(list.get("other_sources").is_none(), "{list}");
}

fn strava_tile(root: &Path, key: &str, extra: &[&str]) -> std::path::PathBuf {
    let dir = root
        .join("chronicle")
        .join(DAY)
        .join("import.strava")
        .join(key);
    fs::create_dir_all(dir.join("talents")).unwrap();
    fs::write(
        dir.join("workout.json"),
        json!({
            "schema": "solstone.import.strava.tile.v1",
            "activity_id": 9,
            "tile": {"index": 0, "count": 1},
            "workout": {"name": "Morning Run", "type": "Run", "distance_m": 10200.0}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(dir.join("stream.json"), "{}").unwrap();
    // What thinking once wrote under a sensed tile.
    fs::write(dir.join("talents/sense.json"), "{}").unwrap();
    for name in extra {
        fs::write(dir.join(name), "other").unwrap();
    }
    dir
}

/// A Strava workout is body data a list leaves out. No source label lists it, so
/// the list never names Strava in other_sources: it counts the workouts it left
/// out and says where they are found. A list with no Strava tile adds nothing.
#[test]
fn an_unfiltered_list_leaves_strava_workouts_out_and_counts_them() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    let tile = strava_tile(root, "070000_300", &[]);
    // A workout's next five minutes is a second tile of the same workout: it
    // counts once.
    let next = strava_tile(root, "070500_300", &[]);
    let list = call(root, "list_transcripts", json!({"day": DAY}));
    assert!(list["transcripts"].as_array().unwrap().is_empty(), "{list}");
    assert!(list.get("other_sources").is_none(), "{list}");
    assert_eq!(list["omitted_workouts"], json!(1), "{list}");
    let note = list["workouts_note"].as_str().unwrap();
    assert!(
        note.contains("search") && note.contains("list_activities"),
        "{note}"
    );
    let empty = empty_note(root, json!({"day": DAY}));
    assert!(empty.contains("omitted_workouts"), "{empty}");
    let any_day = call(root, "list_transcripts", json!({}));
    assert_eq!(any_day["omitted_workouts"], json!(1), "{any_day}");
    // Beside a body window, only the window's source is named.
    window(root, DAY, "063000_300", &many_readings("a", 6, 31, 3), None);
    let list = call(root, "list_transcripts", json!({"day": DAY}));
    assert_eq!(list["other_sources"], json!([LABEL]));
    assert_eq!(list["omitted_workouts"], json!(1), "{list}");

    // With no Strava tile left, nothing is added.
    fs::remove_dir_all(&tile).unwrap();
    fs::remove_dir_all(&next).unwrap();
    let list = call(root, "list_transcripts", json!({"day": DAY}));
    assert!(list.get("omitted_workouts").is_none(), "{list}");
    assert!(list.get("workouts_note").is_none(), "{list}");
    let other = journal_without_body();
    grant(other.path(), ReadScope::WholeJournal);
    let list = call(other.path(), "list_transcripts", json!({"day": DAY}));
    assert!(list.get("other_sources").is_none(), "{list}");
    assert!(list.get("omitted_workouts").is_none(), "{list}");
    let note = empty_note(other.path(), json!({}));
    assert!(!note.contains("List a day"), "{note}");

    // A tile the body-only test rejects is listed as it always was.
    strava_tile(root, "080000_300", &["notes.md"]);
    let list = call(root, "list_transcripts", json!({"day": DAY}));
    assert_eq!(list["transcripts"].as_array().unwrap().len(), 1, "{list}");
    assert!(list.get("omitted_workouts").is_none(), "{list}");
}

fn journal_without_body() -> tempfile::TempDir {
    let journal = journal();
    let capture = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join("default/090000_300");
    fs::create_dir_all(&capture).unwrap();
    fs::write(
        capture.join("audio.jsonl"),
        r#"{"start":"00:00:01","text":"garden planning"}"#,
    )
    .unwrap();
    journal
}

#[test]
fn a_walk_crosses_what_it_cant_read_and_shows_nothing_hidden() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    window(root, DAY, "060000_300", &many_readings("a", 6, 1, 2), None);
    window(root, DAY, "060500_300", &many_readings("b", 6, 6, 2), None);
    window(root, DAY, "061000_300", &many_readings("c", 6, 11, 2), None);
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    let span = list["span_reference"].as_str().unwrap().to_owned();

    // A window deleted after the list is skipped, not fatal.
    let deleted = root
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("060500_300");
    fs::remove_file(deleted.join("body.jsonl")).unwrap();
    fs::write(
        deleted.join("tombstone.json"),
        r#"{"manifest":["a/body.jsonl"]}"#,
    )
    .unwrap();
    let page = call(root, "get_transcript", json!({"reference": span}));
    assert_eq!(page["entries"].as_array().unwrap().len(), 4);
    assert!(page["next_cursor"].is_null());
    let after = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert_eq!(
        after["gaps"],
        json!([
            {"from": "00:00", "to": "06:00"},
            {"from": "06:05", "to": "06:10"},
            {"from": "06:15", "to": "24:00"},
        ])
    );

    // A sparse span reads in short pages and goes on.
    for minute in 0..40 {
        let key = format!("{:02}{:02}00_300", 8 + minute / 12, (minute % 12) * 5);
        window(
            root,
            DAY,
            &key,
            &many_readings(&format!("z{minute}"), 8 + minute / 12, (minute % 12) * 5, 1),
            None,
        );
    }
    let sparse = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "08:00", "time_to": "12:00"}),
    );
    let first = call(
        root,
        "get_transcript",
        json!({"reference": sparse["span_reference"]}),
    );
    assert_eq!(first["entries"].as_array().unwrap().len(), 24);
    assert!(first["next_cursor"].is_string());
}

#[test]
fn a_facet_limited_connection_reads_only_what_its_facet_holds() {
    let journal = journal();
    let root = journal.path();
    window(root, DAY, "063000_300", &[workout()], Some("health"));
    window(
        root,
        DAY,
        "063500_300",
        &many_readings("b", 6, 36, 2),
        Some("health"),
    );
    window(root, DAY, "064000_300", &many_readings("c", 6, 41, 2), None);
    let mut hidden = workout();
    hidden.id = "unfaceted".into();
    hidden.words = vec!["Hidden Walk".into()];
    window(root, "20261011", "063000_300", &[hidden], None);
    fs::write(
        root.join("chronicle").join(DAY).join("body.md"),
        "# body on Saturday, October 10, 2026\n\n- zanzibar\n",
    )
    .unwrap();
    index(root);

    grant(
        root,
        ReadScope::Facets {
            ids: vec![HEALTH.to_owned()],
        },
    );
    let found = call(root, "search", json!({"query": "Douro"}));
    assert_eq!(found["results"].as_array().unwrap().len(), 1, "{found}");
    let hidden = call(root, "search", json!({"query": "Hidden Walk"}));
    assert!(hidden["results"].as_array().unwrap().is_empty(), "{hidden}");
    let document = call(root, "search", json!({"query": "zanzibar"}));
    assert!(
        document["results"].as_array().unwrap().is_empty(),
        "{document}"
    );

    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert_eq!(list["transcripts"].as_array().unwrap().len(), 2);
    assert_eq!(
        list["gaps"],
        json!([{"from": "00:00", "to": "06:30"}, {"from": "06:40", "to": "24:00"}]),
        "the unfaceted window reads as nothing this connection can read"
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 3);
    assert!(!entries.iter().any(|entry| entry.contains("06:41")));
    let plain = call(root, "list_transcripts", json!({"day": DAY}));
    assert_eq!(plain["other_sources"], json!([LABEL]));
    let other_day = call(root, "list_transcripts", json!({"day": "20261011"}));
    assert!(other_day.get("other_sources").is_none(), "{other_day}");

    // Other content this connection can't read isn't hinted at.
    let card = root
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("000000_300");
    fs::create_dir_all(&card).unwrap();
    fs::write(card.join("day_summary_transcript.md"), "a card\n").unwrap();
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert!(list.get("gaps_note").is_none(), "{list}");

    // Limited to another facet: not a trace.
    grant(
        root,
        ReadScope::Facets {
            ids: vec![WORK.to_owned()],
        },
    );
    let nothing = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert!(nothing.get("gaps").is_none(), "{nothing}");
    let any_day = call(root, "list_transcripts", json!({}));
    assert!(any_day["transcripts"].as_array().unwrap().is_empty());
    let note = empty_note(root, json!({}));
    assert!(
        note.contains("complete answer"),
        "a limited connection learns nothing: {note}"
    );
    assert!(
        call(root, "list_transcripts", json!({"day": DAY}))
            .get("other_sources")
            .is_none()
    );

    // The whole journal sees all of it.
    grant(root, ReadScope::WholeJournal);
    let all = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert_eq!(all["transcripts"].as_array().unwrap().len(), 3);
    assert_eq!(
        call(root, "search", json!({"query": "Hidden Walk"}))["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

fn hour_reading(id: &str, at: &str) -> BodyRow {
    reading(id, &format!("2026-10-10 {at} -0600"), "60")
}

fn times(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry.get(11..19).unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn a_span_reads_in_time_order_across_overlapping_windows_and_only_inside_it() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    // An hour window, and two grid windows inside it holding a workout's own series.
    let hour: Vec<BodyRow> = ["06:05:00", "06:31:30", "06:36:30", "06:41:00", "06:59:00"]
        .iter()
        .enumerate()
        .map(|(n, at)| hour_reading(&format!("h{n}"), at))
        .collect();
    window(root, DAY, "060000_3600", &hour, None);
    let mut first = vec![workout(), hour_reading("g0", "06:32:00")];
    first.push(hour_reading("g1", "06:34:59"));
    window(root, DAY, "063000_300", &first, None);
    let mut tile = workout();
    tile.kind = RowKind::ExperienceTile;
    tile.id = "workout-1".into();
    tile.of = Some("workout".into());
    tile.index = Some(1);
    tile.count = Some(10);
    window(
        root,
        DAY,
        "063500_300",
        &[tile, hour_reading("g2", "06:35:10")],
        None,
    );
    let span = |from: &str, to: &str| {
        let list = call(
            root,
            "list_transcripts",
            json!({"day": DAY, "time_from": from, "time_to": to, "source": LABEL}),
        );
        list["span_reference"].as_str().unwrap().to_owned()
    };

    let (whole, _) = every_entry(root, &span("06:00", "07:00"));
    let clock = times(&whole);
    let timed: Vec<&String> = clock.iter().filter(|time| time.contains(':')).collect();
    let mut sorted = timed.clone();
    sorted.sort();
    assert_eq!(timed, sorted, "{whole:#?}");
    assert_eq!(whole.len(), 10, "{whole:#?}");

    // A ten-minute span: the readings inside it and the workout it overlaps, nothing else.
    let (narrow, _) = every_entry(root, &span("06:30", "06:40"));
    assert_eq!(narrow.len(), 7, "{narrow:#?}");
    assert!(narrow.iter().any(|entry| entry.contains("running")));
    assert!(narrow.iter().any(|entry| entry.contains("part 2 of 10")));
    assert!(
        !narrow
            .iter()
            .any(|entry| entry.contains("06:05:00") || entry.contains("06:41:00"))
    );
}

#[test]
fn a_row_an_archive_merge_left_a_second_copy_of_reads_once_from_the_live_copy() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    let at = "2026-10-10 09:05:00 -0600";
    window(
        root,
        DAY,
        "090000_3600",
        &[reading("r1", at, "60")],
        Some("health"),
    );
    window(
        root,
        DAY,
        "090000_3601",
        &[
            reading("r1", at, "61"),
            reading("r2", "2026-10-10 09:10:00 -0600", "62"),
        ],
        Some("health"),
    );
    index(root);
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(
        entries.iter().any(|entry| entry.contains("61")),
        "{entries:?}"
    );
    assert!(
        !entries.iter().any(|entry| entry.contains(" 60")),
        "{entries:?}"
    );
}

#[test]
fn a_connection_that_can_read_only_the_second_copy_still_gets_its_rows() {
    let journal = journal();
    let root = journal.path();
    // Limited to the facet of the copy the importer no longer writes to.
    grant(
        root,
        ReadScope::Facets {
            ids: vec![WORK.to_owned()],
        },
    );
    let at = "2026-10-10 09:05:00 -0600";
    window(
        root,
        DAY,
        "090000_3600",
        &[reading("r1", at, "60")],
        Some("work"),
    );
    window(
        root,
        DAY,
        "090000_3601",
        &[reading("r1", at, "61")],
        Some("health"),
    );
    index(root);
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(entries[0].contains("60"), "{entries:?}");
}

#[test]
fn a_kind_lists_only_the_windows_holding_it_and_says_when_none_do() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    let mut resting = hour_reading("r0", "07:00:00");
    resting.label = "resting heart rate".into();
    resting.record_type = "HKQuantityTypeIdentifierRestingHeartRate".into();
    window(
        root,
        DAY,
        "060000_3600",
        &[hour_reading("h0", "06:10:00")],
        None,
    );
    window(
        root,
        DAY,
        "070000_3600",
        &[resting, hour_reading("h1", "07:10:00")],
        None,
    );
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "kind": "Resting Heart Rate"}),
    );
    let listed = list["transcripts"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "{list}");
    assert_eq!(listed[0]["time"], "07:00-08:00");
    let (entries, pages) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert_eq!(pages, vec![1]);
    assert!(entries[0].contains("resting heart rate"), "{entries:?}");

    // Nothing of that kind reads differently from nothing recorded.
    let absent = empty_note(
        root,
        json!({"day": DAY, "source": LABEL, "kind": "blood oxygen"}),
    );
    assert!(absent.contains("none of the kind"), "{absent}");
    let nothing = empty_note(
        root,
        json!({"day": "20261011", "source": LABEL, "kind": "blood oxygen"}),
    );
    assert!(
        nothing.starts_with("No source with that label"),
        "{nothing}"
    );
    // A range with no window at all reads as it would without a kind.
    let quiet = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "kind": "blood oxygen", "time_from": "12:00", "time_to": "13:00"}),
    );
    assert!(
        quiet["transcripts"].as_array().unwrap().is_empty(),
        "{quiet}"
    );
    let quiet_note = empty_note(
        root,
        json!({"day": DAY, "source": LABEL, "kind": "blood oxygen", "time_from": "12:00", "time_to": "13:00"}),
    );
    assert!(!quiet_note.starts_with("No body source"), "{quiet_note}");

    // A window overlapping the range with no entry inside it isn't "entries in that time".
    let overlap = empty_note(
        root,
        json!({"day": DAY, "source": LABEL, "kind": "blood oxygen", "time_from": "06:30", "time_to": "06:45"}),
    );
    assert!(!overlap.contains("none of the kind"), "{overlap}");

    // Windows without the kind don't use up a read: one call over many of them.
    for hour in 8..20 {
        for slot in [0, 5, 10] {
            window(
                root,
                DAY,
                &format!("{hour:02}{slot:02}00_300"),
                &[hour_reading(
                    &format!("x{hour}{slot}"),
                    &format!("{hour:02}:{slot:02}:30"),
                )],
                None,
            );
        }
    }
    let mut late = hour_reading("r1", "19:59:00");
    late.label = "resting heart rate".into();
    window(root, DAY, "195500_300", &[late], None);
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "kind": "resting heart rate"}),
    );
    let (entries, pages) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert_eq!(pages, vec![2], "{entries:?}");

    // A kind needs a source.
    assert!(
        run_mcp_probe(
            root,
            CONNECTION,
            "list_transcripts",
            &json!({"day": DAY, "kind": "sleep"}),
        )
        .is_err()
    );
}

fn fix_zone(root: &Path, zone: &str) {
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        json!({ "zone": zone }).to_string(),
    )
    .unwrap();
}

/// Rows an export wrote at `-0400`, in windows on the Denver zone it was placed by.
fn rows_on_another_offset(root: &Path) {
    // 08:00-09:00 Denver holds 150 readings the export wrote at -0400 (10:0x).
    let rows: Vec<BodyRow> = (0..150)
        .map(|n| {
            reading(
                &format!("a{n:03}"),
                &format!("2026-10-10 10:{:02}:{:02} -0400", n / 60, n % 60),
                &format!("{}", 1000 + n),
            )
        })
        .collect();
    window(root, DAY, "080000_3600", &rows, None);
    // A 300-second window beside it, the row on the Denver clock's 08:01.
    window(
        root,
        DAY,
        "080000_300",
        &[reading("tile", "2026-10-10 10:01:00 -0400", "2000")],
        None,
    );
    // 22:30 Denver, which this export wrote as the next day's 00:30.
    window(
        root,
        DAY,
        "220000_3600",
        &[reading("late", "2026-10-11 00:30:00 -0400", "71")],
        None,
    );
}

#[test]
fn rows_on_another_offset_are_all_found_in_order_across_pages_and_the_days_edge() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    fix_zone(root, "America/Denver");
    rows_on_another_offset(root);
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    let span = list["span_reference"].as_str().unwrap().to_owned();
    let (entries, pages) = every_entry(root, &span);
    assert_eq!(entries.len(), 152, "pages {pages:?}");
    assert_eq!(list.get("unreadable"), None);
    // In instant order, each keeping the offset it states.
    let times: Vec<&str> = entries.iter().map(|entry| &entry[..25]).collect();
    let mut sorted = times.clone();
    sorted.sort_unstable();
    assert_eq!(times, sorted);
    assert!(
        entries
            .last()
            .unwrap()
            .starts_with("2026-10-11 00:30:00 -0400")
    );

    // A span an agent asks for reads on the windows' clock.
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "08:01", "time_to": "08:02"}),
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 61, "{entries:?}");
    assert!(
        entries
            .iter()
            .all(|entry| entry.starts_with("2026-10-10 10:01"))
    );
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "22:00", "time_to": "23:00"}),
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 1, "{entries:?}");
}

#[test]
fn with_no_fixed_zone_a_read_still_loses_no_row() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    rows_on_another_offset(root);
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    let span = list["span_reference"].as_str().unwrap().to_owned();
    let (entries, pages) = every_entry(root, &span);
    assert_eq!(entries.len(), 152, "pages {pages:?}");
}

#[test]
fn a_clock_change_day_serves_every_row_in_instant_order() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    fix_zone(root, "America/Denver");
    // 2026-11-01 in Denver: 01:30 happens twice, -0600 and then -0700.
    window(
        root,
        "20261101",
        "010000_3600",
        &[
            reading("second", "2026-11-01 01:30:00 -0700", "2"),
            reading("first", "2026-11-01 01:30:00 -0600", "1"),
        ],
        None,
    );
    window(
        root,
        "20261101",
        "020000_3600",
        &[reading("after", "2026-11-01 02:00:00 -0700", "3")],
        None,
    );
    let list = call(
        root,
        "list_transcripts",
        json!({"day": "20261101", "source": LABEL}),
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    let values: Vec<&str> = entries.iter().map(|entry| &entry[..25]).collect();
    assert_eq!(
        values,
        [
            "2026-11-01 01:30:00 -0600",
            "2026-11-01 01:30:00 -0700",
            "2026-11-01 02:00:00 -0700"
        ]
    );
}

#[test]
fn under_a_kind_a_window_that_cant_be_read_is_counted_not_called_empty_of_it() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    window(
        root,
        DAY,
        "080000_3600",
        &[reading("a", "2026-10-10 08:10:00 -0600", "70")],
        None,
    );
    fs::write(
        root.join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join("080000_3600/body.jsonl"),
        "not a row\nnor this\n",
    )
    .unwrap();
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "kind": "resting heart rate"}),
    );
    assert_eq!(list["unreadable"], json!(1), "{list}");
    assert!(!list.to_string().contains("none of the kind"), "{list}");
    let (_, _) = every_entry(root, list["span_reference"].as_str().unwrap());
}

fn interval(kind: RowKind, id_label: (&str, &str), start: &str, end: &str, value: &str) -> BodyRow {
    let mut row = BodyRow::new(
        kind,
        id_label.0.into(),
        "apple_health",
        "HKQuantityTypeIdentifierStepCount",
        id_label.1.into(),
    );
    row.start = Some(start.into());
    row.end = Some(end.into());
    row.value = Some(value.into());
    row
}

#[test]
fn a_narrow_span_finds_an_interval_that_began_in_the_window_before_it() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    fix_zone(root, "America/Denver");
    window(
        root,
        DAY,
        "060000_3600",
        &[
            interval(
                RowKind::Count,
                ("c1", "step count"),
                "2026-10-10 06:50:00 -0600",
                "2026-10-10 07:10:00 -0600",
                "val900",
            ),
            interval(
                RowKind::Stage,
                ("s1", "asleep core"),
                "2026-10-10 06:40:00 -0600",
                "2026-10-10 07:30:00 -0600",
                "val901",
            ),
            interval(
                RowKind::Count,
                ("c2", "step count"),
                "2026-10-10 06:10:00 -0600",
                "2026-10-10 06:20:00 -0600",
                "val902",
            ),
            reading("r0", "2026-10-10 06:30:00 -0600", "val903"),
        ],
        None,
    );
    window(
        root,
        DAY,
        "070000_3600",
        &[reading("r1", "2026-10-10 07:02:00 -0600", "val904")],
        None,
    );
    let read = |extra: Value| {
        let mut arguments =
            json!({"day": DAY, "source": LABEL, "time_from": "07:00", "time_to": "07:05"});
        for (key, value) in extra.as_object().unwrap() {
            arguments[key] = value.clone();
        }
        call(root, "list_transcripts", arguments)
    };
    let list = read(json!({}));
    assert_eq!(list["transcripts"].as_array().unwrap().len(), 1, "{list}");
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    let text = entries.join("\n");
    for held in ["val900", "asleep core", "val904"] {
        assert!(text.contains(held), "{held} isn't served: {text}");
    }
    // What the window before holds and doesn't overlap the span stays out.
    assert!(
        !text.contains("val902") && !text.contains("val903"),
        "{text}"
    );
    assert_eq!(
        list["gaps"],
        json!([]),
        "gaps still describe the span: {list}"
    );

    // Under a kind only the window before holds, nothing is listed as in the range,
    // the answer says what's there, and the span reference reaches it.
    let list = read(json!({"kind": "step count"}));
    assert_eq!(list["transcripts"], json!([]), "{list}");
    assert!(list["began_earlier"].is_string(), "{list}");
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert!(entries.join("\n").contains("val900"), "{entries:?}");
    let list = read(json!({"kind": "heart rate"}));
    assert_eq!(list["transcripts"].as_array().unwrap().len(), 1, "{list}");
    assert!(list.get("began_earlier").is_none(), "{list}");
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert!(!entries.join("\n").contains("val903"), "{entries:?}");
    // A kind none of them hold reads as the empty list that says so.
    let list = read(json!({"kind": "respiratory rate"}));
    assert_eq!(list["transcripts"], json!([]), "{list}");
    assert!(list.get("span_reference").is_none(), "{list}");
}

#[test]
fn with_nothing_in_the_range_an_interval_from_before_it_is_said_and_reached() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    fix_zone(root, "America/Denver");
    window(
        root,
        DAY,
        "060000_3600",
        &[interval(
            RowKind::Stage,
            ("s1", "asleep core"),
            "2026-10-10 06:40:00 -0600",
            "2026-10-10 07:30:00 -0600",
            "val901",
        )],
        None,
    );
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "07:10", "time_to": "07:15"}),
    );
    assert_eq!(list["transcripts"], json!([]), "{list}");
    assert!(list["began_earlier"].is_string(), "{list}");
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert!(entries.join("\n").contains("asleep core"), "{entries:?}");
    // One that ended before the span gives no such answer.
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "07:40", "time_to": "07:45"}),
    );
    assert!(list.get("began_earlier").is_none(), "{list}");
}

/// A two-hour workout as the writer tiles it: its first row and a tile a slot.
fn tiled_workout(root: &Path) {
    let start = "2026-10-10 06:00:00 -0600";
    let end = "2026-10-10 08:00:00 -0600";
    let mut first = workout();
    first.start = Some(start.into());
    first.end = Some(end.into());
    window(root, DAY, "060000_300", std::slice::from_ref(&first), None);
    for index in 1..24_u64 {
        let minutes = 6 * 60 + index * 5;
        let mut tile = BodyRow::new(
            RowKind::ExperienceTile,
            format!("workout-{index}"),
            "apple_health",
            "HKWorkoutActivityTypeRunning",
            "running".into(),
        );
        tile.of = Some("workout".into());
        tile.experience = Some("workout".into());
        tile.by = Some("Synthetic Watch".into());
        tile.start = Some(start.into());
        tile.end = Some(end.into());
        tile.index = Some(index);
        tile.count = Some(24);
        window(
            root,
            DAY,
            &format!("{:02}{:02}00_300", minutes / 60, minutes % 60),
            &[tile],
            None,
        );
    }
}

/// An experience has a tile in every slot, so looking back adds none of them.
#[test]
fn a_narrow_span_over_a_workout_serves_the_tile_it_overlaps_and_no_more() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    fix_zone(root, "America/Denver");
    tiled_workout(root);
    for extra in [json!({}), json!({"kind": "workout"})] {
        let mut arguments =
            json!({"day": DAY, "source": LABEL, "time_from": "07:30", "time_to": "07:35"});
        for (key, value) in extra.as_object().unwrap() {
            arguments[key] = value.clone();
        }
        let list = call(root, "list_transcripts", arguments);
        let times: Vec<&str> = list["transcripts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["time"].as_str().unwrap())
            .collect();
        assert_eq!(times, ["07:30-07:35"], "{extra}: {list}");
        let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
        assert_eq!(entries.len(), 1, "{extra}: {entries:#?}");
    }
}

#[test]
fn a_damaged_line_in_the_window_before_a_span_isnt_served() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    fix_zone(root, "America/Denver");
    window(
        root,
        DAY,
        "060000_3600",
        &[reading("r0", "2026-10-10 06:30:00 -0600", "val903")],
        None,
    );
    let path = root
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("060000_3600/body.jsonl");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("{\"not a row\": \"DAMAGED-OUTSIDE-SPAN\"}\n");
    fs::write(&path, text).unwrap();
    window(
        root,
        DAY,
        "070000_3600",
        &[reading("r1", "2026-10-10 07:02:00 -0600", "val904")],
        None,
    );
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "07:00", "time_to": "07:05"}),
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert!(
        !entries.join("\n").contains("DAMAGED-OUTSIDE-SPAN"),
        "{entries:#?}"
    );
}

#[test]
fn looking_back_pages_without_doubling_or_reaching_past_the_window_before() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    fix_zone(root, "America/Denver");
    let before: Vec<BodyRow> = (0..250_u32)
        .map(|index| {
            interval(
                RowKind::Count,
                (&format!("c{index:03}"), "step count"),
                &format!(
                    "2026-10-10 06:{:02}:{:02} -0600",
                    30 + index / 60,
                    index % 60
                ),
                "2026-10-10 07:10:00 -0600",
                &format!("cv{index:03}x"),
            )
        })
        .collect();
    window(root, DAY, "060000_3600", &before, None);
    let inside: Vec<BodyRow> = (0..250_u32)
        .map(|index| {
            reading(
                &format!("r{index:03}"),
                &format!("2026-10-10 07:{:02}:{:02} -0600", index / 60, index % 60),
                &format!("rv{index:03}x"),
            )
        })
        .collect();
    window(root, DAY, "070000_3600", &inside, None);
    window(
        root,
        DAY,
        "050000_3600",
        &[interval(
            RowKind::Count,
            ("w", "step count"),
            "2026-10-10 05:00:00 -0600",
            "2026-10-10 05:10:00 -0600",
            "outside",
        )],
        None,
    );
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "07:00", "time_to": "07:05"}),
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    let unique: std::collections::BTreeSet<&String> = entries.iter().collect();
    assert_eq!(unique.len(), entries.len());
    // Every count runs into the span, and 07:00-07:05 holds 250 readings' first 250 seconds.
    assert_eq!(entries.len(), 500);
    assert!(!entries.join("\n").contains("outside"));
}

#[test]
fn a_limited_connection_doesnt_look_back_into_what_it_cant_read() {
    let journal = journal();
    let root = journal.path();
    grant(
        root,
        ReadScope::Facets {
            ids: vec![HEALTH.to_owned()],
        },
    );
    fix_zone(root, "America/Denver");
    window(
        root,
        DAY,
        "060000_3600",
        &[interval(
            RowKind::Count,
            ("c1", "step count"),
            "2026-10-10 06:50:00 -0600",
            "2026-10-10 07:10:00 -0600",
            "SECRET-WORK",
        )],
        Some("work"),
    );
    window(
        root,
        DAY,
        "070000_3600",
        &[reading("r1", "2026-10-10 07:02:00 -0600", "val904")],
        Some("health"),
    );
    for extra in [json!({}), json!({"kind": "step count"})] {
        let mut arguments =
            json!({"day": DAY, "source": LABEL, "time_from": "07:00", "time_to": "07:05"});
        for (key, value) in extra.as_object().unwrap() {
            arguments[key] = value.clone();
        }
        let list = call(root, "list_transcripts", arguments);
        assert!(!list.to_string().contains("SECRET-WORK"), "{list}");
        assert!(list.get("began_earlier").is_none(), "{list}");
        if let Some(span) = list["span_reference"].as_str() {
            let (entries, _) = every_entry(root, span);
            assert!(!entries.join("\n").contains("SECRET-WORK"), "{entries:?}");
        }
    }
}

/// The contract's limit, stated in a test: an interval that began before the look-back
/// isn't found by a narrow span, and a whole-day read finds it.
#[test]
fn an_interval_that_began_before_the_look_back_is_found_by_a_whole_day_read() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    fix_zone(root, "America/Denver");
    window(
        root,
        DAY,
        "050000_3600",
        &[interval(
            RowKind::Count,
            ("c1", "step count"),
            "2026-10-10 05:50:00 -0600",
            "2026-10-10 07:10:00 -0600",
            "val900",
        )],
        None,
    );
    window(
        root,
        DAY,
        "070000_3600",
        &[reading("r1", "2026-10-10 07:02:00 -0600", "val904")],
        None,
    );
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL, "time_from": "07:00", "time_to": "07:05"}),
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert!(!entries.join("\n").contains("val900"), "{entries:?}");
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert!(entries.join("\n").contains("val900"), "{entries:?}");
}

const OURA: &str = "import.oura";
const OURA_LABEL: &str = "Oura · imported";

/// [`journal`], with Oura's stream as its writer registers it and the zone it
/// placed its windows on.
fn oura_journal() -> tempfile::TempDir {
    let journal = journal();
    let root = journal.path();
    fs::write(
        root.join("streams/import.oura.json"),
        json!({
            "name": OURA, "kind": "import:oura", "host": null, "platform": null,
            "created_at": 1, "last_day": null, "last_segment": null, "seq": 1,
        })
        .to_string(),
    )
    .unwrap();
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.oura-zone.json"),
        r#"{"zone":"America/Denver"}"#,
    )
    .unwrap();
    journal
}

/// A row as Oura's reader writes it: `by` Oura, the whole document in `fields`.
fn oura_row(kind: RowKind, id: &str, record_type: &str, label: &str, document: Value) -> BodyRow {
    let mut row = BodyRow::new(kind, id.into(), "oura", record_type, label.into());
    row.by = Some("Oura".into());
    row.fields = document
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    row
}

fn oura_heart_rate(id: &str, at: &str, bpm: &str, source: &str) -> BodyRow {
    let mut row = oura_row(
        RowKind::Reading,
        id,
        "heartrate",
        "heart rate",
        json!({"timestamp": at, "bpm": bpm.parse::<u32>().unwrap(), "source": source}),
    );
    row.at = Some(at.into());
    row.value = Some(bpm.into());
    row.unit = Some("count/min".into());
    row
}

/// A day of Oura: every daily document, with what each judges, a walk the owner
/// labelled with heart rate during it, heart rate either side, and a tag.
fn oura_day(root: &Path) {
    let daily = [
        (
            "daily_readiness",
            "daily readiness",
            json!({"score": 82,
            "contributors": {"hrv_balance": 63, "resting_heart_rate": 91}}),
        ),
        (
            "daily_sleep",
            "daily sleep",
            json!({"score": 74, "contributors": {"deep_sleep": 63}}),
        ),
        (
            "daily_activity",
            "daily activity",
            json!({"score": 91, "class_5_min": "0012345"}),
        ),
        (
            "daily_stress",
            "daily stress",
            json!({"day_summary": "stressful",
            "stress_high": 5400, "recovery_high": 1800}),
        ),
        (
            "daily_resilience",
            "daily resilience",
            json!({"level": "limited",
            "contributors": {"sleep_recovery": 41.2}}),
        ),
        (
            "daily_spo2",
            "daily SpO2",
            json!({"spo2_percentage": {"average": 96.5}}),
        ),
        (
            "daily_cardiovascular_age",
            "cardiovascular age",
            json!({"vascular_age": 47}),
        ),
        ("vO2_max", "VO2 max", json!({"vo2_max": 39})),
    ]
    .into_iter()
    .map(|(endpoint, label, mut document)| {
        document["id"] = json!(format!("synthetic-{endpoint}"));
        document["day"] = json!("2026-10-10");
        let mut row = oura_row(RowKind::Dated, endpoint, endpoint, label, document);
        row.date = Some("2026-10-10".into());
        row
    })
    .collect::<Vec<_>>();
    window_in(root, OURA, DAY, "000000_86400", &daily, None);
    window_in(
        root,
        OURA,
        DAY,
        "060000_3600",
        &[
            oura_heart_rate("h1", "2026-10-10T12:10:00.000Z", "7001", "rest"),
            oura_heart_rate("h2", "2026-10-10T12:15:00.000Z", "7002", "rest"),
        ],
        None,
    );
    let mut walk = oura_row(
        RowKind::Experience,
        "walk",
        "walking",
        "walking",
        json!({"id": "synthetic-workout-walk", "activity": "walking", "calories": 148.5,
            "day": "2026-10-10", "distance": 2412.9, "intensity": "hard",
            "label": "synthetic hill loop", "source": "confirmed",
            "start_datetime": "2026-10-10T08:05:00.000-06:00",
            "end_datetime": "2026-10-10T08:41:00.000-06:00"}),
    );
    walk.experience = Some("workout".into());
    walk.start = Some("2026-10-10T08:05:00.000-06:00".into());
    walk.end = Some("2026-10-10T08:41:00.000-06:00".into());
    walk.words = vec!["synthetic hill loop".into()];
    walk.values.insert("distance".into(), "2412.9 m".into());
    walk.values.insert("energy".into(), "148.5 kcal".into());
    window_in(
        root,
        OURA,
        DAY,
        "080500_300",
        &[
            walk,
            oura_heart_rate("h3", "2026-10-10T14:06:00.000Z", "7003", "workout"),
            oura_heart_rate("h4", "2026-10-10T14:08:00.000Z", "7004", "workout"),
        ],
        None,
    );
    let mut tag = oura_row(
        RowKind::Logged,
        "tag",
        "tag_generic_caffeine",
        "caffeine",
        json!({"id": "synthetic-tag", "tag_type_code": "tag_generic_caffeine",
            "start_time": "2026-10-10T09:15:00-06:00", "comment": "second cup"}),
    );
    tag.start = Some("2026-10-10T09:15:00-06:00".into());
    tag.words = vec!["second cup".into()];
    window_in(
        root,
        OURA,
        DAY,
        "090000_3600",
        &[
            tag,
            oura_heart_rate("h5", "2026-10-10T15:20:00.000Z", "7005", "awake"),
        ],
        None,
    );
    solstone_core_location::body_day::BodyDays::default().reconcile(root);
}

/// The proof Apple's first test gives, on Oura: a search hit, its day, the
/// workout whole, and a source's heart rate paged across a day with its gaps.
#[test]
fn an_agent_finds_an_oura_workout_and_its_day_and_pages_its_heart_rate() {
    let journal = oura_journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    oura_day(root);
    index(root);

    // Search finds the workout by the words a person would type, and its day.
    for query in ["walking", "synthetic hill loop"] {
        let found = call(root, "search", json!({"query": query, "day": DAY}));
        let hit = found["results"].as_array().unwrap().iter().find(|result| {
            result["snippet"]
                .as_str()
                .unwrap_or_default()
                .contains("hill loop")
        });
        assert!(hit.is_some(), "{query}: {found}");
    }
    for query in ["Saturday October", "daily readiness", "caffeine"] {
        let day = call(root, "search", json!({"query": query, "day": DAY}));
        assert!(
            !day["results"].as_array().unwrap().is_empty(),
            "{query}: {day}"
        );
    }
    // Held reading bounds appear in body.md, while interior samples and raw vendor fields stay out.
    for query in ["7001", "7005"] {
        let found = call(root, "search", json!({"query": query, "day": DAY}));
        assert!(
            !found["results"].as_array().unwrap().is_empty(),
            "{query}: {found}"
        );
    }
    for query in ["7003", "intensity", "confirmed"] {
        let found = call(root, "search", json!({"query": query, "day": DAY}));
        assert!(
            found["results"].as_array().unwrap().is_empty(),
            "{query}: {found}"
        );
    }

    // The workout hit's link reaches the workout with every field.
    let found = call(
        root,
        "search",
        json!({"query": "synthetic hill loop", "day": DAY}),
    );
    let workout_hit = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row.get("segments")
                .and_then(Value::as_array)
                .is_some_and(|segs| !segs.is_empty())
        })
        .expect("workout hit with segment link");
    let fetched = call(
        root,
        "fetch",
        json!({"reference": workout_hit["reference"]}),
    );
    assert!(fetched.to_string().contains("2412.9 m"), "{fetched}");
    let link = fetched["segments"][0]["reference"].as_str().unwrap();
    let tile = call(root, "get_transcript", json!({"reference": link}));
    let text = tile["entries"].to_string().replace('\\', "");
    for field in [
        "workout walking",
        "Oura",
        "intensity: hard",
        "source: confirmed",
        "calories: 148.5",
        "distance: 2412.9",
        "label: synthetic hill loop",
    ] {
        assert!(text.contains(field), "{field}: {text}");
    }
    assert_eq!(tile["source"], OURA_LABEL, "{tile}");
    assert!(!tile.to_string().contains(OURA), "{tile}");

    // One source's heart rate across the morning: the windows holding it, and
    // what no window covers. The whole-day documents cover nothing.
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "time_from": "06:00", "time_to": "10:00",
            "source": OURA_LABEL, "kind": "heart rate"}),
    );
    let times: Vec<&str> = list["transcripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|listed| listed["time"].as_str().unwrap())
        .collect();
    assert_eq!(
        times,
        ["06:00-07:00", "08:05-08:10", "09:00-10:00"],
        "{list}"
    );
    assert_eq!(
        list["gaps"],
        json!([{"from": "07:00", "to": "08:05"}, {"from": "08:10", "to": "09:00"}]),
        "{list}"
    );
    assert!(!list.to_string().contains(OURA), "{list}");
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 5, "{entries:?}");
    for (entry, bpm) in entries.iter().zip(["7001", "7002", "7003", "7004", "7005"]) {
        assert!(
            entry.contains(&format!("heart rate {bpm} count/min")) && entry.contains("Oura"),
            "{entry}"
        );
    }
    assert!(
        !entries
            .iter()
            .any(|entry| entry.contains("walking") || entry.contains("caffeine"))
    );
}

/// What Oura judges stays out of everything the journal writes, and an agent
/// still reads it whole, attributed to Oura.
#[test]
fn oura_judgements_never_reach_search_and_an_agent_reads_them_attributed() {
    let journal = oura_journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    oura_day(root);
    index(root);
    for query in ["stressful", "limited", "contributors", "day_summary"] {
        let found = call(root, "search", json!({"query": query}));
        assert!(
            found["results"].as_array().unwrap().is_empty(),
            "{query}: {found}"
        );
    }
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": OURA_LABEL}),
    );
    let whole_day = list["transcripts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["time"] == "00:00-23:59")
        .unwrap_or_else(|| panic!("{list}"));
    let read = call(
        root,
        "get_transcript",
        json!({"reference": whole_day["reference"]}),
    );
    assert_eq!(read["source"], OURA_LABEL, "{read}");
    // Entries are escaped for markdown, as every transcript's are.
    let entries: Vec<String> = read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.as_str().unwrap().replace('\\', ""))
        .collect();
    assert_eq!(entries.len(), 8, "{read}");
    let stress = entries
        .iter()
        .find(|entry| entry.contains("daily stress"))
        .unwrap_or_else(|| panic!("{read}"));
    assert!(
        stress.starts_with("for 2026-10-10 daily stress · Oura"),
        "{stress}"
    );
    assert!(stress.contains("day_summary: stressful"), "{stress}");
    let text = entries.join("\n");
    assert!(text.contains("level: limited"), "{text}");
    assert!(text.contains("score: 82"), "{text}");
}

fn oura_moved(id: &str, of: &str) -> BodyRow {
    let mut row = oura_row(RowKind::Moved, id, "walking", "walking", json!({}));
    row.by = None;
    row.fields.clear();
    row.of = Some(of.into());
    row
}

fn oura_walk(start: &str, end: &str) -> BodyRow {
    let mut walk = oura_row(
        RowKind::Experience,
        "walk",
        "walking",
        "walking",
        json!({"id": "synthetic-workout-walk", "label": "synthetic hill loop"}),
    );
    walk.experience = Some("workout".into());
    walk.start = Some(start.into());
    walk.end = Some(end.into());
    walk.words = vec!["synthetic hill loop".into()];
    walk
}

/// A window whose rows all moved elsewhere is no window to any reader: search
/// loses its chunk, a listing covers nothing with it and names no source for it,
/// and a read's window budget doesn't spend on it. Its twin, a mark beside a live
/// reading, is a window like any other.
#[test]
fn a_window_holding_only_moved_rows_is_absent_and_one_with_a_live_reading_is_present() {
    let journal = oura_journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    // The walk first written at 07:00, and found by search there.
    window_in(
        root,
        OURA,
        DAY,
        "070000_300",
        &[oura_walk(
            "2026-10-10T07:00:00.000-06:00",
            "2026-10-10T07:04:00.000-06:00",
        )],
        None,
    );
    index(root);
    let hits = |root: &Path| {
        call(
            root,
            "search",
            json!({"query": "synthetic hill loop", "day": DAY}),
        )["results"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(hits(root), 1);

    // Oura moved it to 08:05: the old window holds only its mark, the new one the
    // walk and its old tile's mark, and an hour holds a mark beside a reading.
    window_in(
        root,
        OURA,
        DAY,
        "070000_300",
        &[oura_moved("walk", "walk")],
        None,
    );
    // The index reads a file again when its time moves, as a publication's does.
    fs::File::options()
        .write(true)
        .open(
            root.join("chronicle")
                .join(DAY)
                .join(OURA)
                .join("070000_300/body.jsonl"),
        )
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    let mut moved_walk = oura_walk(
        "2026-10-10T08:05:00.000-06:00",
        "2026-10-10T08:09:00.000-06:00",
    );
    moved_walk.id = "walk".into();
    window_in(
        root,
        OURA,
        DAY,
        "080500_300",
        &[moved_walk, oura_moved("walk-1", "walk")],
        None,
    );
    window_in(
        root,
        OURA,
        DAY,
        "090000_3600",
        &[
            oura_moved("walk@20261010/090000_3600", "walk"),
            oura_heart_rate("h5", "2026-10-10T15:20:00.000Z", "7005", "awake"),
        ],
        None,
    );
    index(root);
    // Search holds one chunk for the walk, from its new window.
    let found = call(
        root,
        "search",
        json!({"query": "synthetic hill loop", "day": DAY}),
    );
    let results = found["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{found}");
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap_or_default()
            .replace('\\', "")
            .contains("08:05 -0600"),
        "{found}"
    );

    // The listing: the marked hour is listed, the moved-only window isn't, and
    // what it covered is a gap.
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": OURA_LABEL}),
    );
    let times: Vec<&str> = list["transcripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|listed| listed["time"].as_str().unwrap())
        .collect();
    assert_eq!(times, ["08:05-08:10", "09:00-10:00"], "{list}");
    assert_eq!(
        list["gaps"],
        json!([
            {"from": "00:00", "to": "08:05"},
            {"from": "08:10", "to": "09:00"},
            {"from": "10:00", "to": "24:00"},
        ]),
        "{list}"
    );
    // Its entries are the live rows only.
    let (entries, _) = every_entry(root, list["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(
        entries[0].starts_with("workout walking, 2026-10-10T08:05"),
        "{entries:?}"
    );
    assert!(entries[1].contains("heart rate 7005"), "{entries:?}");
    // A window's own page reads its live rows and no mark.
    let marked = list["transcripts"][0]["reference"].as_str().unwrap();
    let page = call(root, "get_transcript", json!({"reference": marked}));
    assert_eq!(page["entries"].as_array().unwrap().len(), 1, "{page}");

    // A day of only marks names no source, and a dayless list says no body data.
    let next = "20261011";
    window_in(
        root,
        OURA,
        next,
        "070000_300",
        &[oura_moved("walk-2", "walk")],
        None,
    );
    let day = call(root, "list_transcripts", json!({"day": next}));
    assert!(day.get("other_sources").is_none(), "{day}");
    let only_marks = oura_journal();
    grant(only_marks.path(), ReadScope::WholeJournal);
    window_in(
        only_marks.path(),
        OURA,
        DAY,
        "070000_300",
        &[oura_moved("walk", "walk")],
        None,
    );
    let note = empty_note(only_marks.path(), json!({}));
    assert!(!note.contains("List a day"), "{note}");
    assert!(
        call(only_marks.path(), "list_transcripts", json!({"day": DAY}))
            .get("other_sources")
            .is_none()
    );
    // The twin: a mark beside a live reading is body data, and names its source.
    window_in(
        only_marks.path(),
        OURA,
        DAY,
        "090000_3600",
        &[
            oura_moved("walk@20261010/090000_3600", "walk"),
            oura_heart_rate("h5", "2026-10-10T15:20:00.000Z", "7005", "awake"),
        ],
        None,
    );
    assert!(empty_note(only_marks.path(), json!({})).contains("List a day"));
    assert_eq!(
        call(only_marks.path(), "list_transcripts", json!({"day": DAY}))["other_sources"],
        json!([OURA_LABEL])
    );
}

/// Windows of marks don't spend a read's budget: a day with more of them than one
/// read visits still serves its live rows on the first page.
#[test]
fn moved_only_windows_spend_nothing_of_a_reads_window_budget() {
    let journal = oura_journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    for minute in 0..30 {
        let key = format!("06{:02}00_300", minute * 2);
        window_in(
            root,
            OURA,
            DAY,
            &key,
            &[oura_moved(&format!("walk@{DAY}/{key}"), "walk")],
            None,
        );
    }
    window_in(
        root,
        OURA,
        DAY,
        "070000_3600",
        &[
            oura_heart_rate("h1", "2026-10-10T13:10:00.000Z", "7001", "rest"),
            oura_heart_rate("h2", "2026-10-10T13:15:00.000Z", "7002", "rest"),
        ],
        None,
    );
    let list = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": OURA_LABEL}),
    );
    let page = call(
        root,
        "get_transcript",
        json!({"reference": list["span_reference"]}),
    );
    assert_eq!(page["entries"].as_array().unwrap().len(), 2, "{page}");
    assert!(page["next_cursor"].is_null(), "{page}");
}

#[test]
fn a_strava_line_a_code_record_holds_isnt_listed() {
    let journal = self::journal();
    let root = journal.path();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "body".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let line = json!({"id": "exercise_1", "activity": "exercise", "title": "Morning Run",
        "stream": "import.strava", "segments": ["070000_300"], "created_at": 1});
    let code = json!({"id": "body_code", "source": "body", "activity": "workout",
        "account": {"key": "import.strava\u{1f}1"},
        "converted": {"primary": "p", "decided_at": "t", "lines": {}}});
    let write = |code: &Value| {
        let path = root.join(format!("facets/health/activities/{DAY}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{line}\n{code}\n")).unwrap();
    };
    let titles = || {
        call(root, "list_activities", json!({"day": DAY}))["activities"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["title"] == "Morning Run")
            .count()
    };
    write(&code);
    assert_eq!(titles(), 1, "a line no record holds is listed");
    let mut holding = code.clone();
    holding["converted"]["lines"]
        .as_object_mut()
        .unwrap()
        .insert(
            format!("{DAY}|health|exercise_1"),
            json!({"sha256": "x", "hidden": false, "fields": {}}),
        );
    write(&holding);
    assert_eq!(titles(), 0, "a line a record holds isn't");
}

const OLD_DAY: &str = "20260102";

fn old_bundles_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/body_old_bundles")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn with_old_native(journal: &Path) {
    let name = "body-01M70HDTD9N4P15X06CRDG2DWZ";
    copy_tree(
        &old_bundles_dir().join("native").join(name),
        &journal.join("imports").join(name),
    );
}

fn with_old_python(journal: &Path) {
    copy_tree(
        &old_bundles_dir().join("python_era/20260304_080000"),
        &journal.join("imports/20260304_080000"),
    );
}

#[test]
fn a_day_only_an_old_bundle_covers_is_noted_for_a_whole_journal_connection() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    with_old_native(root);
    // The day has no folder of its own: the note still reaches the listing.
    assert!(!root.join("chronicle").join(OLD_DAY).exists());
    let list = call(root, "list_transcripts", json!({"day": OLD_DAY}));
    assert_eq!(list["transcripts"], json!([]));
    let note = list["earlier_imports_note"].as_str().expect("the note");
    assert!(note.contains("Apple Health"), "{note}");
    assert!(note.contains("this day"), "{note}");

    // Without body the note is withheld, and the listing says why.
    grant_kinds(root, &["transcripts"], ReadScope::WholeJournal);
    let list = call(root, "list_transcripts", json!({"day": OLD_DAY}));
    assert!(list.get("earlier_imports_note").is_none(), "{list}");
    assert_eq!(list["body_note"], BODY_WORDS, "{list}");
}

#[test]
fn a_limited_connection_gets_no_note() {
    let journal = journal();
    let root = journal.path();
    grant(
        root,
        ReadScope::Facets {
            ids: vec![HEALTH.to_owned()],
        },
    );
    with_old_native(root);
    let list = call(root, "list_transcripts", json!({"day": OLD_DAY}));
    assert!(list.get("earlier_imports_note").is_none(), "{list}");
}

#[test]
fn a_connection_without_transcripts_gets_no_note() {
    let journal = journal();
    let root = journal.path();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    with_old_native(root);
    // list_transcripts itself is off without Transcripts, so nothing is said at all.
    assert!(
        run_mcp_probe(
            root,
            CONNECTION,
            "list_transcripts",
            &json!({"day": OLD_DAY})
        )
        .is_err()
    );
}

#[test]
fn a_python_era_import_says_month_and_goes_once_the_day_is_touched() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    with_old_python(root);
    let list = call(root, "list_transcripts", json!({"day": "20260311"}));
    let note = list["earlier_imports_note"].as_str().expect("the note");
    assert!(note.contains("month of March 2026"), "{note}");

    // A new-path import that covers the day retires it, even with no rows there.
    let import = root.join("imports/20260401_000000");
    fs::create_dir_all(&import).unwrap();
    fs::write(
        import.join("manifest.json"),
        json!({"source_type": "apple_health", "span": [["20260301", "20260331"]]}).to_string(),
    )
    .unwrap();
    fs::write(import.join("body-touched.jsonl"), "").unwrap();
    let list = call(root, "list_transcripts", json!({"day": "20260311"}));
    assert!(list.get("earlier_imports_note").is_none(), "{list}");
}

#[test]
fn a_named_source_gets_the_note_in_gaps_note_before_nothing_readable() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    with_old_native(root);
    let list = call(
        root,
        "list_transcripts",
        json!({"day": OLD_DAY, "source": LABEL}),
    );
    assert_eq!(list["transcripts"], json!([]));
    assert!(
        list["gaps_note"]
            .as_str()
            .unwrap_or_default()
            .contains("Apple Health"),
        "{list}"
    );

    grant(
        root,
        ReadScope::Facets {
            ids: vec![HEALTH.to_owned()],
        },
    );
    let list = call(
        root,
        "list_transcripts",
        json!({"day": OLD_DAY, "source": LABEL}),
    );
    assert!(list.get("gaps_note").is_none(), "{list}");
}

const NEXT: &str = "20261011";

/// The owner's hidden body days, as the store holds them.
fn hide_days(root: &Path, days: &[&str]) {
    use solstone_core_format::content::body_hidden_days::{hidden_days_bytes, hidden_days_path};
    let path = hidden_days_path(root);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let days = days.iter().map(|day| (*day).to_owned()).collect();
    fs::write(path, hidden_days_bytes(&days)).unwrap();
}

/// A store of hidden days that can't be read.
fn damage_hidden_days(root: &Path) {
    let path = solstone_core_format::content::body_hidden_days::hidden_days_path(root);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "{ damaged").unwrap();
}

fn probe_fails(root: &Path, tool: &str, arguments: Value) -> bool {
    run_mcp_probe(root, CONNECTION, tool, &arguments).is_err()
}

fn night_piece(kind: RowKind, id: &str, index: Option<u64>) -> BodyRow {
    let mut row = BodyRow::new(
        kind,
        id.into(),
        "apple_health",
        "HKCategoryTypeIdentifierSleepAnalysis",
        "sleep".into(),
    );
    row.experience = Some("sleep".into());
    row.by = Some("Synthetic Watch".into());
    row.start = Some("2026-10-10 23:10:00 -0600".into());
    row.end = Some("2026-10-11 06:30:00 -0600".into());
    if kind != RowKind::Experience {
        row.of = Some("night".into());
    }
    row.index = index;
    row
}

/// A night that starts late on DAY and runs into NEXT, beside each day's own
/// readings, and a Strava workout on each side of midnight.
fn a_night_across_midnight(root: &Path) {
    fix_zone(root, "America/Denver");
    fs::write(
        root.join("imports/.strava-zone.json"),
        json!({"zone": "America/Denver"}).to_string(),
    )
    .unwrap();
    window(
        root,
        DAY,
        "060000_3600",
        &many_readings("d", 6, 31, 3),
        None,
    );
    window(
        root,
        DAY,
        "230000_3600",
        &[night_piece(RowKind::Experience, "night", None)],
        None,
    );
    window(
        root,
        NEXT,
        "000000_3600",
        &[
            night_piece(RowKind::ExperienceTile, "night:1", Some(1)),
            reading("early", "2026-10-11 00:20:00 -0600", "58"),
        ],
        None,
    );
    let mut stage = night_piece(RowKind::Stage, "s1", None);
    stage.start = Some("2026-10-11 01:30:00 -0600".into());
    stage.end = Some("2026-10-11 01:50:00 -0600".into());
    window(
        root,
        NEXT,
        "010000_3600",
        &[
            night_piece(RowKind::ExperienceTile, "night:2", Some(2)),
            stage,
        ],
        None,
    );
    window(
        root,
        NEXT,
        "070000_3600",
        &[reading("own", "2026-10-11 07:00:00 -0600", "61")],
        None,
    );
    strava_tile(root, "070000_300", &[]);
    // The second piece of a run that started on DAY, on NEXT.
    let dir = root
        .join("chronicle")
        .join(NEXT)
        .join("import.strava/000000_300");
    fs::create_dir_all(dir.join("talents")).unwrap();
    fs::write(
        dir.join("workout.json"),
        json!({
            "schema": "solstone.import.strava.tile.v1",
            "activity_id": 7,
            "tile": {"index": 1, "count": 2},
            "workout": {"name": "Late Run", "type": "Run",
                "start": "2026-10-10T23:55:00-06:00", "elapsed_seconds": 1200}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(dir.join("stream.json"), "{}").unwrap();
    fs::write(dir.join("talents/sense.json"), "{}").unwrap();
}

fn source_list(root: &Path, day: &str) -> Value {
    call(
        root,
        "list_transcripts",
        json!({"day": day, "source": LABEL}),
    )
}

#[test]
fn a_hidden_day_lists_and_reads_no_body_window_and_its_night_leaves_the_next_morning() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    a_night_across_midnight(root);

    let listed = source_list(root, DAY);
    assert_eq!(
        listed["transcripts"].as_array().unwrap().len(),
        2,
        "{listed}"
    );
    let on_day = listed["transcripts"][0]["reference"].clone();
    assert!(!probe_fails(
        root,
        "get_transcript",
        json!({"reference": on_day})
    ));
    assert_eq!(
        source_list(root, NEXT)["transcripts"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let day = call(root, "list_transcripts", json!({"day": DAY}));
    assert_eq!(day["other_sources"], json!([LABEL]), "{day}");
    assert_eq!(day["omitted_workouts"], json!(1), "{day}");
    let next = call(root, "list_transcripts", json!({"day": NEXT}));
    assert_eq!(next["omitted_workouts"], json!(1), "{next}");

    hide_days(root, &[DAY]);
    // The hidden day: no window, no span to read, and a window named before
    // the hide reads as not found.
    let listed = source_list(root, DAY);
    assert!(
        listed["transcripts"].as_array().unwrap().is_empty(),
        "{listed}"
    );
    assert!(listed.get("span_reference").is_none(), "{listed}");
    assert!(probe_fails(
        root,
        "get_transcript",
        json!({"reference": on_day})
    ));
    // A stray file beside a window's records doesn't open it.
    for stream in ["import.apple_health", "import.strava"] {
        for window in fs::read_dir(root.join("chronicle").join(DAY).join(stream))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir())
        {
            fs::write(window.path().join("copy (1).txt"), "a sync client's copy").unwrap();
        }
    }
    assert!(probe_fails(
        root,
        "get_transcript",
        json!({"reference": on_day})
    ));
    assert!(
        source_list(root, DAY)["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for stream in ["import.apple_health", "import.strava"] {
        for window in fs::read_dir(root.join("chronicle").join(DAY).join(stream))
            .into_iter()
            .flatten()
            .flatten()
        {
            let _ = fs::remove_file(window.path().join("copy (1).txt"));
        }
    }
    // Counts leave it out: its source and its workout.
    let day = call(root, "list_transcripts", json!({"day": DAY}));
    assert!(day.get("other_sources").is_none(), "{day}");
    assert!(day.get("omitted_workouts").is_none(), "{day}");
    // The next day keeps its own rows and source; the night's part after
    // midnight and the run's second piece go with the day they started.
    let next = call(root, "list_transcripts", json!({"day": NEXT}));
    assert_eq!(next["other_sources"], json!([LABEL]), "{next}");
    assert!(next.get("omitted_workouts").is_none(), "{next}");
    let listed = source_list(root, NEXT);
    let windows = listed["transcripts"].as_array().unwrap();
    assert_eq!(
        windows
            .iter()
            .map(|w| w["time"].clone())
            .collect::<Vec<_>>(),
        [json!("00:00-01:00"), json!("07:00-08:00")],
        "a window holding only the night's rows isn't listed: {listed}"
    );
    let (entries, _) = every_entry(root, listed["span_reference"].as_str().unwrap());
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(
        entries.iter().all(|entry| !entry.contains("sleep")),
        "{entries:?}"
    );
    let window = call(
        root,
        "get_transcript",
        json!({"reference": windows[0]["reference"]}),
    );
    let entries = window["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{window}");
    assert!(entries[0].as_str().unwrap().contains("58"), "{window}");

    // Hiding the next day instead leaves the night on the day it started.
    hide_days(root, &[NEXT]);
    assert!(
        source_list(root, NEXT)["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let listed = source_list(root, DAY);
    let (entries, _) = every_entry(root, listed["span_reference"].as_str().unwrap());
    assert!(
        entries.iter().any(|entry| entry.contains("sleep")),
        "{entries:?}"
    );

    // While which days are hidden can't be read, no body window is.
    hide_days(root, &[]);
    let next_window = source_list(root, NEXT)["transcripts"][0]["reference"].clone();
    damage_hidden_days(root);
    for day in [DAY, NEXT] {
        let listed = source_list(root, day);
        assert!(
            listed["transcripts"].as_array().unwrap().is_empty(),
            "{listed}"
        );
        let plain = call(root, "list_transcripts", json!({"day": day}));
        assert!(plain.get("other_sources").is_none(), "{plain}");
        assert!(plain.get("omitted_workouts").is_none(), "{plain}");
    }
    assert!(probe_fails(
        root,
        "get_transcript",
        json!({"reference": next_window})
    ));
}

fn run_row(id: &str, by: &str, start: &str, end: &str) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Experience,
        id.into(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "running".into(),
    );
    row.experience = Some("workout".into());
    row.start = Some(start.into());
    row.end = Some(end.into());
    row.by = Some(by.into());
    row
}

fn run_record(root: &Path, day: &str, row: &BodyRow) {
    use solstone_core_format::content::body_records::recorder_id;
    let path = root.join(format!("facets/health/activities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let record = json!({
        "id": format!("body_{}", row.id), "source": "body", "activity": "workout",
        "account": {"key": format!("{STREAM}\u{1f}{}", row.id), "start": row.start,
            "end": row.end, "type": "running", "recorder": recorder_id(STREAM, row.by.as_deref())},
    });
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{record}\n"));
    fs::write(path, text).unwrap();
}

fn permit(root: &Path, permission: ReadPermission) {
    PermissionStore::open(root)
        .set_permission(CONNECTION, permission)
        .unwrap();
}

fn body_activities(root: &Path) -> Vec<Value> {
    call(root, "list_activities", json!({"from": DAY, "to": NEXT}))["activities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|activity| activity["source"] == "body")
        .cloned()
        .collect()
}

fn week_names_the_run(root: &Path) -> bool {
    call(root, "get_week", json!({"week": "20261004"}))
        .to_string()
        .contains("Synthetic Watch")
}

#[test]
fn a_group_whose_day_is_hidden_is_found_by_no_group_reader() {
    let journal = journal();
    let root = journal.path();
    fix_zone(root, "America/Denver");
    // One run from two recorders across midnight, anchored on DAY. The
    // watch's window is filed under work, so a connection granted health
    // reads only the ring's, on NEXT.
    let watch = run_row(
        "w",
        "Synthetic Watch",
        "2026-10-10 23:55:00 -0600",
        "2026-10-11 06:55:00 -0600",
    );
    let ring = run_row(
        "r",
        "Synthetic Ring",
        "2026-10-11 00:05:00 -0600",
        "2026-10-11 07:00:00 -0600",
    );
    window(
        root,
        DAY,
        "235500_300",
        std::slice::from_ref(&watch),
        Some("work"),
    );
    window(
        root,
        NEXT,
        "000500_300",
        std::slice::from_ref(&ring),
        Some("health"),
    );
    run_record(root, DAY, &watch);
    run_record(root, NEXT, &ring);
    let health_only = ReadPermission {
        categories: vec![
            "facets".to_owned(),
            "transcripts".to_owned(),
            "body".to_owned(),
        ],
        scope: ReadScope::Facets {
            ids: vec![HEALTH.to_owned()],
        },
    };

    // The whole journal: one group, under DAY.
    permit(root, ReadPermission::default_whole_journal());
    let listed = body_activities(root);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let reference = json!({"reference": listed[0]["reference"]});
    assert!(!probe_fails(root, "fetch", reference.clone()));
    assert!(!probe_fails(
        root,
        "get_activity_sources",
        reference.clone()
    ));
    assert!(week_names_the_run(root));
    hide_days(root, &[DAY]);
    assert!(body_activities(root).is_empty());
    assert!(probe_fails(root, "fetch", reference.clone()));
    assert!(probe_fails(root, "get_activity_sources", reference.clone()));
    assert!(!week_names_the_run(root));

    // A connection that may read only the ring's window still finds nothing:
    // the anchor is decided on the owner's whole group.
    hide_days(root, &[]);
    permit(root, health_only.clone());
    let listed = body_activities(root);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let reference = json!({"reference": listed[0]["reference"]});
    assert!(!probe_fails(
        root,
        "get_activity_sources",
        reference.clone()
    ));
    hide_days(root, &[DAY]);
    assert!(body_activities(root).is_empty());
    assert!(probe_fails(root, "fetch", reference.clone()));
    assert!(probe_fails(root, "get_activity_sources", reference.clone()));
    // With NEXT hidden, its window isn't read, so no face lands under it.
    hide_days(root, &[NEXT]);
    assert!(body_activities(root).is_empty());
    assert!(probe_fails(root, "fetch", reference.clone()));
    assert!(probe_fails(root, "get_activity_sources", reference));

    // While which days are hidden can't be read, no connection sees the group.
    hide_days(root, &[]);
    permit(root, ReadPermission::default_whole_journal());
    let listed = body_activities(root);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let reference = json!({"reference": listed[0]["reference"]});
    damage_hidden_days(root);
    assert!(body_activities(root).is_empty());
    assert!(probe_fails(root, "fetch", reference.clone()));
    assert!(probe_fails(root, "get_activity_sources", reference));
    assert!(!week_names_the_run(root));
    // The owner's own view still lists it.
    let owner = solstone_core_location::body_records::body_faces_range(
        root, "health", DAY, NEXT, None, false,
    )
    .unwrap();
    assert_eq!(owner.values().flatten().count(), 1);
}

#[test]
fn a_strava_line_on_a_hidden_day_is_listed_by_no_connection() {
    let journal = self::journal();
    let root = journal.path();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "body".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let line = json!({"id": "exercise_1", "activity": "exercise", "title": "Morning Run",
        "stream": "import.strava", "segments": ["070000_300"], "created_at": 1});
    let other = json!({"id": "meeting_1", "activity": "meeting", "title": "Standup",
        "segments": ["090000_300"], "created_at": 2});
    let path = root.join(format!("facets/health/activities/{DAY}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("{line}\n{other}\n")).unwrap();
    let titles = || {
        let mut titles: Vec<String> =
            call(root, "list_activities", json!({"day": DAY}))["activities"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|row| row["title"].as_str().map(str::to_owned))
                .collect();
        titles.sort();
        titles
    };
    assert_eq!(titles(), ["Morning Run", "Standup"]);
    // Found by search and read while the day shows.
    index(root);
    let found = call(root, "search", json!({"query": "Morning Run", "day": DAY}));
    let reference = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result.to_string().contains("Morning Run"))
        .unwrap_or_else(|| panic!("{found}"))["reference"]
        .clone();
    let read = call(root, "fetch", json!({"reference": reference}));
    assert!(read.to_string().contains("Morning Run"), "{read}");
    hide_days(root, &[DAY]);
    assert_eq!(titles(), ["Standup"], "the hidden day's workout line goes");
    // A reference search gave out before the day was hidden reads nothing now.
    assert!(
        run_mcp_probe(root, CONNECTION, "fetch", &json!({"reference": reference})).is_err(),
        "a hidden day's workout line was fetched"
    );
    damage_hidden_days(root);
    assert_eq!(
        titles(),
        ["Standup"],
        "and every one while the days can't be read"
    );
}

const BODY_WORDS: &str = crate::body_source::BODY_NOT_GRANTED;

fn workout_day(root: &Path) {
    let mut rows = many_readings("a", 6, 31, 3);
    rows.push(workout());
    window(root, DAY, "063000_300", &rows, None);
    fs::write(
        root.join("chronicle").join(DAY).join("body.md"),
        "# body on Saturday, October 10, 2026\n\n## Apple Health · imported\n\n- workout: running\n",
    )
    .unwrap();
    index(root);
}

fn found_douro(root: &Path, arguments: Value) -> (bool, Value) {
    let found = call(root, "search", arguments);
    let any = found["results"].as_array().unwrap().iter().any(|result| {
        result["snippet"]
            .as_str()
            .unwrap_or_default()
            .contains("Douro")
    });
    (any, found)
}

#[test]
fn a_connection_approved_before_body_reads_none_of_it_and_is_told_why() {
    let journal = journal();
    let root = journal.path();
    // What every connection approved before body existed holds.
    grant_kinds(
        root,
        &["transcripts", "entities", "facets", "screen_text"],
        ReadScope::WholeJournal,
    );
    // Indexed under the old classification: body files are stored as transcripts.
    workout_day(root);

    let list = call(root, "list_transcripts", json!({"day": DAY}));
    assert!(list.get("other_sources").is_none(), "{list}");
    assert_eq!(list["body_note"], BODY_WORDS, "{list}");
    assert_eq!(empty_note(root, json!({"day": DAY})), BODY_WORDS);

    let source = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert!(
        source["transcripts"].as_array().unwrap().is_empty(),
        "{source}"
    );
    assert_eq!(source["body_note"], BODY_WORDS, "{source}");

    for arguments in [
        json!({"query": "Douro Loop", "day": DAY}),
        json!({"query": "Douro Loop", "day": DAY, "category": "transcripts"}),
        json!({"query": "running", "day": DAY}),
    ] {
        let (any, found) = found_douro(root, arguments.clone());
        assert!(!any, "{arguments}: {found}");
        assert_eq!(found["body_note"], BODY_WORDS, "{arguments}: {found}");
    }
    // A search that would have found no body data says nothing of it.
    let unrelated = call(root, "search", json!({"query": "kestrel", "day": DAY}));
    assert!(unrelated.get("body_note").is_none(), "{unrelated}");
    // Asking for body by name is refused, as any kind not granted is.
    assert!(
        run_mcp_probe(
            root,
            CONNECTION,
            "search",
            &json!({"query": "Douro", "category": "body"})
        )
        .is_err()
    );
}

#[test]
fn body_alone_reaches_body_data_and_one_tick_grants_it() {
    let journal = journal();
    let root = journal.path();
    grant_kinds(root, &["transcripts", "facets"], ReadScope::WholeJournal);
    workout_day(root);
    assert!(!found_douro(root, json!({"query": "Douro Loop", "day": DAY})).0);

    // The owner ticks body: the same index now answers.
    grant_kinds(
        root,
        &["transcripts", "facets", "body"],
        ReadScope::WholeJournal,
    );
    let (any, found) = found_douro(root, json!({"query": "Douro Loop", "day": DAY}));
    assert!(any, "{found}");
    assert!(found.get("body_note").is_none(), "{found}");

    // Body without transcripts still reaches every body route the tools have.
    grant_kinds(root, &["body"], ReadScope::WholeJournal);
    let list = call(root, "list_transcripts", json!({"day": DAY}));
    assert_eq!(list["other_sources"], json!([LABEL]), "{list}");
    let source = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    assert!(
        !source["transcripts"].as_array().unwrap().is_empty(),
        "{source}"
    );
    for category in [None, Some("body")] {
        let mut arguments = json!({"query": "Douro Loop", "day": DAY});
        if let Some(category) = category {
            arguments["category"] = json!(category);
        }
        let (any, found) = found_douro(root, arguments);
        assert!(any, "{category:?}: {found}");
    }
}

#[test]
fn an_old_strava_line_needs_body_by_every_tool() {
    let journal = journal();
    let root = journal.path();
    let line = json!({"id": "exercise_1", "activity": "exercise", "title": "Morning Run",
        "description": "kestrel ridge loop",
        "stream": "import.strava", "segments": ["070000_300"], "created_at": 1});
    let other = json!({"id": "meeting_1", "activity": "meeting", "title": "Standup",
        "segments": ["090000_300"], "created_at": 2});
    let path = root.join(format!("facets/health/activities/{DAY}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("{line}\n{other}\n")).unwrap();
    index(root);
    let titles = |value: &Value, key: &str| {
        let mut titles: Vec<String> = value[key]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["title"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        titles.sort();
        titles
    };
    let kestrel = |root: &Path| {
        let found = call(root, "search", json!({"query": "kestrel"}));
        let any = !found["results"].as_array().unwrap().is_empty();
        (any, found)
    };

    // Facets alone, as a connection approved before body held: not listed,
    // shown in the week or found, and each answer says why.
    grant_kinds(root, &["transcripts", "facets"], ReadScope::WholeJournal);
    let listed = call(root, "list_activities", json!({"day": DAY}));
    assert_eq!(titles(&listed, "activities"), ["Standup"], "{listed}");
    assert_eq!(listed["body_note"], BODY_WORDS, "{listed}");
    let week = call(root, "get_week", json!({"week": "20261004"}));
    assert!(!week.to_string().contains("Morning Run"), "{week}");
    assert_eq!(week["body_note"], BODY_WORDS, "{week}");
    let (any, found) = kestrel(root);
    assert!(!any, "{found}");
    assert_eq!(found["body_note"], BODY_WORDS, "{found}");

    // With body it reads as before.
    grant_kinds(
        root,
        &["transcripts", "facets", "body"],
        ReadScope::WholeJournal,
    );
    let listed = call(root, "list_activities", json!({"day": DAY}));
    assert_eq!(
        titles(&listed, "activities"),
        ["Morning Run", "Standup"],
        "{listed}"
    );
    assert!(listed.get("body_note").is_none(), "{listed}");
    let (any, found) = kestrel(root);
    assert!(any, "{found}");
}

fn contains_forbidden_summary_key(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let lower = key.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "avg"
                        | "average"
                        | "mean"
                        | "median"
                        | "baseline"
                        | "delta"
                        | "trend"
                        | "score"
                        | "readiness_score"
                        | "sleep_score"
                ) || lower.contains("average")
                    || lower.contains("median")
                    || lower.contains("baseline")
                    || lower.contains("trend")
                {
                    return Some(key.clone());
                }
                if let Some(found) = contains_forbidden_summary_key(child) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(contains_forbidden_summary_key),
        _ => None,
    }
}

#[test]
fn get_week_body_surfaces_five_held_measures_across_sources_and_keeps_awake_separate() {
    let journal = oura_journal();
    let root = journal.path();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        r#"{"zone":"UTC"}"#,
    )
    .unwrap();
    grant_kinds(
        root,
        &["transcripts", "facets", "body"],
        ReadScope::WholeJournal,
    );

    // Day 1 (2025-05-04, Sunday): rich Apple Health day (Watch + Phone) + Oura equivalents +
    // non-bounded readings (blood glucose, oxygen saturation) that stay out of get_week.
    let mut night_ah = BodyRow::new(
        RowKind::Experience,
        "sleep-ah-1".into(),
        "apple_health",
        "HKCategoryTypeIdentifierSleepAnalysis",
        "night".into(),
    );
    night_ah.experience = Some("sleep".into());
    night_ah.by = Some("Watch".into());
    night_ah.start = Some("2025-05-03T21:40:00+00:00".into());
    night_ah.end = Some("2025-05-04T06:45:00+00:00".into());

    let mk_stage = |id: &str, record_type: &str, label: &str, start: &str, end: &str| {
        let mut row = BodyRow::new(
            RowKind::Stage,
            id.into(),
            "apple_health",
            record_type,
            label.into(),
        );
        row.by = Some("Watch".into());
        row.of = Some("sleep-ah-1".into());
        row.start = Some(start.into());
        row.end = Some(end.into());
        row
    };
    let st_deep = mk_stage(
        "st-deep",
        "HKCategoryValueSleepAnalysisAsleepDeep",
        "asleep deep",
        "2025-05-03T21:40:00+00:00",
        "2025-05-03T23:18:00+00:00",
    );
    let st_core = mk_stage(
        "st-core",
        "HKCategoryValueSleepAnalysisAsleepCore",
        "asleep core",
        "2025-05-03T23:18:00+00:00",
        "2025-05-04T04:25:00+00:00",
    );
    let st_rem = mk_stage(
        "st-rem",
        "HKCategoryValueSleepAnalysisAsleepREM",
        "asleep rem",
        "2025-05-04T04:25:00+00:00",
        "2025-05-04T06:25:00+00:00",
    );
    let st_awake = mk_stage(
        "st-awake",
        "HKCategoryValueSleepAnalysisAwake",
        "awake",
        "2025-05-04T06:25:00+00:00",
        "2025-05-04T06:45:00+00:00",
    );

    let mut watch_energy = BodyRow::new(
        RowKind::Count,
        "energy-1".into(),
        "apple_health",
        "HKQuantityTypeIdentifierActiveEnergyBurned",
        "active energy burned".into(),
    );
    watch_energy.by = Some("Watch".into());
    watch_energy.start = Some("2025-05-04T00:00:00+00:00".into());
    watch_energy.end = Some("2025-05-05T00:00:00+00:00".into());
    watch_energy.unit = Some("kcal".into());
    watch_energy.value = Some("158".into());

    let mut phone_steps = BodyRow::new(
        RowKind::Count,
        "steps-phone".into(),
        "apple_health",
        "HKQuantityTypeIdentifierStepCount",
        "step count".into(),
    );
    phone_steps.by = Some("Phone".into());
    phone_steps.start = Some("2025-05-04T07:00:00+00:00".into());
    phone_steps.end = Some("2025-05-04T08:00:00+00:00".into());
    phone_steps.unit = Some("count".into());
    phone_steps.value = Some("900".into());

    let mk_reading = |id: &str, rtype: &str, label: &str, at: &str, val: &str, unit: &str| {
        let mut row = BodyRow::new(
            RowKind::Reading,
            id.into(),
            "apple_health",
            rtype,
            label.into(),
        );
        row.by = Some("Watch".into());
        row.at = Some(at.into());
        row.value = Some(val.into());
        row.unit = Some(unit.into());
        row
    };
    let rhr_1 = mk_reading(
        "rhr-1",
        "HKQuantityTypeIdentifierRestingHeartRate",
        "resting heart rate",
        "2025-05-04T06:45:00+00:00",
        "76",
        "count/min",
    );
    let rhr_2 = mk_reading(
        "rhr-2",
        "HKQuantityTypeIdentifierRestingHeartRate",
        "resting heart rate",
        "2025-05-04T18:45:00+00:00",
        "72",
        "count/min",
    );
    let hrv_1 = mk_reading(
        "hrv-1",
        "HKQuantityTypeIdentifierHeartRateVariabilitySDNN",
        "heart rate variability sdnn",
        "2025-05-04T06:45:00+00:00",
        "57",
        "ms",
    );
    let hrv_2 = mk_reading(
        "hrv-2",
        "HKQuantityTypeIdentifierHeartRateVariabilitySDNN",
        "heart rate variability sdnn",
        "2025-05-04T12:15:00+00:00",
        "63.5",
        "ms",
    );
    let hr_1 = mk_reading(
        "hr-1",
        "HKQuantityTypeIdentifierHeartRate",
        "heart rate",
        "2025-05-04T01:00:00+00:00",
        "71",
        "count/min",
    );
    let hr_2 = mk_reading(
        "hr-2",
        "HKQuantityTypeIdentifierHeartRate",
        "heart rate",
        "2025-05-04T09:30:00+00:00",
        "88",
        "count/min",
    );
    let hr_3 = mk_reading(
        "hr-3",
        "HKQuantityTypeIdentifierHeartRate",
        "heart rate",
        "2025-05-04T14:00:00+00:00",
        "71",
        "count/min",
    );
    // Non-bounded reading stays out of get_week while appearing in body.md.
    let glucose = mk_reading(
        "bg-1",
        "HKQuantityTypeIdentifierBloodGlucose",
        "blood glucose",
        "2025-05-04T08:00:00+00:00",
        "94",
        "mg/dL",
    );

    window(
        root,
        "20250503",
        "210000_3600",
        &[night_ah, st_deep, st_core],
        None,
    );
    window(
        root,
        "20250504",
        "060000_300",
        &[
            st_rem,
            st_awake,
            watch_energy,
            phone_steps,
            rhr_1,
            rhr_2,
            hrv_1,
            hrv_2,
            hr_1,
            hr_2,
            hr_3,
            glucose,
        ],
        None,
    );

    // Oura equivalents on the same day: active_calories, sleep stage fields, heartrate readings.
    let mut oura_activity = oura_row(
        RowKind::Dated,
        "oura-act-1",
        "daily_activity",
        "daily activity",
        json!({"id": "oura-act-1", "day": "2025-05-04", "steps": 6200, "active_calories": 412, "score": 88}),
    );
    oura_activity.date = Some("2025-05-04".into());
    let mut oura_sleep = oura_row(
        RowKind::Experience,
        "oura-sleep-1",
        "sleep",
        "night",
        json!({
            "id": "oura-sleep-1",
            "day": "2025-05-04",
            "total_sleep_duration": 26340,
            "deep_sleep_duration": 5460,
            "light_sleep_duration": 14400,
            "rem_sleep_duration": 6480,
            "awake_time": 1920,
            "lowest_heart_rate": 47,
            "average_hrv": 62
        }),
    );
    oura_sleep.experience = Some("sleep".into());
    oura_sleep.start = Some("2025-05-03T22:41:00-06:00".into());
    oura_sleep.end = Some("2025-05-04T06:32:00-06:00".into());
    window_in(root, OURA, "20250503", "220000_3600", &[oura_sleep], None);
    window_in(
        root,
        OURA,
        "20250504",
        "000000_86400",
        &[oura_activity],
        None,
    );
    window_in(
        root,
        OURA,
        "20250504",
        "080000_3600",
        &[
            oura_heart_rate("ohr-1", "2025-05-04T14:05:00Z", "58", "rest"),
            oura_heart_rate("ohr-2", "2025-05-04T14:25:00Z", "92", "awake"),
        ],
        None,
    );

    // Day 2 (2025-05-05): sparse day with steps-only, a bare sleep without stages,
    // and an awake-only sleep to prove awake never produces asleep_seconds = 0.
    let mut sparse_steps = BodyRow::new(
        RowKind::Count,
        "steps-day2".into(),
        "apple_health",
        "HKQuantityTypeIdentifierStepCount",
        "step count".into(),
    );
    sparse_steps.by = Some("Phone".into());
    sparse_steps.start = Some("2025-05-05T10:00:00+00:00".into());
    sparse_steps.end = Some("2025-05-05T11:00:00+00:00".into());
    sparse_steps.unit = Some("count".into());
    sparse_steps.value = Some("1200".into());
    let mut bare_nap = BodyRow::new(
        RowKind::Experience,
        "nap-bare".into(),
        "apple_health",
        "HKCategoryTypeIdentifierSleepAnalysis",
        "nap".into(),
    );
    bare_nap.experience = Some("sleep".into());
    bare_nap.start = Some("2025-05-05T13:00:00+00:00".into());
    bare_nap.end = Some("2025-05-05T13:30:00+00:00".into());
    let mut awake_only_night = BodyRow::new(
        RowKind::Experience,
        "night-awake-only".into(),
        "apple_health",
        "HKCategoryTypeIdentifierSleepAnalysis",
        "night".into(),
    );
    awake_only_night.experience = Some("sleep".into());
    awake_only_night.start = Some("2025-05-05T04:00:00+00:00".into());
    awake_only_night.end = Some("2025-05-05T04:30:00+00:00".into());
    let mut awake_only_stage = BodyRow::new(
        RowKind::Stage,
        "st-awake-only".into(),
        "apple_health",
        "HKCategoryValueSleepAnalysisAwake",
        "awake".into(),
    );
    awake_only_stage.of = Some("night-awake-only".into());
    awake_only_stage.start = Some("2025-05-05T04:00:00+00:00".into());
    awake_only_stage.end = Some("2025-05-05T04:30:00+00:00".into());
    window(
        root,
        "20250505",
        "040000_3600",
        &[sparse_steps, bare_nap, awake_only_night, awake_only_stage],
        None,
    );

    let week = call(
        root,
        "get_week",
        json!({"week": "20250504", "include": ["body"]}),
    );
    let range = call(
        root,
        "get_week",
        json!({"day_from": "20250504", "day_to": "20250505", "include": ["body"]}),
    );
    assert_eq!(contains_forbidden_summary_key(&week), None, "{week}");
    assert_eq!(contains_forbidden_summary_key(&range), None, "{range}");

    // Single-week and range mode return identical body summaries for both days.
    let d1_week = &week["days"][0]["summary"]["body"];
    let d1_range = &range["days"][0]["summary"]["body"];
    assert_eq!(d1_week, d1_range);
    let d2_week = &week["days"][1]["summary"]["body"];
    let d2_range = &range["days"][1]["summary"]["body"];
    assert_eq!(d2_week, d2_range);

    // Day 1 movement: Phone steps (no active_energy_kcal), Watch active_energy_kcal, Oura steps + active_energy_kcal.
    let mov = d1_range["movement"].as_array().unwrap();
    assert_eq!(mov.len(), 3, "{mov:?}");
    let phone = mov.iter().find(|m| m["recorded_by"] == "Phone").unwrap();
    assert_eq!(phone["steps"], 900.0);
    assert!(phone.get("active_energy_kcal").is_none(), "{phone}");
    let watch = mov.iter().find(|m| m["recorded_by"] == "Watch").unwrap();
    assert_eq!(watch["active_energy_kcal"], 158.0);
    let oura_m = mov.iter().find(|m| m["source"] == OURA_LABEL).unwrap();
    assert_eq!(oura_m["steps"], 6200.0);
    assert_eq!(oura_m["active_energy_kcal"], 412.0);

    // Day 1 nights: Apple Health awake stays out of asleep_seconds (31500, not 32700);
    // Oura maps deep/light/rem/awake without relabeling light as core.
    let nights = d1_range["nights"].as_array().unwrap();
    assert_eq!(nights.len(), 2, "{nights:?}");
    let ah_night = nights.iter().find(|n| n["source"] == LABEL).unwrap();
    assert_eq!(ah_night["asleep_seconds"], 31_500);
    assert_eq!(
        ah_night["stages_seconds"],
        json!({"awake": 1200, "core": 18420, "deep": 5880, "rem": 7200})
    );
    let oura_night = nights.iter().find(|n| n["source"] == OURA_LABEL).unwrap();
    assert_eq!(oura_night["asleep_seconds"], 26_340);
    assert_eq!(
        oura_night["stages_seconds"],
        json!({"awake": 1920, "deep": 5460, "light": 14400, "rem": 6480})
    );

    // Day 1 readings: Apple Watch has RHR, HRV, HR (with earliest min_at tie-break);
    // Oura has HR only (no RHR/HRV from sleep lowest_heart_rate / average_hrv);
    // blood glucose stays excluded.
    let readings = d1_range["readings"].as_array().unwrap();
    assert_eq!(readings.len(), 2, "{readings:?}");
    let ah_r = readings.iter().find(|r| r["source"] == LABEL).unwrap();
    assert_eq!(
        ah_r["resting_heart_rate_bpm"],
        json!({"min": 72.0, "max": 76.0, "count": 2})
    );
    assert_eq!(
        ah_r["hrv_sdnn_ms"],
        json!({"min": 57.0, "max": 63.5, "count": 2})
    );
    assert_eq!(
        ah_r["heart_rate_bpm"],
        json!({
            "min": 71.0,
            "max": 88.0,
            "count": 3,
            "min_at": "2025-05-04T01:00:00+00:00",
            "max_at": "2025-05-04T09:30:00+00:00",
        })
    );
    let oura_r = readings.iter().find(|r| r["source"] == OURA_LABEL).unwrap();
    assert!(oura_r.get("resting_heart_rate_bpm").is_none(), "{oura_r}");
    assert!(oura_r.get("hrv_sdnn_ms").is_none(), "{oura_r}");
    assert_eq!(
        oura_r["heart_rate_bpm"],
        json!({
            "min": 58.0,
            "max": 92.0,
            "count": 2,
            "min_at": "2025-05-04T14:05:00Z",
            "max_at": "2025-05-04T14:25:00Z",
        })
    );
    assert!(!d1_range.to_string().contains("glucose"), "{d1_range}");

    // Day 2 sparse day: steps-only, no readings, bare nap omits stages_seconds,
    // awake-only night keeps asleep_seconds = null while reporting stages_seconds.awake = 1800.
    assert!(d2_range.get("readings").is_none(), "{d2_range}");
    let d2_nights = d2_range["nights"].as_array().unwrap();
    let nap = d2_nights
        .iter()
        .find(|n| n["to_bed"] == "2025-05-05T13:00:00+00:00")
        .unwrap();
    assert!(nap.get("stages_seconds").is_none(), "{nap}");
    let awake_only = d2_nights
        .iter()
        .find(|n| n["to_bed"] == "2025-05-05T04:00:00+00:00")
        .unwrap();
    assert!(awake_only["asleep_seconds"].is_null(), "{awake_only}");
    assert_eq!(awake_only["stages_seconds"], json!({"awake": 1800}));

    // BODY-denied connection sees zero body data on get_week(include=["body"]).
    grant_kinds(root, &["transcripts", "facets"], ReadScope::WholeJournal);
    let denied = call(
        root,
        "get_week",
        json!({"day_from": "20250504", "day_to": "20250505", "include": ["body"]}),
    );
    let denied_body = &denied["days"][0]["summary"]["body"];
    assert!(denied_body.get("movement").is_none(), "{denied}");
    assert!(denied_body.get("nights").is_none(), "{denied}");
    assert!(denied_body.get("readings").is_none(), "{denied}");
    assert!(denied_body["note"].is_string(), "{denied}");

    // B2 search + fetch: body.md holds values/bounds/durations while Oura score stays absent.
    grant_kinds(
        root,
        &["transcripts", "facets", "body"],
        ReadScope::WholeJournal,
    );
    solstone_core_location::body_day::BodyDays::default().reconcile(root);
    index(root);
    let found = call(
        root,
        "search",
        json!({"kind": ["body"], "day": "20250504", "query": "158"}),
    );
    let hit = found["results"]
        .as_array()
        .unwrap()
        .first()
        .expect("body.md search hit for held active energy");
    let fetched = call(root, "fetch", json!({"reference": hit["reference"]}));
    let fetched_text = fetched.to_string();
    assert!(fetched_text.contains("158 kcal"), "{fetched_text}");
    assert!(fetched_text.contains("72–76 bpm"), "{fetched_text}");
    assert!(fetched_text.contains("57–63.5 ms"), "{fetched_text}");
    assert!(fetched_text.contains("71–88 bpm"), "{fetched_text}");
    assert!(fetched_text.contains("94 mg/dL"), "{fetched_text}");
    assert!(fetched_text.contains("2 h"), "{fetched_text}");
    assert!(fetched_text.contains("20 min"), "{fetched_text}");
    assert!(!fetched_text.contains("score"), "{fetched_text}");
}

#[test]
fn get_week_body_range_paginates_cleanly_under_byte_limit_and_tools_list_stays_invariant() {
    let journal = journal();
    let root = journal.path();
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        r#"{"zone":"UTC"}"#,
    )
    .unwrap();
    grant_kinds(
        root,
        &[
            "transcripts",
            "facets",
            "body",
            "places",
            "photos",
            "notes",
            "entities",
        ],
        ReadScope::WholeJournal,
    );

    // Populate 6 days with full 5-measure body rows and force byte-budget pagination.
    for day_num in 1..=6 {
        let day = format!("2025050{day_num}");
        let mut energy = BodyRow::new(
            RowKind::Count,
            format!("e-{day}"),
            "apple_health",
            "HKQuantityTypeIdentifierActiveEnergyBurned",
            "active energy burned".into(),
        );
        energy.by = Some("Watch".into());
        energy.start = Some(format!("2025-05-0{day_num}T00:00:00+00:00"));
        energy.end = Some(format!("2025-05-0{}T00:00:00+00:00", day_num + 1));
        energy.unit = Some("kcal".into());
        energy.value = Some((200 + day_num * 10).to_string());
        let mut hr = BodyRow::new(
            RowKind::Reading,
            format!("hr-{day}"),
            "apple_health",
            "HKQuantityTypeIdentifierHeartRate",
            "heart rate".into(),
        );
        hr.by = Some("Watch".into());
        hr.at = Some(format!("2025-05-0{day_num}T12:00:00+00:00"));
        hr.unit = Some("count/min".into());
        hr.value = Some((70 + day_num).to_string());
        window(root, &day, "120000_300", &[energy, hr], None);
    }

    let decision = crate::permissions::evaluate_connection_read(root, CONNECTION);
    let crate::permissions::PermissionDecision::Snapshot(snapshot) = decision else {
        panic!("expected snapshot");
    };
    let principal = crate::dispatch::DispatchPrincipal {
        connection: CONNECTION,
        agent_identity: CONNECTION,
    };
    let include = [solstone_core_indexer_query::AdmittedCategory::Body]
        .into_iter()
        .collect();

    let mut seen_days = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let page = crate::tools::week::project_range_with_limits(
            root,
            "20250501",
            "20250506",
            cursor.as_deref(),
            principal,
            &snapshot,
            &include,
            100,
            600,
        )
        .unwrap();
        pages += 1;
        for row in page.value["days"].as_array().unwrap() {
            seen_days.push(row["day"].as_str().unwrap().to_owned());
            assert!(row["summary"]["body"]["movement"][0]["active_energy_kcal"].is_number());
            assert!(row["summary"]["body"]["readings"][0]["heart_rate_bpm"]["min"].is_number());
        }
        cursor = page.value["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert!(
        pages > 1,
        "expected multi-page split under tight byte budget"
    );
    assert_eq!(
        seen_days,
        vec![
            "20250501", "20250502", "20250503", "20250504", "20250505", "20250506"
        ]
    );

    // tools/list invariance: plain object root schemas (zero top-level combinators) and unchanged tool names.
    let tools = crate::registry::advertised_tools_list(
        root,
        &crate::permissions::PermissionDecision::Snapshot(snapshot),
    );
    let listed = tools["tools"].as_array().unwrap();
    let names: Vec<&str> = listed.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"get_week"), "{names:?}");
    for tool in listed {
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{tool}");
        for combinator in ["oneOf", "anyOf", "allOf"] {
            assert!(schema.get(combinator).is_none(), "{tool}");
        }
    }
}

fn stream_window(
    root: &Path,
    day: &str,
    stream: &str,
    name: &str,
    rows: &[BodyRow],
    facet: Option<&str>,
) {
    if stream == "import.garmin" && !root.join("streams/import.garmin.json").exists() {
        fs::create_dir_all(root.join("streams")).unwrap();
        fs::write(
            root.join("streams/import.garmin.json"),
            json!({
                "name": "import.garmin", "kind": "import:garmin", "host": null, "platform": null,
                "created_at": 1, "last_day": null, "last_segment": null, "seq": 1,
            })
            .to_string(),
        )
        .unwrap();
    }
    window_in(root, stream, day, name, rows, facet);
}

#[test]
fn womens_health_is_withheld_across_garmin_and_apple_health_until_explicitly_granted() {
    let journal = journal();
    let root = journal.path();
    fix_zone(root, "America/Denver");
    fs::write(
        root.join("imports/.garmin-zone.json"),
        json!({"zone": "America/Denver"}).to_string(),
    )
    .unwrap();

    // Garmin: mixed window (daily summary + menstrual cycle summary) + WH-only day on NEXT.
    let mut garmin_daily = BodyRow::new(
        RowKind::Dated,
        "g_daily".into(),
        "garmin",
        "daily_summary",
        "daily summary".into(),
    );
    garmin_daily.date = Some("2026-10-10".into());
    garmin_daily.by = Some("Garmin".into());
    garmin_daily.fields.insert("steps".into(), json!(8120));

    let mut garmin_wh = BodyRow::new(
        RowKind::Dated,
        "g_wh".into(),
        "garmin",
        "menstrual_cycle_summary",
        "menstrual cycle summary".into(),
    );
    garmin_wh.date = Some("2026-10-10".into());
    garmin_wh.by = Some("Garmin".into());
    garmin_wh
        .fields
        .insert("currentPhase".into(), json!("FOLLICULAR"));

    let mut garmin_wh_only = BodyRow::new(
        RowKind::Dated,
        "g_wh_only".into(),
        "garmin",
        "menstrual_cycle_dayview",
        "menstrual cycle day view".into(),
    );
    garmin_wh_only.date = Some("2026-10-11".into());
    garmin_wh_only.by = Some("Garmin".into());
    garmin_wh_only.fields.insert("flow".into(), json!("LIGHT"));

    stream_window(
        root,
        DAY,
        "import.garmin",
        "000000_86400",
        &[garmin_daily, garmin_wh],
        None,
    );
    stream_window(
        root,
        NEXT,
        "import.garmin",
        "000000_86400",
        &[garmin_wh_only],
        None,
    );

    // Apple Health: mixed window (heart rate + MenstrualFlow + BasalBodyTemperature).
    let ah_hr = reading("ah_hr", "2026-10-10 08:00:00 -0600", "64");
    let mut ah_flow = BodyRow::new(
        RowKind::Logged,
        "ah_flow".into(),
        "apple_health",
        "HKCategoryTypeIdentifierMenstrualFlow",
        "menstrual flow".into(),
    );
    ah_flow.start = Some("2026-10-10 08:05:00 -0600".into());
    ah_flow.end = Some("2026-10-10 08:05:00 -0600".into());
    ah_flow.by = Some("Apple Health".into());
    ah_flow.value = Some("medium".into());

    let mut ah_bbt = BodyRow::new(
        RowKind::Reading,
        "ah_bbt".into(),
        "apple_health",
        "HKQuantityTypeIdentifierBasalBodyTemperature",
        "basal body temperature".into(),
    );
    ah_bbt.start = Some("2026-10-10 08:10:00 -0600".into());
    ah_bbt.end = Some("2026-10-10 08:10:00 -0600".into());
    ah_bbt.by = Some("Apple Health".into());
    ah_bbt.value = Some("36.7".into());
    ah_bbt.unit = Some("degC".into());

    window(root, DAY, "080000_3600", &[ah_hr, ah_flow, ah_bbt], None);

    // Build owner's full body.md on disk (which legitimately holds WH entries for the owner)
    // and index it so we test MCP search/fetch filtering and query-token oracle prevention.
    let owner_body_md =
        solstone_core_location::body_day::render_day_document_for_agent(root, DAY, true)
            .expect("owner body.md");
    assert!(owner_body_md.contains("menstrual"), "{owner_body_md}");
    fs::write(
        root.join("chronicle").join(DAY).join("body.md"),
        &owner_body_md,
    )
    .unwrap();
    index(root);

    // 1. Connection with ONLY `body` (no `womens_health`):
    grant_kinds(root, &["body"], ReadScope::WholeJournal);

    // WH-only day (NEXT) does NOT leak presence in other_sources or source listing.
    let next_list = call(root, "list_transcripts", json!({"day": NEXT}));
    assert!(next_list.get("other_sources").is_none(), "{next_list}");
    let next_garmin = call(
        root,
        "list_transcripts",
        json!({"day": NEXT, "source": "Garmin · imported"}),
    );
    assert!(
        next_garmin["transcripts"].as_array().unwrap().is_empty(),
        "{next_garmin}"
    );

    // Garmin mixed window on DAY: lists daily summary, omits menstrual_cycle_summary.
    let garmin_day = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": "Garmin · imported"}),
    );
    let garmin_ref = garmin_day["transcripts"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let garmin_span = garmin_day["span_reference"].as_str().unwrap().to_owned();
    let garmin_seg = call(root, "get_transcript", json!({"reference": garmin_ref}));
    assert!(
        garmin_seg.to_string().contains("daily summary"),
        "{garmin_seg}"
    );
    assert!(
        !garmin_seg.to_string().contains("menstrual"),
        "{garmin_seg}"
    );
    let (span_entries, _) = every_entry(root, &garmin_span);
    assert!(
        span_entries.iter().any(|e| e.contains("daily summary")),
        "{span_entries:?}"
    );
    assert!(
        span_entries.iter().all(|e| !e.contains("menstrual")),
        "{span_entries:?}"
    );

    // Apple Health mixed window on DAY: keeps heart rate, strips menstrual flow & basal body temperature.
    let ah_day = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    let ah_ref = ah_day["transcripts"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let ah_span = ah_day["span_reference"].as_str().unwrap().to_owned();
    let ah_seg = call(root, "get_transcript", json!({"reference": ah_ref}));
    assert!(ah_seg.to_string().contains("64"), "{ah_seg}");
    assert!(!ah_seg.to_string().contains("menstrual"), "{ah_seg}");
    assert!(
        !ah_seg.to_string().contains("basal body temperature"),
        "{ah_seg}"
    );
    let (ah_entries, _) = every_entry(root, &ah_span);
    assert!(
        ah_entries.iter().any(|e| e.contains("64")),
        "{ah_entries:?}"
    );
    assert!(
        ah_entries
            .iter()
            .all(|e| !e.contains("menstrual") && !e.contains("basal body temperature")),
        "{ah_entries:?}"
    );

    // Search on body.md: querying "menstrual" or "basal" returns ZERO hits (no oracle!).
    for q in ["menstrual", "basal body temperature"] {
        let found = call(root, "search", json!({"query": q, "day": DAY}));
        assert!(
            found["results"].as_array().unwrap().is_empty(),
            "query {q:?} must not leak WH from body.md: {found}"
        );
    }
    // Searching a non-WH term still finds body.md, and fetching it serves filtered markdown without WH.
    let daily_found = call(
        root,
        "search",
        json!({"query": "daily summary", "day": DAY}),
    );
    let body_md_ref = daily_found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r.to_string().contains("body"))
        .unwrap_or_else(|| panic!("expected body.md result: {daily_found}"))["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let fetched_filtered = call(root, "fetch", json!({"reference": body_md_ref}));
    let filtered_text = fetched_filtered.to_string();
    assert!(filtered_text.contains("daily summary"), "{filtered_text}");
    assert!(!filtered_text.contains("menstrual"), "{filtered_text}");
    assert!(
        !filtered_text.contains("basal body temperature"),
        "{filtered_text}"
    );

    // 2. Grant `body` + `womens_health`: everything surfaces cleanly!
    grant_kinds(root, &["body", "womens_health"], ReadScope::WholeJournal);

    let next_list_wh = call(root, "list_transcripts", json!({"day": NEXT}));
    assert_eq!(
        next_list_wh["other_sources"],
        json!(["Garmin · imported"]),
        "{next_list_wh}"
    );

    let garmin_day_wh = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": "Garmin · imported"}),
    );
    let garmin_seg_wh = call(
        root,
        "get_transcript",
        json!({"reference": garmin_day_wh["transcripts"][0]["reference"]}),
    );
    assert!(
        garmin_seg_wh
            .to_string()
            .contains("menstrual cycle summary"),
        "{garmin_seg_wh}"
    );

    let ah_day_wh = call(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": LABEL}),
    );
    let ah_seg_wh = call(
        root,
        "get_transcript",
        json!({"reference": ah_day_wh["transcripts"][0]["reference"]}),
    );
    assert!(
        ah_seg_wh.to_string().contains("menstrual flow"),
        "{ah_seg_wh}"
    );
    assert!(
        ah_seg_wh.to_string().contains("basal body temperature"),
        "{ah_seg_wh}"
    );

    let wh_found = call(root, "search", json!({"query": "menstrual", "day": DAY}));
    assert!(
        !wh_found["results"].as_array().unwrap().is_empty(),
        "{wh_found}"
    );
    let wh_ref = wh_found["results"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let fetched_wh = call(root, "fetch", json!({"reference": wh_ref}));
    assert!(fetched_wh.to_string().contains("menstrual"), "{fetched_wh}");

    // 3. Revoking `womens_health` invalidates prior span/search references immediately.
    let prior_span = garmin_day_wh["span_reference"].as_str().unwrap().to_owned();
    grant_kinds(root, &["body"], ReadScope::WholeJournal);
    assert!(probe_fails(
        root,
        "get_transcript",
        json!({"reference": prior_span})
    ));
    assert!(probe_fails(root, "fetch", json!({"reference": wh_ref})));
}

#[test]
fn garmin_workout_gps_coordinates_follow_places_precision_in_transcript_and_span_reads() {
    let journal = journal();
    let root = journal.path();
    fix_zone(root, "America/Denver");
    fs::write(
        root.join("imports/.garmin-zone.json"),
        json!({"zone": "America/Denver"}).to_string(),
    )
    .unwrap();

    let mut run = BodyRow::new(
        RowKind::Experience,
        "g_run_1".into(),
        "garmin",
        "trail_running",
        "trail running".into(),
    );
    run.experience = Some("workout".into());
    run.by = Some("Garmin".into());
    run.start = Some("2026-10-10T07:00:00-06:00".into());
    run.end = Some("2026-10-10T07:04:00-06:00".into());
    run.words = vec!["Morning Ridge Run".into()];
    run.fields.insert("locationName".into(), json!("Boulder"));
    run.fields.insert("distance".into(), json!(5200.0));
    run.fields.insert("averageHR".into(), json!(154.0));
    run.fields.insert("startLatitude".into(), json!(40.01731));
    run.fields
        .insert("startLongitude".into(), json!(-105.27289));
    run.fields.insert("endLatitude".into(), json!(40.02418));
    run.fields.insert("endLongitude".into(), json!(-105.28341));

    stream_window(root, DAY, "import.garmin", "070000_300", &[run], None);

    let read_both = |root: &Path| -> (String, String) {
        let listed = call(
            root,
            "list_transcripts",
            json!({"day": DAY, "source": "Garmin · imported"}),
        );
        let seg_ref = listed["transcripts"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();
        let span_ref = listed["span_reference"].as_str().unwrap().to_owned();
        let seg_resp = call(root, "get_transcript", json!({"reference": seg_ref}));
        let seg_text = seg_resp["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap().replace('\\', ""))
            .collect::<Vec<_>>()
            .join("\n");
        let (entries, _) = every_entry(root, &span_ref);
        let span_text = entries.join("\n");
        (seg_text, span_text)
    };

    // 1. Body only (no places): no coords, no H3 cells; keeps locationName, title, stats.
    grant_kinds(root, &["body"], ReadScope::WholeJournal);
    let (seg, span) = read_both(root);
    for text in [&seg, &span] {
        assert!(text.contains("Morning Ridge Run"), "{text}");
        assert!(text.contains("Boulder"), "{text}");
        assert!(text.contains("5200"), "{text}");
        assert!(!text.contains("startLatitude"), "{text}");
        assert!(!text.contains("endLongitude"), "{text}");
        assert!(!text.contains("40.01731"), "{text}");
        assert!(!text.contains("startLocationCell"), "{text}");
    }

    // 2. Body + places (NamesOnly): same omission of coords while keeping locationName.
    grant_kinds(
        root,
        &["body", "places", "places_names"],
        ReadScope::WholeJournal,
    );
    let (seg, span) = read_both(root);
    for text in [&seg, &span] {
        assert!(text.contains("Boulder"), "{text}");
        assert!(!text.contains("startLatitude"), "{text}");
        assert!(!text.contains("startLocationCell"), "{text}");
    }

    // 3. Body + places (Approx): removes exact lat/lon, emits H3 res-8 startLocationCell & endLocationCell.
    grant_kinds(
        root,
        &["body", "places", "places_approx"],
        ReadScope::WholeJournal,
    );
    let (seg, span) = read_both(root);
    for text in [&seg, &span] {
        assert!(text.contains("Boulder"), "{text}");
        assert!(text.contains("startLocationCell"), "{text}");
        assert!(text.contains("endLocationCell"), "{text}");
        assert!(text.contains("h3"), "{text}");
        assert!(!text.contains("startLatitude"), "{text}");
        assert!(!text.contains("40.01731"), "{text}");
    }

    // 4. Body + places (Full): exact startLatitude/startLongitude/endLatitude/endLongitude preserved.
    grant_kinds(
        root,
        &["body", "places", "places_full"],
        ReadScope::WholeJournal,
    );
    let (seg, span) = read_both(root);
    for text in [&seg, &span] {
        assert!(text.contains("startLatitude"), "{text}");
        assert!(text.contains("40.01731"), "{text}");
        assert!(text.contains("-105.27289"), "{text}");
        assert!(text.contains("endLatitude"), "{text}");
        assert!(!text.contains("startLocationCell"), "{text}");
    }
}
