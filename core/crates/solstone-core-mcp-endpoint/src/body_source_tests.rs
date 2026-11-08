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

fn grant(journal: &Path, scope: ReadScope) {
    PermissionStore::open(journal)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
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
        chrono::Utc::now(),
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

    // Another stream's label reaches nothing, even with that stream on the day.
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
    assert!(other.get("gaps").is_none(), "{other}");
    assert!(other["transcripts"].as_array().unwrap().is_empty());

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
    assert!(nothing.starts_with("No body source"), "{nothing}");
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
    // Never a heart rate value or a field nobody chose to restate.
    for query in ["7003", "7001", "intensity", "confirmed"] {
        let found = call(root, "search", json!({"query": query, "day": DAY}));
        assert!(
            found["results"].as_array().unwrap().is_empty(),
            "{query}: {found}"
        );
    }

    // The hit's link reaches the workout with every field.
    let found = call(
        root,
        "search",
        json!({"query": "synthetic hill loop", "day": DAY}),
    );
    let fetched = call(
        root,
        "fetch",
        json!({"reference": found["results"][0]["reference"]}),
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
