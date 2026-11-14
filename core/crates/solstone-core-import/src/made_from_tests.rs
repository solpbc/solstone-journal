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
