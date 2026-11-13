// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A read succeeds exactly when the connection's own lists show the thing.
//!
//! For each state of a journal and each kind of connection, every activity,
//! segment and entity the journal's files hold is read by its address alone,
//! the way anything an agent could write for itself would reach it. What the
//! connection is shown comes from its own answers over the real dispatch, never
//! from the functions under test.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};

use crate::dispatch::{DispatchError, DispatchPrincipal, codec, run_mcp_probe};
use crate::permissions::{
    PermissionDecision, PermissionStore, ReadPermission, ReadScope, evaluate_connection_read,
};
use crate::references::{
    ActivityAddress, ReferenceKind, ReferenceTarget, SegmentAddress, SourceSpanReference,
};
use crate::tools::ToolError;

const CONNECTION: &str = "bearer:may-see";
const HEALTH: &str = "123e4567-e89b-42d3-a456-426614174000";
const WORK: &str = "223e4567-e89b-42d3-a456-426614174000";
const DAY: &str = "20261010";
const WEEK: &str = "20261004";
const BODY_STREAM: &str = "import.apple_health";
const BODY_LABEL: &str = "Apple Health · imported";
const PRINCIPAL: DispatchPrincipal<'static> = DispatchPrincipal {
    connection: CONNECTION,
    agent_identity: CONNECTION,
};

fn reading(id: &str, at: &str) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Reading,
        id.into(),
        "apple_health",
        "HKQuantityTypeIdentifierHeartRate",
        "heart rate".into(),
    );
    row.at = Some(at.into());
    row.value = Some("61".into());
    row.unit = Some("count/min".into());
    row.by = Some("Synthetic Watch".into());
    row
}

fn run(id: &str, start: &str, end: &str) -> BodyRow {
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
    row.by = Some("Synthetic Watch".into());
    row
}

fn moved(id: &str, of: &str) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Moved,
        id.into(),
        "apple_health",
        "walking",
        "walking".into(),
    );
    row.of = Some(of.into());
    row
}

fn window(root: &Path, key: &str, rows: &[BodyRow], facet: Option<&str>) {
    let dir = root.join("chronicle").join(DAY).join(BODY_STREAM).join(key);
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

fn spoken(root: &Path, key: &str, facet: Option<&str>, words: &str) {
    let dir = root.join("chronicle").join(DAY).join("desk").join(key);
    fs::create_dir_all(dir.join("talents")).unwrap();
    if let Some(facet) = facet {
        fs::write(
            dir.join("talents/facets.json"),
            json!([{ "facet": facet }]).to_string(),
        )
        .unwrap();
    }
    fs::write(
        dir.join("audio.jsonl"),
        format!("{{\"source\":\"recording\"}}\n{{\"start\":\"00:00:02\",\"text\":\"{words}\"}}\n"),
    )
    .unwrap();
}

fn entity(root: &Path, facet: &str, id: &str, name: &str) {
    let link = root.join("facets").join(facet).join("entities").join(id);
    fs::create_dir_all(&link).unwrap();
    fs::write(
        link.join("entity.json"),
        json!({"entity_id": id, "last_seen": DAY}).to_string(),
    )
    .unwrap();
    let identity = root.join("entities").join(id);
    fs::create_dir_all(&identity).unwrap();
    fs::write(
        identity.join("entity.json"),
        json!({"id": id, "name": name, "type": "Person"}).to_string(),
    )
    .unwrap();
}

fn activities(root: &Path, facet: &str, records: &[Value]) {
    let path = root.join(format!("facets/{facet}/activities/{DAY}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = records.iter().map(|record| format!("{record}\n")).collect();
    fs::write(path, text).unwrap();
}

/// A journal holding one of everything a list leaves out, beside what it shows.
fn journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("solstone-mcp-may-see-")
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
            "name": BODY_STREAM, "kind": "import:apple_health", "host": null, "platform": null,
            "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        root.join("streams/phone_calendar.json"),
        json!({
            "name": "phone_calendar", "kind": "observer", "host": null, "platform": null,
            "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
            "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "source": "calendar",
        })
        .to_string(),
    )
    .unwrap();

    // The zone that places the body source's windows.
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        json!({ "zone": "America/Denver" }).to_string(),
    )
    .unwrap();

    // What was said: one segment in each facet and one in none.
    spoken(root, "090000_300", Some("work"), "standup words");
    spoken(root, "100000_300", Some("health"), "walk words");
    spoken(root, "110000_300", None, "unfiled words");
    // One that holds body data beside what was said: hiding the day's body
    // data takes all of it.
    spoken(root, "113000_300", Some("health"), "mixed words");
    fs::write(
        root.join(format!("chronicle/{DAY}/desk/113000_300/body.jsonl")),
        encode_rows(&[reading("x1", "2026-10-10 11:31:00 -0600")]),
    )
    .unwrap();
    // Body windows: one in each facet, and one whose rows all moved away.
    window(
        root,
        "063000_300",
        &[
            reading("a1", "2026-10-10 06:31:00 -0600"),
            run(
                "run1",
                "2026-10-10 06:31:00 -0600",
                "2026-10-10 06:34:00 -0600",
            ),
        ],
        Some("health"),
    );
    window(
        root,
        "064000_300",
        &[reading("b1", "2026-10-10 06:41:00 -0600")],
        Some("work"),
    );
    window(
        root,
        "065000_300",
        &[moved("m1", "elsewhere")],
        Some("health"),
    );
    // A Strava workout tile and a calendar note: neither is read as a segment.
    let tile = root
        .join("chronicle")
        .join(DAY)
        .join("import.strava")
        .join("070000_300");
    fs::create_dir_all(tile.join("talents")).unwrap();
    fs::write(
        tile.join("workout.json"),
        json!({
            "schema": "solstone.import.strava.tile.v1", "activity_id": 9,
            "tile": {"index": 0, "count": 1},
            "workout": {"name": "Morning Run", "type": "Run", "distance_m": 10200.0}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(tile.join("stream.json"), "{}").unwrap();
    fs::write(
        tile.join("talents/facets.json"),
        json!([{ "facet": "health" }]).to_string(),
    )
    .unwrap();
    let calendar = root
        .join("chronicle")
        .join(DAY)
        .join("phone_calendar")
        .join("120000_300");
    fs::create_dir_all(&calendar).unwrap();
    fs::write(
        calendar.join("stream.json"),
        r#"{"stream":"phone_calendar","prev_day":null,"prev_segment":null,"seq":1}"#,
    )
    .unwrap();
    fs::write(
        calendar.join("calendar_0.jsonl"),
        "{\"title\":\"Planning\",\"start\":\"2026-10-10T12:00:00-06:00\"}\n",
    )
    .unwrap();

    activities(
        root,
        "work",
        &[
            json!({"id": "meeting_1", "activity": "meeting", "title": "Standup",
                "stream": "desk", "segments": ["090000_300"], "created_at": 1,
                "level_avg": 0.9}),
            json!({"id": "meeting_2", "activity": "meeting", "title": "Hidden review",
                "segments": ["090000_300"], "created_at": 2, "hidden": true}),
            json!({"id": "plan_1", "activity": "meeting", "title": "Retired plan",
                "source": "anticipated", "created_at": 3}),
        ],
    );
    activities(
        root,
        "health",
        &[
            json!({"id": "walk_1", "activity": "walking", "title": "Walk",
                "segments": ["100000_300"], "created_at": 1}),
            // The standup again, as the health facet holds it: one moment in
            // two facets, which a list shows once.
            json!({"id": "meeting_1h", "activity": "meeting", "title": "Standup",
                "stream": "desk", "segments": ["090000_300"], "created_at": 1,
                "level_avg": 0.2}),
            // A Strava line thinking once wrote, still standing.
            json!({"id": "exercise_1", "activity": "exercise", "title": "Morning Run",
                "stream": "import.strava", "segments": ["070000_300"], "created_at": 2}),
            // And one a code record has folded in: superseded.
            json!({"id": "exercise_2", "activity": "exercise", "title": "Folded Run",
                "stream": "import.strava", "segments": ["071000_300"], "created_at": 3}),
            json!({"id": "body_code", "source": "body", "activity": "workout",
                "account": {"key": "import.strava\u{1f}1"},
                "converted": {"primary": "p", "decided_at": "t", "lines": {
                    format!("{DAY}|health|exercise_2"):
                        {"sha256": "x", "hidden": false, "fields": {}}}}}),
            json!({"id": "body_run1", "source": "body", "activity": "workout",
                "account": {"key": format!("{BODY_STREAM}\u{1f}run1"),
                    "start": "2026-10-10 06:31:00 -0600", "end": "2026-10-10 06:34:00 -0600",
                    "type": "running",
                    "recorder": recorder_id(BODY_STREAM, Some("Synthetic Watch"))}}),
        ],
    );
    // A folder a facet merge leaves behind: never a facet, whatever it holds.
    let scratch = root.join("facets/.merge-scratch");
    fs::create_dir_all(scratch.join("activities")).unwrap();
    fs::write(
        scratch.join("facet.json"),
        json!({"id": WORK, "title": "scratch", "description": "scratch"}).to_string(),
    )
    .unwrap();
    fs::write(
        scratch.join(format!("activities/{DAY}.jsonl")),
        format!(
            "{}\n",
            json!({"id": "scratch_1", "activity": "meeting", "title": "Scratch", "created_at": 1})
        ),
    )
    .unwrap();

    entity(root, "work", "alice", "Alice Example");
    entity(root, "health", "bob", "Bob Example");

    let report = solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    assert_eq!(report.failed, 0, "{report:?}");
    journal
}

fn grant(root: &Path, kinds: &[&str], scope: ReadScope) {
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
                scope,
            },
        )
        .unwrap();
}

fn snapshot(root: &Path) -> crate::permissions::ConnectionReadSnapshot {
    match evaluate_connection_read(root, CONNECTION) {
        PermissionDecision::Snapshot(snapshot) => snapshot,
        PermissionDecision::Denied { reason } => panic!("no snapshot: {reason}"),
    }
}

/// Every page of one list, or nothing when the connection may not call it.
fn pages(root: &Path, tool: &str, arguments: Value) -> Vec<Value> {
    let mut answers = Vec::new();
    let mut arguments = arguments;
    loop {
        let Ok(answer) = run_mcp_probe(root, CONNECTION, tool, &arguments) else {
            return answers;
        };
        let next = answer.get("next_cursor").cloned();
        answers.push(answer);
        match next {
            Some(Value::String(cursor)) => arguments["cursor"] = json!(cursor),
            _ => return answers,
        }
    }
}

/// Every reference string anywhere in an answer.
fn references(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if let (true, Some(text)) = (
                    matches!(key.as_str(), "reference" | "source"),
                    value.as_str(),
                ) {
                    found.push(text.to_owned());
                }
                references(value, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| references(item, found)),
        _ => {}
    }
}

#[derive(Default, Debug)]
struct Shown {
    /// (facet id, day, record identity)
    activities: BTreeSet<(String, String, String)>,
    /// (day, stream, segment)
    segments: BTreeSet<(String, String, String)>,
    /// (facet id, entity id)
    entities: BTreeSet<(String, String)>,
}

/// What this connection's own lists show it, read from their answers.
fn shown(root: &Path) -> Shown {
    let mut answers = Vec::new();
    answers.extend(pages(
        root,
        "list_activities",
        json!({"day_from": "20261003", "day_to": "20261017", "limit": 100}),
    ));
    answers.extend(pages(root, "get_week", json!({"week": WEEK})));
    answers.extend(pages(
        root,
        "list_transcripts",
        json!({"day": DAY, "limit": 100}),
    ));
    answers.extend(pages(
        root,
        "list_transcripts",
        json!({"day": DAY, "source": BODY_LABEL, "limit": 100}),
    ));
    answers.extend(pages(root, "list_entities", json!({"limit": 100})));
    let mut found = Vec::new();
    for answer in &answers {
        references(answer, &mut found);
    }
    let generation = snapshot(root).generation;
    let mut shown = Shown::default();
    for reference in found {
        for kind in [
            ReferenceKind::Activity,
            ReferenceKind::Segment,
            ReferenceKind::Entity,
        ] {
            match codec().resolve(&reference, kind, CONNECTION, generation) {
                Ok(ReferenceTarget::Activity(activity)) => {
                    shown.activities.insert((
                        activity.facet_id,
                        activity.day,
                        activity.record_identity,
                    ));
                }
                Ok(ReferenceTarget::Segment(segment)) => {
                    shown
                        .segments
                        .insert((segment.day, segment.stream, segment.segment));
                }
                Ok(ReferenceTarget::Entity(entity)) => {
                    shown.entities.insert((entity.facet_id, entity.entity_id));
                }
                _ => {}
            }
        }
    }
    shown
}

fn not_found<T>(result: &Result<T, DispatchError>) -> bool {
    matches!(
        result,
        Err(DispatchError::Tool(ToolError::ReferenceNotFound))
    )
}

fn folders(path: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[derive(Default, Debug)]
struct Tally {
    read: usize,
    refused: usize,
}

impl Tally {
    fn count(&mut self, read: bool) {
        if read {
            self.read += 1;
        } else {
            self.refused += 1;
        }
    }
}

/// Read everything the journal's files hold by address, and hold each read to
/// what the connection's own lists show.
fn hold_reads_to_lists(root: &Path, state: &str, kinds: &[&str], tallies: &mut [Tally; 3]) {
    // `get_activity_sources` is offered only with transcripts or body beside
    // facets: without either, no activity's sources are read.
    let reads_sources = kinds.contains(&"transcripts") || kinds.contains(&"body");
    let shown = shown(root);
    let snapshot = snapshot(root);
    let facet_ids = |folder: &str| {
        fs::read_to_string(root.join("facets").join(folder).join("facet.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|facet| facet["id"].as_str().map(str::to_owned))
            .unwrap_or_default()
    };

    // The facets this connection's own list names.
    let reached: BTreeSet<String> = pages(root, "list_facets", json!({}))
        .iter()
        .flat_map(|answer| answer["facets"].as_array().cloned().unwrap_or_default())
        .filter_map(|facet| facet["id"].as_str().map(str::to_owned))
        .collect();
    // What copies of one moment share, for each activity the lists show.
    let moment = |record: &Value| {
        let stream = record["stream"].as_str()?;
        let segments = record["segments"].as_array()?;
        Some(format!("{stream}|{}", Value::Array(segments.clone())))
    };
    let mut listed_moments = BTreeSet::new();
    for folder in folders(&root.join("facets")) {
        let facet_id = facet_ids(&folder);
        let text = fs::read_to_string(root.join(format!("facets/{folder}/activities/{DAY}.jsonl")))
            .unwrap_or_default();
        for record in text.lines().filter(|line| !line.trim().is_empty()) {
            let record: Value = serde_json::from_str(record).unwrap();
            let id = record["id"].as_str().unwrap().to_owned();
            if !folder.starts_with('.')
                && shown
                    .activities
                    .contains(&(facet_id.clone(), DAY.to_owned(), id))
                && let Some(moment) = moment(&record)
            {
                listed_moments.insert(moment);
            }
        }
    }

    for folder in folders(&root.join("facets")) {
        let facet_id = facet_ids(&folder);
        let Ok(text) =
            fs::read_to_string(root.join(format!("facets/{folder}/activities/{DAY}.jsonl")))
        else {
            continue;
        };
        for record in text.lines().filter(|line| !line.trim().is_empty()) {
            let record: Value = serde_json::from_str(record).unwrap();
            let id = record["id"].as_str().unwrap();
            // A dot folder is no facet, so nothing in it is listed under any.
            // A moment held by two facets is listed once, under one of them:
            // its other copy reads too, for a connection that reaches the
            // copy's facet, and is a row of that facet's own list.
            let listed = !folder.starts_with('.')
                && (shown
                    .activities
                    .contains(&(facet_id.clone(), DAY.to_owned(), id.to_owned()))
                    || (reached.contains(&facet_id)
                        && moment(&record).is_some_and(|moment| listed_moments.contains(&moment))));
            // By folder and by id: both name the same facet.
            for facet in [folder.as_str(), facet_id.as_str()] {
                if facet == facet_id && folder.starts_with('.') {
                    continue;
                }
                let address = ActivityAddress {
                    facet: facet.to_owned(),
                    day: DAY.to_owned(),
                    record_identity: id.to_owned(),
                    file_digest: None,
                    unit_index: None,
                    source_binding: None,
                };
                let fetched = crate::dispatch::fetch_activity(root, PRINCIPAL, &snapshot, &address);
                assert_eq!(
                    fetched.is_ok(),
                    listed,
                    "{state}: fetch of {facet}/{id}, listed {listed}"
                );
                assert!(
                    fetched.is_ok() || not_found(&fetched),
                    "{state}: {facet}/{id}"
                );
                let sources = crate::tools::activities::sources_of(
                    root, PRINCIPAL, &snapshot, &address, None,
                );
                assert_eq!(
                    !not_found(&sources),
                    listed && reads_sources,
                    "{state}: sources of {facet}/{id}, listed {listed}"
                );
                tallies[0].count(listed);
            }
        }
    }

    for stream in folders(&root.join("chronicle").join(DAY)) {
        for segment in folders(&root.join("chronicle").join(DAY).join(&stream)) {
            let listed =
                shown
                    .segments
                    .contains(&(DAY.to_owned(), stream.clone(), segment.clone()));
            let read = crate::dispatch::read_segment(
                root,
                PRINCIPAL,
                &snapshot,
                &SegmentAddress {
                    day: DAY.to_owned(),
                    stream: stream.clone(),
                    segment: segment.clone(),
                    version: None,
                },
                None,
            );
            assert_eq!(
                read.is_ok(),
                listed,
                "{state}: read of {stream}/{segment}, listed {listed}"
            );
            assert!(
                read.is_ok() || not_found(&read),
                "{state}: {stream}/{segment}"
            );
            tallies[1].count(listed);
        }
    }

    for folder in folders(&root.join("facets")) {
        let facet_id = facet_ids(&folder);
        for entity in folders(&root.join("facets").join(&folder).join("entities")) {
            let listed = shown.entities.contains(&(facet_id.clone(), entity.clone()));
            for facet in [folder.as_str(), facet_id.as_str()] {
                let read = crate::dispatch::read_entity(root, &snapshot, facet, &entity);
                assert_eq!(
                    read.is_ok(),
                    listed,
                    "{state}: entity {entity} of {facet}, listed {listed}"
                );
                assert!(
                    read.is_ok() || not_found(&read),
                    "{state}: {facet}/{entity}"
                );
                tallies[2].count(listed);
            }
        }
    }
}

const ALL: &[&str] = &["transcripts", "entities", "facets", "screen_text", "body"];

/// A kind of connection: what to call it, the categories it holds, its scope.
type Connection = (&'static str, Vec<&'static str>, ReadScope);

fn connections() -> Vec<Connection> {
    let work = || ReadScope::Facets {
        ids: vec![WORK.to_owned()],
    };
    let health = || ReadScope::Facets {
        ids: vec![HEALTH.to_owned()],
    };
    vec![
        ("everything", ALL.to_vec(), ReadScope::WholeJournal),
        ("work only", ALL.to_vec(), work()),
        ("health only", ALL.to_vec(), health()),
        (
            "no body",
            vec!["transcripts", "entities", "facets", "screen_text"],
            ReadScope::WholeJournal,
        ),
        ("facets alone", vec!["facets"], ReadScope::WholeJournal),
        (
            "transcripts alone",
            vec!["transcripts"],
            ReadScope::WholeJournal,
        ),
        ("body alone", vec!["body"], ReadScope::WholeJournal),
        ("entities alone, health", vec!["entities"], health()),
        ("facets and body, health", vec!["facets", "body"], health()),
    ]
}

fn hide_day(root: &Path) {
    use solstone_core_format::content::body_hidden_days::{hidden_days_bytes, hidden_days_path};
    let path = hidden_days_path(root);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, hidden_days_bytes(&[DAY.to_owned()].into())).unwrap();
}

/// The states an owner's journal passes through, each on a fresh journal.
/// A state of the journal: what to call it, and what makes it so.
type State = (&'static str, fn(&Path));

fn states() -> Vec<State> {
    vec![
        ("as recorded", |_| {}),
        ("the day's body data hidden", hide_day),
        ("hidden days unreadable", |root| {
            let path = solstone_core_format::content::body_hidden_days::hidden_days_path(root);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "{ damaged").unwrap();
        }),
        ("a segment deleted", |root| {
            fs::write(
                root.join(format!("chronicle/{DAY}/desk/090000_300/tombstone.json")),
                "{}",
            )
            .unwrap();
        }),
        ("a segment moved to the other facet", |root| {
            fs::write(
                root.join(format!(
                    "chronicle/{DAY}/desk/090000_300/talents/facets.json"
                )),
                json!([{ "facet": "health" }]).to_string(),
            )
            .unwrap();
        }),
        ("an activity hidden after it was listed", |root| {
            let path = root.join(format!("facets/health/activities/{DAY}.jsonl"));
            let text = fs::read_to_string(&path)
                .unwrap()
                .replace(r#""id":"walk_1""#, r#""hidden":true,"id":"walk_1""#);
            fs::write(path, text).unwrap();
        }),
        ("a facet's declaration gone", |root| {
            fs::remove_file(root.join("facets/work/facet.json")).unwrap();
        }),
    ]
}

#[test]
fn every_read_by_address_succeeds_exactly_when_the_connections_lists_show_it() {
    // Activities, segments, entities.
    let mut tallies = [Tally::default(), Tally::default(), Tally::default()];
    for (state, apply) in states() {
        let journal = journal();
        let root = journal.path();
        apply(root);
        for (connection, kinds, scope) in connections() {
            grant(root, &kinds, scope);
            hold_reads_to_lists(
                root,
                &format!("{state} / {connection}"),
                &kinds,
                &mut tallies,
            );
        }
    }
    // Neither side of the comparison is empty for any kind.
    for (kind, tally) in ["activities", "segments", "entities"].iter().zip(&tallies) {
        assert!(tally.read > 20 && tally.refused > 20, "{kind}: {tally:?}");
    }
}

/// The gaps the sealed references covered, each named: what a list leaves out
/// reads as not found by address, for a connection that reads everything.
#[test]
fn what_a_list_leaves_out_is_not_found_by_address() {
    let journal = journal();
    let root = journal.path();
    grant(root, ALL, ReadScope::WholeJournal);
    let snapshot = snapshot(root);
    let activity = |facet: &str, id: &str| ActivityAddress {
        facet: facet.to_owned(),
        day: DAY.to_owned(),
        record_identity: id.to_owned(),
        file_digest: None,
        unit_index: None,
        source_binding: None,
    };
    let fetch = |facet: &str, id: &str| {
        crate::dispatch::fetch_activity(root, PRINCIPAL, &snapshot, &activity(facet, id))
    };
    let sources = |facet: &str, id: &str| {
        crate::tools::activities::sources_of(root, PRINCIPAL, &snapshot, &activity(facet, id), None)
    };
    // Shown: read both ways.
    assert!(fetch("work", "meeting_1").is_ok());
    assert!(sources("work", "meeting_1").is_ok());
    for (facet, id, what) in [
        ("work", "meeting_2", "a hidden activity"),
        ("work", "plan_1", "a retired plan"),
        ("health", "exercise_2", "a superseded Strava line"),
        (".merge-scratch", "scratch_1", "a merge's scratch folder"),
        ("work", "walk_1", "another facet's activity"),
        ("work/../health", "walk_1", "a path written as a facet"),
        ("nowhere", "meeting_1", "a facet that isn't there"),
    ] {
        assert!(not_found(&fetch(facet, id)), "fetch: {what}");
        assert!(not_found(&sources(facet, id)), "sources: {what}");
    }
    // A day that is no day, or one the facet holds no file for.
    for day in ["2026101", "../20261010", "20261011"] {
        let mut address = activity("work", "meeting_1");
        address.day = day.to_owned();
        assert!(not_found(&crate::dispatch::fetch_activity(
            root, PRINCIPAL, &snapshot, &address
        )));
    }

    let segment = |stream: &str, name: &str| {
        crate::dispatch::read_segment(
            root,
            PRINCIPAL,
            &snapshot,
            &SegmentAddress {
                day: DAY.to_owned(),
                stream: stream.to_owned(),
                segment: name.to_owned(),
                version: None,
            },
            None,
        )
    };
    assert!(segment("desk", "090000_300").is_ok());
    assert!(segment(BODY_STREAM, "063000_300").is_ok());
    for (stream, name, what) in [
        ("import.strava", "070000_300", "a Strava workout tile"),
        (BODY_STREAM, "065000_300", "a window whose rows all moved"),
        ("phone_calendar", "120000_300", "a calendar note"),
        ("desk", "130000_300", "a segment that isn't there"),
        ("desk/..", "090000_300", "a path written as a stream"),
    ] {
        assert!(not_found(&segment(stream, name)), "{what}");
    }

    // A body record reads as its face; a Strava line as its record.
    assert!(fetch("health", "body_run1").is_ok());
    assert!(fetch("health", "exercise_1").is_ok());

    // A span of its own belongs to a body source alone.
    let span = |stream: &str| {
        let reference = codec()
            .mint(
                CONNECTION,
                snapshot.generation,
                ReferenceTarget::SourceSpan(SourceSpanReference {
                    day: DAY.to_owned(),
                    stream: stream.to_owned(),
                    time_from: None,
                    time_to: None,
                    kind: None,
                    position: None,
                }),
            )
            .unwrap();
        run_mcp_probe(
            root,
            CONNECTION,
            "get_transcript",
            &json!({"reference": reference}),
        )
    };
    assert!(span(BODY_STREAM).is_ok());
    assert!(span("desk").is_err());
    assert!(span("import.strava").is_err());
}

/// A sealed reference still refuses what changed since it was listed, and a
/// reference from before the owner hid an activity reads nothing after.
#[test]
fn a_reference_from_before_a_hide_reads_nothing_after_it() {
    let journal = journal();
    let root = journal.path();
    grant(root, ALL, ReadScope::WholeJournal);
    let listed = run_mcp_probe(root, CONNECTION, "list_activities", &json!({"day": DAY})).unwrap();
    let reference = listed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["title"] == "Walk")
        .unwrap_or_else(|| panic!("{listed}"))["reference"]
        .clone();
    let read = |tool: &str| run_mcp_probe(root, CONNECTION, tool, &json!({"reference": reference}));
    assert!(read("fetch").is_ok());
    assert!(read("get_activity_sources").is_ok());
    let path = root.join(format!("facets/health/activities/{DAY}.jsonl"));
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace(r#""id":"walk_1""#, r#""hidden":true,"id":"walk_1""#);
    fs::write(path, text).unwrap();
    assert!(read("fetch").is_err());
    assert!(read("get_activity_sources").is_err());
}

/// Body data read by address follows the body permission and the owner's
/// hidden days, as every list of it does.
#[test]
fn body_data_by_address_needs_the_permission_and_a_day_that_shows() {
    let read = |root: &Path, id: &str| {
        let snapshot = snapshot(root);
        let address = ActivityAddress {
            facet: "health".to_owned(),
            day: DAY.to_owned(),
            record_identity: id.to_owned(),
            file_digest: None,
            unit_index: None,
            source_binding: None,
        };
        (
            crate::dispatch::fetch_activity(root, PRINCIPAL, &snapshot, &address),
            crate::tools::activities::sources_of(root, PRINCIPAL, &snapshot, &address, None),
        )
    };
    let window = |root: &Path| {
        crate::dispatch::read_segment(
            root,
            PRINCIPAL,
            &snapshot(root),
            &SegmentAddress {
                day: DAY.to_owned(),
                stream: BODY_STREAM.to_owned(),
                segment: "063000_300".to_owned(),
                version: None,
            },
            None,
        )
    };
    let journal = journal();
    let root = journal.path();
    grant(root, ALL, ReadScope::WholeJournal);
    for id in ["exercise_1", "body_run1"] {
        let (fetched, sources) = read(root, id);
        assert!(fetched.is_ok() && !not_found(&sources), "{id} with body");
    }
    assert!(window(root).is_ok());

    // Without the body permission: a Strava line, a body record and a body
    // window are each not found.
    grant(
        root,
        &["transcripts", "entities", "facets", "screen_text"],
        ReadScope::WholeJournal,
    );
    for id in ["exercise_1", "body_run1"] {
        let (fetched, sources) = read(root, id);
        assert!(
            not_found(&fetched) && not_found(&sources),
            "{id} without body"
        );
    }
    assert!(not_found(&window(root)));
    // An ordinary activity beside them still reads.
    assert!(read(root, "walk_1").0.is_ok());

    // With it again, on a day the owner hid.
    grant(root, ALL, ReadScope::WholeJournal);
    hide_day(root);
    for id in ["exercise_1", "body_run1"] {
        let (fetched, sources) = read(root, id);
        assert!(
            not_found(&fetched) && not_found(&sources),
            "{id} on a hidden day"
        );
    }
    assert!(not_found(&window(root)));
    assert!(read(root, "walk_1").0.is_ok());
}
