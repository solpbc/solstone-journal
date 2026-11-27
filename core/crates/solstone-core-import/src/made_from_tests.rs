// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]

use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_removals::{Action, Page, Surface, Unit};
use solstone_core_retention::NoIndex;
use solstone_core_retention::made_from::MadeFrom;
use solstone_core_trash::Change;
use tempfile::TempDir;

use super::MadeFromIt;

const DAY: &str = "20261110";
const FACET_ID: &str = "11111111-1111-4111-8111-111111111111";
const A: &str = "090000_300";
const B: &str = "090500_300";
const C: &str = "091000_300";
const D: &str = "100000_300";

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn sense(said: &str, who: &str) -> Value {
    json!({"density": "active", "content_type": "meeting", "activity_summary": said,
        "facets": [{"facet": "work", "level": "high", "activity": said}],
        "entities": [{"name": who}]})
}

fn activity(id: &str, segments: &[&str], words: &str) -> Value {
    json!({"id": id, "facet": "work", "activity": "meeting", "stream": "laptop",
        "segments": segments, "title": words, "description": words,
        "story": {"body": words}, "level_avg": 1.0, "active_entities": [],
        "destination_id": FACET_ID, "created_at": 1})
}

/// A day of `laptop` recordings: a meeting over A B C and one over D alone.
/// B and D said the secret.
fn journal() -> TempDir {
    let root = TempDir::new().unwrap();
    let root_path = root.path();
    write(
        root_path,
        "config/journal.json",
        br#"{"setup":{"completed_at":1700000000000}}"#,
    );
    write(
        root_path,
        "facets/work/facet.json",
        json!({"id": FACET_ID, "title": "work"})
            .to_string()
            .as_bytes(),
    );
    for (key, said, who) in [
        (A, "planning the launch", "Ann"),
        (B, "the kestrel secret", "Bob"),
        (C, "reviewing the launch", "Cat"),
        (D, "the kestrel secret again", "Dee"),
    ] {
        let dir = format!("chronicle/{DAY}/laptop/{key}");
        write(root_path, &format!("{dir}/audio.flac"), b"sound");
        write(
            root_path,
            &format!("{dir}/talents/sense.json"),
            sense(said, who).to_string().as_bytes(),
        );
    }
    let rows = [
        activity("meeting_090000_300", &[A, B, C], "the kestrel secret"),
        activity("meeting_100000_300", &[D], "the kestrel secret again"),
    ];
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    write(
        root_path,
        &format!("facets/work/activities/{DAY}.jsonl"),
        text.as_bytes(),
    );
    write(
        root_path,
        &format!("facets/work/activities/{DAY}/meeting_090000_300/story.md"),
        b"the kestrel secret",
    );
    root
}

fn records(root: &Path) -> Vec<Map<String, Value>> {
    solstone_core_facets::read_activity_records_strict(root, "work", DAY).unwrap()
}

fn surface() -> Surface {
    Surface::Web {
        page: Page::Transcripts,
    }
}

fn delete(root: &Path, key: &str, made: &MadeFromIt) -> solstone_core_trash::Item {
    let path = format!("chronicle/{DAY}/laptop/{key}");
    let (item, _) = crate::segment_trash::trash_segment(
        root,
        &NoIndex,
        made,
        Unit::Segment {
            day: DAY.into(),
            stream: "laptop".into(),
            segment: key.into(),
        },
        path,
        Action::SegmentDelete,
        surface(),
    )
    .unwrap();
    item
}

#[test]
fn a_deleted_recordings_activities_are_made_again_from_what_remains_and_asked_for() {
    let root = journal();
    let made = MadeFromIt::new();
    let b = delete(root.path(), B, &made);
    let d = delete(root.path(), D, &made);
    assert!(made.skipped().is_empty(), "{:?}", made.skipped());

    let rows = records(root.path());
    assert_eq!(rows.len(), 1, "the meeting over D alone goes");
    let meeting = &rows[0];
    assert_eq!(meeting["segments"], json!([A, C]));
    assert_eq!(meeting["description"], json!("reviewing the launch"));
    assert_eq!(meeting["active_entities"], json!(["Ann", "Cat"]));
    // The store titles a record from its description when it has no title.
    assert_eq!(meeting["title"], json!("reviewing the launch"));
    assert!(!meeting.contains_key("story"));
    let file = fs::read_to_string(
        root.path()
            .join(format!("facets/work/activities/{DAY}.jsonl")),
    )
    .unwrap();
    assert!(!file.contains("kestrel"), "{file}");
    assert!(
        !root
            .path()
            .join(format!("facets/work/activities/{DAY}/meeting_090000_300"))
            .exists(),
        "what its talents wrote goes with it"
    );

    // One ask for the day per delete, thawing the briefing, naming the
    // activity still there.
    let asks = solstone_core_system::owner_ask::read(root.path()).unwrap();
    assert_eq!(asks.len(), 2);
    assert!(
        asks.values()
            .all(|ask| ask.day == DAY && ask.thaw && ask.from_trash())
    );
    assert!(asks.values().any(|ask| {
        ask.to_seed
            .iter()
            .any(|asked| asked.activity == "meeting_090000_300")
    }));

    // A stop part way: the sweep runs the step again for the same item. The
    // copy kept for recover is the first one, and nothing is asked twice.
    let kept = solstone_core_trash::kept(root.path(), &b.id, "made-from.json")
        .unwrap()
        .unwrap();
    made.recordings_changed(
        root.path(),
        &b,
        Change::Detached,
        &[solstone_core_retention::receipt::Target {
            day: DAY.into(),
            stream: "laptop".into(),
            dir: B.into(),
        }],
    )
    .unwrap();
    assert_eq!(
        solstone_core_trash::kept(root.path(), &b.id, "made-from.json")
            .unwrap()
            .unwrap(),
        kept
    );
    assert_eq!(
        solstone_core_system::owner_ask::read(root.path())
            .unwrap()
            .len(),
        2
    );
    assert!(String::from_utf8(kept).unwrap().contains("kestrel"));

    // Recover puts each back with its recording, made again from all of it.
    for item in [&d, &b] {
        crate::trash_kinds::with_kinds(root.path(), |kinds| {
            solstone_core_trash::attach(root.path(), kinds, &item.id, Action::Recover, surface())
        })
        .unwrap();
    }
    let rows = records(root.path());
    assert_eq!(rows.len(), 2);
    let by_id = |id: &str| rows.iter().find(|row| row["id"] == json!(id)).unwrap();
    assert_eq!(by_id("meeting_090000_300")["segments"], json!([A, B, C]));
    assert_eq!(
        by_id("meeting_090000_300")["active_entities"],
        json!(["Ann", "Bob", "Cat"])
    );
    assert_eq!(by_id("meeting_100000_300")["segments"], json!([D]));
    let asks = solstone_core_system::owner_ask::read(root.path()).unwrap();
    assert!(asks.values().filter(|ask| ask.from_trash()).count() >= 3);
}

#[test]
fn a_memory_or_body_data_asks_for_nothing() {
    let root = journal();
    let made = MadeFromIt::new();
    write(
        root.path(),
        "chronicle/20261110/import.apple_health/090000_3600/body.json",
        b"{}",
    );
    let item = solstone_core_trash::Item {
        schema: 1,
        id: "x".into(),
        state: solstone_core_trash::State::Trashed,
        unit: Unit::Segment {
            day: DAY.into(),
            stream: "import.apple_health".into(),
            segment: "090000_3600".into(),
        },
        action: Action::SegmentDelete,
        surface: surface(),
        trashed_at: String::new(),
        settled_at: None,
        erase_at: String::new(),
        paths: Vec::new(),
        bytes: 0,
        pending: None,
        label: None,
        refs: Vec::new(),
        needs: Vec::new(),
        operation: None,
        linked: false,
        stretch: None,
    };
    made.recordings_changed(
        root.path(),
        &item,
        Change::Detached,
        &[solstone_core_retention::receipt::Target {
            day: DAY.into(),
            stream: "import.apple_health".into(),
            dir: "090000_3600".into(),
        }],
    )
    .unwrap();
    assert!(
        solstone_core_system::owner_ask::read(root.path())
            .unwrap()
            .is_empty()
    );
    assert_eq!(records(root.path()).len(), 2);
}

fn detections(root: &Path) -> Vec<Value> {
    solstone_core_facets::read_detected_entities(root, "work", DAY).unwrap()
}

fn origin(key: &str) -> String {
    format!("{DAY}/laptop/{key}")
}

fn voice(key: &str, sentence: u64) -> solstone_core_entity::VoiceprintItem {
    solstone_core_entity::VoiceprintItem {
        embedding: vec![0.0625; 256],
        metadata: json!({"schema_version": 1, "stream_layout": "named", "day": DAY,
            "segment_key": key, "source": "audio", "stream": "laptop",
            "sentence_id": sentence, "added_at": 1, "last_seen_ts": 1}),
    }
}

fn voice_keys(root: &Path, entity: &str) -> Vec<String> {
    let Some(archive) = solstone_core_entity::load_entity_voiceprints_file(root, entity) else {
        return Vec::new();
    };
    let mut keys: Vec<String> = archive
        .metadata
        .iter()
        .map(|row| {
            serde_json::from_str::<Value>(row).unwrap()["segment_key"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    keys.sort();
    keys
}

#[test]
fn what_a_recording_found_and_the_voices_drawn_from_it_leave_with_it_and_come_back() {
    let root = journal();
    let rows = [
        json!({"id": "bob", "type": "Person", "name": "Bob", "description": "said the kestrel secret",
            "described_by": origin(B), "segments": [origin(B)], "updated_at": 1}),
        json!({"id": "ann", "type": "Person", "name": "Ann", "description": "heard the kestrel secret",
            "described_by": origin(B), "segments": [origin(A), origin(B)], "updated_at": 1}),
        json!({"id": "cat", "type": "Person", "name": "Cat", "description": "reviewed it",
            "described_by": origin(C), "segments": [origin(B), origin(C)], "updated_at": 1}),
        json!({"id": "eve", "type": "Person", "name": "Eve", "description": "added by the owner",
            "updated_at": 1}),
    ];
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    write(
        root.path(),
        &format!("facets/work/entities/{DAY}.jsonl"),
        text.as_bytes(),
    );
    let encoder = solstone_core_entity::EncoderIdentity {
        id: "unresolved".into(),
        sha256: "0".repeat(64),
        width: 256,
    };
    for id in ["bob", "ann"] {
        solstone_core_entity::save_entity_identity(
            root.path(),
            id,
            &json!({"id": id, "name": id, "type": "Person"}),
            None,
        )
        .unwrap();
    }
    solstone_core_entity::save_voiceprints_batch(
        root.path(),
        "bob",
        &[voice(A, 1), voice(B, 2)],
        &encoder,
    )
    .unwrap();
    solstone_core_entity::save_voiceprints_batch(root.path(), "ann", &[voice(B, 3)], &encoder)
        .unwrap();

    let made = MadeFromIt::new();
    let b = delete(root.path(), B, &made);
    assert!(made.skipped().is_empty(), "{:?}", made.skipped());

    let after = detections(root.path());
    let by_name =
        |rows: &[Value], name: &str| rows.iter().find(|row| row["name"] == json!(name)).cloned();
    assert!(
        by_name(&after, "Bob").is_none(),
        "found only in it: gone with it"
    );
    let ann = by_name(&after, "Ann").unwrap();
    assert_eq!(ann["segments"], json!([origin(A)]));
    assert_eq!(ann["description"], "", "it may quote what left");
    let cat = by_name(&after, "Cat").unwrap();
    assert_eq!(cat["segments"], json!([origin(C)]));
    assert_eq!(
        cat["description"], "reviewed it",
        "a remaining recording wrote it"
    );
    assert_eq!(
        by_name(&after, "Eve").unwrap()["description"],
        "added by the owner"
    );
    assert_eq!(voice_keys(root.path(), "bob"), vec![A.to_owned()]);
    assert!(voice_keys(root.path(), "ann").is_empty());

    // A detection still running for it when it left writes nothing.
    solstone_core_facets::upsert_detection_segment(
        root.path(),
        "work",
        DAY,
        &origin(B),
        &[solstone_core_facets::DetectedEntityInput {
            entity_type: "Person".into(),
            name: "Bob".into(),
            description: "the kestrel secret".into(),
        }],
    )
    .unwrap();
    assert!(by_name(&detections(root.path()), "Bob").is_none());

    crate::trash_kinds::with_kinds(root.path(), |kinds| {
        solstone_core_trash::attach(root.path(), kinds, &b.id, Action::Recover, surface())
    })
    .unwrap();
    let back = detections(root.path());
    assert_eq!(
        by_name(&back, "Bob").unwrap()["segments"],
        json!([origin(B)])
    );
    let ann = by_name(&back, "Ann").unwrap();
    assert_eq!(ann["segments"], json!([origin(A), origin(B)]));
    assert_eq!(ann["description"], "heard the kestrel secret");
    assert_eq!(
        by_name(&back, "Cat").unwrap()["segments"],
        json!([origin(B), origin(C)])
    );
    assert_eq!(
        voice_keys(root.path(), "bob"),
        vec![A.to_owned(), B.to_owned()]
    );
    assert_eq!(voice_keys(root.path(), "ann"), vec![B.to_owned()]);
}

const MEMORY: &str = "091500_1";

fn memory_stream() -> String {
    format!("agent-memory-{}", "ab".repeat(32))
}

/// The day's journal plus one agent memory: `word` found only in it, and
/// Ann found in it and in A.
fn memory_journal() -> (TempDir, String) {
    let root = journal();
    let stream = memory_stream();
    write(
        root.path(),
        &format!("chronicle/{DAY}/{stream}/{MEMORY}/memory.md"),
        b"saved a test note on the owner's word",
    );
    let noted = format!("{DAY}/{stream}/{MEMORY}");
    let rows = [
        json!({"id": "word", "type": "Tool", "name": "word", "description": "used to save the test note",
            "described_by": noted, "segments": [noted], "updated_at": 1}),
        json!({"id": "ann", "type": "Person", "name": "Ann", "description": "planned the launch",
            "described_by": origin(A), "segments": [noted, origin(A)], "updated_at": 1}),
    ];
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    write(
        root.path(),
        &format!("facets/work/entities/{DAY}.jsonl"),
        text.as_bytes(),
    );
    (root, noted)
}

fn delete_memory(root: &Path, made: &dyn MadeFrom) -> solstone_core_trash::Item {
    solstone_core_retention::recording_trash::Recordings::new(&NoIndex, made)
        .trash(
            root,
            &solstone_core_retention::receipt::Target {
                day: DAY.into(),
                stream: memory_stream(),
                dir: MEMORY.into(),
            },
            Unit::Memory {
                day: DAY.into(),
                segment: MEMORY.into(),
            },
            Action::MemoryDelete,
            surface(),
        )
        .unwrap()
}

fn recover(root: &Path, item: &solstone_core_trash::Item) {
    crate::trash_kinds::with_kinds(root, |kinds| {
        solstone_core_trash::attach(root, kinds, &item.id, Action::Recover, surface())
    })
    .unwrap();
}

fn by_name(rows: &[Value], name: &str) -> Option<Value> {
    rows.iter().find(|row| row["name"] == json!(name)).cloned()
}

#[test]
fn what_an_agents_memory_found_leaves_with_it_and_comes_back() {
    let (root, noted) = memory_journal();
    let made = MadeFromIt::new();
    let item = delete_memory(root.path(), &made);
    assert!(made.skipped().is_empty(), "{:?}", made.skipped());

    let after = detections(root.path());
    assert!(by_name(&after, "word").is_none(), "found only in it: gone");
    assert_eq!(
        by_name(&after, "Ann").unwrap()["segments"],
        json!([origin(A)])
    );
    // Nothing else is re-made from a memory, and nothing is asked for.
    assert_eq!(records(root.path()).len(), 2);
    assert!(
        solstone_core_system::owner_ask::read(root.path())
            .unwrap()
            .is_empty()
    );

    recover(root.path(), &item);
    let back = detections(root.path());
    assert_eq!(by_name(&back, "word").unwrap()["segments"], json!([noted]));
    assert_eq!(
        by_name(&back, "Ann").unwrap()["segments"],
        json!([noted, origin(A)])
    );
}

#[test]
fn a_memory_trashed_before_this_drops_what_it_found_when_recovered_and_deleted_again() {
    let (root, noted) = memory_journal();
    // As before: the memory left and what it found stayed.
    let earlier = delete_memory(
        root.path(),
        &solstone_core_retention::made_from::NothingMade,
    );
    assert_eq!(
        by_name(&detections(root.path()), "word").unwrap()["segments"],
        json!([noted])
    );

    recover(root.path(), &earlier);
    let made = MadeFromIt::new();
    delete_memory(root.path(), &made);
    assert!(made.skipped().is_empty(), "{:?}", made.skipped());
    let after = detections(root.path());
    assert!(by_name(&after, "word").is_none());
    assert_eq!(
        by_name(&after, "Ann").unwrap()["segments"],
        json!([origin(A)])
    );
}

#[test]
fn erasing_a_recording_takes_a_late_runs_outputs_and_the_old_words_of_a_correction() {
    let root = journal();
    // The owner corrected the title of the meeting over A B C, before B went.
    let mut rows = records(root.path());
    let meeting = rows
        .iter_mut()
        .find(|row| row["id"] == json!("meeting_090000_300"))
        .unwrap();
    meeting.insert("title".into(), json!("my own title"));
    meeting.insert(
        "edits".into(),
        json!([{"timestamp": "2026-11-01T00:00:00Z", "actor": "owner:update",
            "fields": ["title"], "note": "", "set": {"title": "my own title"},
            "before": {"title": "the kestrel secret"}}]),
    );
    let text: String = rows
        .iter()
        .map(|row| format!("{}\n", Value::Object(row.clone())))
        .collect();
    write(
        root.path(),
        &format!("facets/work/activities/{DAY}.jsonl"),
        text.as_bytes(),
    );

    let made = MadeFromIt::new();
    let b = delete(root.path(), B, &made);
    // A run going at the delete wrote into its place afterwards.
    let place = root.path().join(format!("chronicle/{DAY}/laptop/{B}"));
    write(
        root.path(),
        &format!("chronicle/{DAY}/laptop/{B}/talents/sense.json"),
        b"the kestrel secret",
    );
    assert!(place.join("talents").exists());

    crate::trash_kinds::with_kinds(root.path(), |kinds| {
        solstone_core_trash::erase(root.path(), kinds, &b.id, Action::Erase, surface())
    })
    .unwrap();
    let left: Vec<String> = fs::read_dir(&place)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.ends_with(".lock"))
        .collect();
    assert_eq!(left, vec!["tombstone.json".to_owned()]);

    let meeting = records(root.path())
        .into_iter()
        .find(|row| row["id"] == json!("meeting_090000_300"))
        .unwrap();
    assert_eq!(meeting["title"], "my own title", "the correction stays");
    assert!(meeting["edits"][0].get("before").is_none());
    assert!(
        !fs::read_to_string(
            root.path()
                .join(format!("facets/work/activities/{DAY}.jsonl"))
        )
        .unwrap()
        .contains("\"the kestrel secret\"")
    );
    let refused = solstone_core_facets::remove_activity_edit(
        root.path(),
        "work",
        DAY,
        "meeting_090000_300",
        0,
        "2026-11-01T00:00:00Z",
        "owner:update",
        "2026-11-20T00:00:00Z",
    );
    assert!(
        matches!(
            refused,
            Err(solstone_core_facets::ActivityRecordStoreError::EditRefused(
                solstone_core_facets::EditRefusal::NotRecorded
            ))
        ),
        "{refused:?}"
    );
}

#[test]
fn a_recover_brings_back_only_its_own_and_the_owners_words_stay() {
    let root = journal();
    let rows = [
        json!({"id": "dee", "type": "Person", "name": "Dee", "description": "my own note",
            "described_by": "owner", "segments": [origin(A), origin(B)], "updated_at": 1}),
        json!({"id": "fay", "type": "Person", "name": "Fay", "description": "said it last",
            "described_by": origin(D), "segments": [origin(A), origin(B), origin(D)], "updated_at": 1}),
    ];
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    write(
        root.path(),
        &format!("facets/work/entities/{DAY}.jsonl"),
        text.as_bytes(),
    );
    solstone_core_entity::save_entity_identity(
        root.path(),
        "bob",
        &json!({"id": "bob", "name": "bob", "type": "Person"}),
        None,
    )
    .unwrap();
    let encoder = solstone_core_entity::EncoderIdentity {
        id: "unresolved".into(),
        sha256: "0".repeat(64),
        width: 256,
    };

    let made = MadeFromIt::new();
    let b = delete(root.path(), B, &made);
    let by_name = |name: &str| {
        detections(root.path())
            .into_iter()
            .find(|row| row["name"] == json!(name))
            .unwrap()
    };
    assert_eq!(by_name("Dee")["description"], "my own note");
    assert_eq!(by_name("Dee")["segments"], json!([origin(A)]));

    // A voice heard in it, saved by a run that read it before it left, isn't.
    let saved =
        solstone_core_entity::save_voiceprints_batch(root.path(), "bob", &[voice(B, 9)], &encoder)
            .unwrap();
    assert_eq!(saved, 0);
    assert!(voice_keys(root.path(), "bob").is_empty());

    let _d = delete(root.path(), D, &made);
    assert_eq!(by_name("Fay")["segments"], json!([origin(A)]));
    crate::trash_kinds::with_kinds(root.path(), |kinds| {
        solstone_core_trash::attach(root.path(), kinds, &b.id, Action::Recover, surface())
    })
    .unwrap();
    let fay = by_name("Fay");
    assert_eq!(
        fay["segments"],
        json!([origin(A), origin(B)]),
        "D is still in the trash"
    );
    assert_eq!(
        fay["description"], "",
        "D wrote it, and D is still in the trash"
    );
    assert_eq!(by_name("Dee")["description"], "my own note");
}

#[test]
fn default_stream_suffixed_activities_and_anchored_user_rows_trash_and_recover() {
    let root = TempDir::new().unwrap();
    let root_path = root.path();
    write(
        root_path,
        "config/journal.json",
        br#"{"setup":{"completed_at":1700000000000}}"#,
    );
    write(
        root_path,
        "facets/work/facet.json",
        json!({"id": FACET_ID, "title": "work"})
            .to_string()
            .as_bytes(),
    );
    for (dir_name, said, who) in [
        (A, "planning the launch", "Ann"),
        (B, "the kestrel secret", "Bob"),
        ("091000_300_summary", "reviewing the launch", "Cat"),
        (D, "the kestrel secret again", "Dee"),
    ] {
        let dir = format!("chronicle/{DAY}/{dir_name}");
        write(root_path, &format!("{dir}/audio.flac"), b"sound");
        write(
            root_path,
            &format!("{dir}/talents/sense.json"),
            sense(said, who).to_string().as_bytes(),
        );
    }
    let default_activity = |id: &str, segments: &[&str], words: &str| {
        json!({
            "id": id, "facet": "work", "activity": "meeting",
            "segments": segments, "title": words, "description": words,
            "story": {"body": words},
            "participation_said": [{"name": "Bob", "quote": words}],
            "level_avg": 1.0, "active_entities": [],
            "destination_id": FACET_ID, "created_at": 1
        })
    };
    let user_anchored = json!({
        "id": "meeting_user_anchor", "facet": "work", "activity": "meeting",
        "source": "user", "segments": [D],
        "title": "Owner manual title", "description": "Owner manual words",
        "details": "Keep this note", "active_entities": [], "created_at": 2
    });
    let rows = [
        default_activity("meeting_090000_300", &[A, B, C], "the kestrel secret"),
        default_activity("meeting_100000_300", &[D], "the kestrel secret again"),
        user_anchored,
    ];
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    write(
        root_path,
        &format!("facets/work/activities/{DAY}.jsonl"),
        text.as_bytes(),
    );

    let trash_default = |key: &str, made: &MadeFromIt| {
        let path = format!("chronicle/{DAY}/{key}");
        let (item, _) = crate::segment_trash::trash_segment(
            root_path,
            &NoIndex,
            made,
            Unit::Segment {
                day: DAY.into(),
                stream: solstone_core_journal_io::DEFAULT_STREAM.into(),
                segment: key.into(),
            },
            path,
            Action::SegmentDelete,
            surface(),
        )
        .unwrap();
        item
    };

    let made = MadeFromIt::new();
    let b_item = trash_default(B, &made);
    let d_item = trash_default(D, &made);
    assert!(made.skipped().is_empty(), "{:?}", made.skipped());

    // Search hiding detects gone _default segments on streamless records.
    let stale_default = default_activity("stale", &[B], "kestrel")
        .as_object()
        .unwrap()
        .clone();
    assert!(solstone_core_segment::record_names_gone_segment(
        root_path,
        DAY,
        &stale_default
    ));

    let rows = records(root_path);
    assert_eq!(
        rows.len(),
        2,
        "single-segment _default row goes, user row stays"
    );
    fn by_id<'a>(rows: &'a [Map<String, Value>], id: &str) -> &'a Map<String, Value> {
        rows.iter().find(|row| row["id"] == json!(id)).unwrap()
    }
    let meeting = by_id(&rows, "meeting_090000_300");
    assert_eq!(meeting["segments"], json!([A, C]));
    assert_eq!(meeting["description"], json!("reviewing the launch"));
    assert_eq!(meeting["active_entities"], json!(["Ann", "Cat"]));
    assert!(!meeting.contains_key("participation_said"));
    assert!(!meeting.contains_key("story"));

    let manual = by_id(&rows, "meeting_user_anchor");
    assert_eq!(manual["segments"], json!([]));
    assert_eq!(manual["title"], json!("Owner manual title"));
    assert_eq!(manual["description"], json!("Owner manual words"));
    assert_eq!(manual["details"], json!("Keep this note"));

    let file =
        fs::read_to_string(root_path.join(format!("facets/work/activities/{DAY}.jsonl"))).unwrap();
    assert!(!file.contains("kestrel"), "{file}");

    // Recover both B and D: multi-segment row restores B, single-segment row returns,
    // and segment-anchored source: "user" row regains its restored segment link [D].
    for item in [&b_item, &d_item] {
        crate::trash_kinds::with_kinds(root_path, |kinds| {
            solstone_core_trash::attach(root_path, kinds, &item.id, Action::Recover, surface())
        })
        .unwrap();
    }
    let restored = records(root_path);
    assert_eq!(restored.len(), 3);
    assert_eq!(
        by_id(&restored, "meeting_090000_300")["segments"],
        json!([A, B, C])
    );
    assert_eq!(
        by_id(&restored, "meeting_090000_300")["active_entities"],
        json!(["Ann", "Bob", "Cat"])
    );
    assert_eq!(
        by_id(&restored, "meeting_100000_300")["segments"],
        json!([D])
    );
    let restored_manual = by_id(&restored, "meeting_user_anchor");
    assert_eq!(restored_manual["segments"], json!([D]));
    assert_eq!(restored_manual["title"], json!("Owner manual title"));
    assert_eq!(restored_manual["description"], json!("Owner manual words"));
}

#[test]
fn experience_cascades_on_trash_recovers_on_untrash_and_scrubs_on_erase() {
    use solstone_core_facets::{
        ActivityRef, ComposedExperienceUpdate, ExperienceDirectPatch, ExperienceMoment,
        apply_composed_experience, create_owner_experience, load_experiences,
        update_experience_direct,
    };

    let root = journal();
    let root_path = root.path();

    // Experience 1 spans two activities: A/B/C and D, with owner-held title and two moments.
    let exp_two = create_owner_experience(
        root_path,
        "work",
        vec![
            ActivityRef {
                day: DAY.to_owned(),
                stream: "laptop".to_owned(),
                id: "meeting_090000_300".to_owned(),
                facet: None,
            },
            ActivityRef {
                day: DAY.to_owned(),
                stream: "laptop".to_owned(),
                id: "meeting_100000_300".to_owned(),
                facet: None,
            },
        ],
        Some("Launch day".to_owned()),
        Some("Original summary".to_owned()),
        "owner",
        "2026-11-10T12:00:00Z",
    )
    .unwrap();
    apply_composed_experience(
        root_path,
        "work",
        DAY,
        &exp_two.id,
        ComposedExperienceUpdate {
            title: "Launch day".to_owned(),
            summary: "Planning and follow-up.".to_owned(),
            locations: vec![],
            entity_refs: vec![],
            moments: vec![
                ExperienceMoment {
                    id: "m1".to_owned(),
                    started_at: "2026-11-10 09:00".to_owned(),
                    ended_at: "2026-11-10 09:15".to_owned(),
                    summary: "Planning session.".to_owned(),
                    activity_refs: vec![ActivityRef {
                        day: DAY.to_owned(),
                        stream: "laptop".to_owned(),
                        id: "meeting_090000_300".to_owned(),
                        facet: None,
                    }],
                    segment_refs: vec![format!("{DAY}/laptop/{A}")],
                    media_refs: vec![],
                },
                ExperienceMoment {
                    id: "m2".to_owned(),
                    started_at: "2026-11-10 10:00".to_owned(),
                    ended_at: "2026-11-10 10:05".to_owned(),
                    summary: "Follow-up containing kestrel secret.".to_owned(),
                    activity_refs: vec![ActivityRef {
                        day: DAY.to_owned(),
                        stream: "laptop".to_owned(),
                        id: "meeting_100000_300".to_owned(),
                        facet: None,
                    }],
                    segment_refs: vec![format!("{DAY}/laptop/{D}")],
                    media_refs: vec![],
                },
            ],
            started_at: None,
            ended_at: None,
        },
        "2026-11-10T12:05:00Z",
    )
    .unwrap();
    // Stamp an owner edit so `edits[].before` carries previous summary text (exercising erase scrub).
    update_experience_direct(
        root_path,
        "work",
        DAY,
        &exp_two.id,
        ExperienceDirectPatch {
            title: Some("Owner held launch day".to_owned()),
            summary: Some("Edited summary".to_owned()),
            ..Default::default()
        },
        "owner",
        "2026-11-10T12:10:00Z",
    )
    .unwrap();

    // Experience 2 depends solely on segment D -> must retire when D is trashed.
    let exp_one = create_owner_experience(
        root_path,
        "work",
        vec![ActivityRef {
            day: DAY.to_owned(),
            stream: "laptop".to_owned(),
            id: "meeting_100000_300".to_owned(),
            facet: None,
        }],
        Some("Solo D meeting".to_owned()),
        Some("Only segment D.".to_owned()),
        "owner",
        "2026-11-10T12:01:00Z",
    )
    .unwrap();

    let made = MadeFromIt::new();
    let d_item = delete(root_path, D, &made);

    // After trashing D: exp_one is retired, exp_two keeps meeting_090000_300 + m1 + owner-held title,
    // drops m2, and enqueues an OwnerAsk experience_compose!
    let after_trash = load_experiences(root_path, "work", DAY, false).unwrap();
    assert_eq!(after_trash.len(), 1);
    assert_eq!(after_trash[0].id, exp_two.id);
    assert_eq!(after_trash[0].title, "Owner held launch day");
    assert_eq!(after_trash[0].activities.len(), 1);
    assert_eq!(after_trash[0].activities[0].id, "meeting_090000_300");
    assert_eq!(after_trash[0].moments.len(), 1);
    assert_eq!(after_trash[0].moments[0].id, "m1");
    let asks = solstone_core_system::owner_ask::read(root_path).unwrap();
    assert!(
        asks.values().any(|ask| {
            ask.day == DAY
                && ask.experiences.iter().any(|e| {
                    e.facet == "work" && e.anchor_day == DAY && e.experience_id == exp_two.id
                })
        }),
        "expected OwnerAsk for surviving experience after trash cascade, got {asks:?}"
    );

    // Recover D -> both experiences return intact!
    crate::trash_kinds::with_kinds(root_path, |kinds| {
        solstone_core_trash::attach(root_path, kinds, &d_item.id, Action::Recover, surface())
    })
    .unwrap();
    let restored = load_experiences(root_path, "work", DAY, false).unwrap();
    assert_eq!(restored.len(), 2);
    assert!(restored.iter().any(|e| e.id == exp_one.id));
    let restored_two = restored.iter().find(|e| e.id == exp_two.id).unwrap();
    assert_eq!(restored_two.activities.len(), 2);
    assert_eq!(restored_two.moments.len(), 2);

    // Trash D again and erase it -> edit `before` history scrubbed on disk.
    let d_item_2 = delete(root_path, D, &made);
    crate::trash_kinds::with_kinds(root_path, |kinds| {
        solstone_core_trash::erase(root_path, kinds, &d_item_2.id, Action::Erase, surface())
    })
    .unwrap();
    let raw_after_erase =
        fs::read_to_string(root_path.join(format!("facets/work/experiences/{DAY}.jsonl"))).unwrap();
    assert!(
        !raw_after_erase.contains("Original summary"),
        "expected erased before-values scrubbed: {raw_after_erase}"
    );
}
