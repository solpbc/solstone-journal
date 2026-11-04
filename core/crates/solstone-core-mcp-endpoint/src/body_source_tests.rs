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
    let dir = root.join("chronicle").join(day).join(STREAM).join(key);
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

    // 31 + 80 + 5 entries, whole window pages, at most 100 on one.
    assert_eq!(from_first.len(), 116);
    assert_eq!(pages, vec![31, 85]);
    let unique: std::collections::BTreeSet<_> = from_first.iter().collect();
    assert_eq!(unique.len(), from_first.len(), "each entry once");
    assert!(from_first[0].contains("Apple Health, Synthetic Watch"));
    assert!(
        from_first
            .iter()
            .all(|entry| entry.contains("Apple Health"))
    );
    assert!(from_first[1].contains("distinctive-version-11"));

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
    fs::write(dir.join("tombstone.json"), "{}").unwrap();
    let list = call(root, "list_transcripts", json!({"day": DAY}));
    assert!(list.get("other_sources").is_none(), "{list}");
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
    fs::write(deleted.join("tombstone.json"), "{}").unwrap();
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
