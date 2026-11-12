// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::collections::VecDeque;
const ID: &str = "not_1d3tmYTlCICgjy";
const REV: &str = "2026-11-01T09:10:00Z";
struct Fake {
    replies: VecDeque<(u16, Value)>,
    queries: Vec<(String, Vec<(&'static str, String)>)>,
}
impl Fake {
    fn new(replies: Vec<(u16, Value)>) -> Self {
        Self {
            replies: replies.into(),
            queries: Vec::new(),
        }
    }
}
impl Api for Fake {
    fn get(&mut self, path: &str, query: &[(&str, String)]) -> Result<(u16, Value), Error> {
        self.queries.push((
            path.into(),
            query
                .iter()
                .map(|(k, v)| {
                    (
                        match *k {
                            "page_size" => "page_size",
                            "updated_after" => "updated_after",
                            "cursor" => "cursor",
                            "include" => "include",
                            _ => panic!("unexpected parameter"),
                        },
                        v.clone(),
                    )
                })
                .collect(),
        ));
        Ok(self.replies.pop_front().expect("unexpected API request"))
    }
}
fn note(revision: &str) -> Value {
    // The documented Get Note shape, with the documented example note ID.
    json!({"id":ID,"title":"planning","created_at":"2026-11-01T08:58:00Z","updated_at":revision,"deleted_at":null,"web_url":"https://notes.granola.ai/d/f3e45e0f-24cc-480b-9a6c-8b1f5e3d7a2c","summary_text":"agreed a date","summary_markdown":null,"private_notes_text":"ask about funding","private_notes_markdown":null,"attendees":[{"name":"Robin","email":"r@example.org"}],"calendar_event":{"event_title":"weekly planning","scheduled_start_time":"2026-11-01T08:55:00Z"},"transcript":[{"text":"First words.","start_time":"2026-11-01T08:58:00Z","end_time":"2026-11-01T08:58:01Z","speaker":{"source":"microphone","attribution":"me"}},{"text":"Second words.","start_time":"2026-11-01T09:01:00Z","end_time":"2026-11-01T09:01:02Z","speaker":{"source":"microphone","diarization_label":"Speaker A"}}]})
}
fn page(revision: &str) -> Value {
    json!({"notes":[{"id":ID,"updated_at":revision,"deleted_at":null}],"hasMore":false,"cursor":null})
}
fn root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("imports")).unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/journal.json"),
        r#"{"identity":{"timezone":"America/Denver"}}"#,
    )
    .unwrap();
    root
}
fn now() -> DateTime<Utc> {
    instant("2026-11-02T10:00:00Z").unwrap()
}
fn output(root: &Path) -> PathBuf {
    let s = state(root).unwrap();
    directory(root, ID, &s.notes[ID]).unwrap().join(OUTPUT)
}
fn good(_: &Path, _: &str, _: &Placement, _: &Path) -> Result<(), Error> {
    Ok(())
}
fn run_once(root: &Path, note: Value, revision: &str) -> Result<(), Error> {
    sync_with(
        root,
        &mut Fake::new(vec![(200, page(revision)), (200, note)]),
        &mut good,
    )
}

#[test]
fn case_distinct_remote_ids_have_distinct_meetings_on_every_filesystem() {
    let root = root();
    let other_id = ID.to_ascii_lowercase();
    let mut other = note(REV);
    other["id"] = json!(other_id);
    other["transcript"][0]["text"] = json!("A different meeting.");
    let listing = json!({"notes":[{"id":ID,"updated_at":REV},{"id":other_id,"updated_at":REV}],"hasMore":false});
    sync_with(
        root.path(),
        &mut Fake::new(vec![(200, listing), (200, note(REV)), (200, other)]),
        &mut good,
    )
    .unwrap();
    let saved = state(root.path()).unwrap();
    assert_eq!(saved.notes.len(), 2);
    let first = directory(root.path(), ID, &saved.notes[ID])
        .unwrap()
        .join(OUTPUT);
    let second = directory(root.path(), &other_id, &saved.notes[&other_id])
        .unwrap()
        .join(OUTPUT);
    assert_ne!(
        stream(ID).to_ascii_lowercase(),
        stream(&other_id).to_ascii_lowercase()
    );
    assert!(fs::read_to_string(&first).unwrap().contains("First words."));
    assert!(
        fs::read_to_string(&second)
            .unwrap()
            .contains("A different meeting.")
    );
    assert_eq!(legacy_id(ID), Some(ID.into()));
    assert_ne!(legacy_id(ID), legacy_id(&other_id));
}

#[test]
fn edits_replace_one_meeting_and_null_transcripts_preserve_it() {
    let root = root();
    run_once(root.path(), note(REV), REV).unwrap();
    let output = output(root.path());
    let first = fs::read(&output).unwrap();
    let s = state(root.path()).unwrap();
    let p = &s.notes[ID];
    assert_eq!(p.day, "20261101");
    assert_eq!(p.segment, "015800_182");
    let rows = fs::read_to_string(&output).unwrap();
    assert!(rows.contains("Granola: me"));
    assert!(!rows.contains("\"You\""));
    let revision = "2026-11-02T09:30:00Z";
    let mut changed = note(revision);
    changed["summary_text"] = json!("a revised date");
    changed["transcript"][0]["text"] = json!("Changed words.");
    run_once(root.path(), changed, revision).unwrap();
    assert_eq!(output, super::tests::output(root.path()));
    assert_ne!(first, fs::read(&output).unwrap());
    assert_eq!(state(root.path()).unwrap().notes.len(), 1);
    let prior = fs::read(&output).unwrap();
    let watermark = state(root.path()).unwrap().watermark;
    let mut unavailable = note("2026-11-03T09:30:00Z");
    unavailable["transcript"] = Value::Null;
    assert_eq!(
        run_once(root.path(), unavailable, "2026-11-03T09:30:00Z"),
        Err(Error::Transcript)
    );
    assert_eq!(fs::read(&output).unwrap(), prior);
    assert_eq!(state(root.path()).unwrap().watermark, watermark);
    assert!(status(root.path()).unwrap()["attention"].is_string());
    let mut unchanged = Fake::new(vec![(200, page(revision))]);
    sync_with(root.path(), &mut unchanged, &mut good).unwrap();
    assert!(unchanged.replies.is_empty());
    assert!(status(root.path()).unwrap()["attention"].is_null());
}
#[test]
fn a_failed_publication_retries_the_same_placement_and_remote_deletion_keeps_it() {
    let root = root();
    let mut fail = |_: &Path, _: &str, _: &Placement, _: &Path| Err(Error::Storage);
    let mut api = Fake::new(vec![(200, page(REV)), (200, note(REV))]);
    assert_eq!(
        sync_with(root.path(), &mut api, &mut fail),
        Err(Error::Storage)
    );
    let path = output(root.path());
    assert!(path.is_file());
    assert!(state(root.path()).unwrap().watermark.is_none());
    assert!(state(root.path()).unwrap().notes[ID].revision.is_none());
    run_once(root.path(), note(REV), REV).unwrap();
    assert_eq!(path, output(root.path()));
    let before = fs::read(&path).unwrap();
    let mut deleted = page("2026-11-03T09:30:00Z");
    deleted["notes"][0]["deleted_at"] = json!("2026-11-03T09:30:00Z");
    sync_with(root.path(), &mut Fake::new(vec![(200, deleted)]), &mut good).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[test]
fn all_pages_and_large_transcripts_are_consumed_before_advancing() {
    let root = root();
    let mut oversized = note(REV);
    let turns = oversized["transcript"].take();
    let mut api = Fake::new(vec![
        (200, json!({"notes":[],"hasMore":true,"cursor":"next"})),
        (200, page(REV)),
        (413, Value::Null),
        (200, oversized),
        (
            200,
            json!({"transcript":[],"hasMore":true,"cursor":"turns"}),
        ),
        (
            200,
            json!({"transcript":turns,"hasMore":false,"cursor":null}),
        ),
    ]);
    sync_with(root.path(), &mut api, &mut good).unwrap();
    assert!(api.replies.is_empty());
    assert_eq!(api.queries[1].1.last().unwrap(), &("cursor", "next".into()));
    let mut next = Fake::new(vec![(
        200,
        json!({"notes":[],"hasMore":false,"cursor":null}),
    )]);
    sync_with(root.path(), &mut next, &mut good).unwrap();
    // An empty first page supplies no server clock boundary, so the next run revisits history.
    assert!(!next.queries[0].1.iter().any(|(k, _)| *k == "updated_after"));
    sync_with(
        root.path(),
        &mut Fake::new(vec![(200, page(REV))]),
        &mut good,
    )
    .unwrap();
    let mut incremental = Fake::new(vec![(
        200,
        json!({"notes":[],"hasMore":false,"cursor":null}),
    )]);
    sync_with(root.path(), &mut incremental, &mut good).unwrap();
    assert!(
        incremental.queries[0]
            .1
            .iter()
            .any(|(k, v)| *k == "updated_after" && v == "2026-11-01T09:08:00.000Z")
    );
    let old = state(root.path()).unwrap().watermark;
    for status in [401, 403, 429, 503] {
        let result = sync_with(
            root.path(),
            &mut Fake::new(vec![(status, Value::Null)]),
            &mut good,
        );
        assert!(result.is_err());
        assert_eq!(state(root.path()).unwrap().watermark, old);
        assert!(output(root.path()).is_file());
    }
}
#[test]
fn owner_removal_wins_an_interrupted_publication_and_reconnect() {
    let root = root();
    let mut remove = |_: &Path, _: &str, _: &Placement, output: &Path| {
        fs::write(output.parent().unwrap().join("tombstone.json"), "{}").unwrap();
        fs::remove_file(output).unwrap();
        Err(Error::Storage)
    };
    assert_eq!(
        sync_with(
            root.path(),
            &mut Fake::new(vec![(200, page(REV)), (200, note(REV))]),
            &mut remove
        ),
        Err(Error::Storage)
    );
    let path = output(root.path());
    run_once(root.path(), note(REV), REV).unwrap();
    assert!(!path.exists());
    assert!(path.parent().unwrap().join("tombstone.json").exists());
    {
        let _held = hold(root.path()).unwrap();
        set_key(root.path(), Some("grn_test_personal_key")).unwrap();
    }
    assert!(state(root.path()).unwrap().watermark.is_none());
    sync_with(
        root.path(),
        &mut Fake::new(vec![(200, page(REV))]),
        &mut good,
    )
    .unwrap();
    assert!(!path.exists());
    assert!(
        !fs::read_to_string(root.path().join(STATE))
            .unwrap()
            .contains("grn_test_personal_key")
    );
    assert!(
        !status(root.path())
            .unwrap()
            .to_string()
            .contains("grn_test_personal_key")
    );
}
#[test]
fn old_exact_ids_are_untouched_but_equal_titles_and_times_do_not_dedupe() {
    let root = root();
    let old = root
        .path()
        .join("chronicle/20261101/import.granola/015800_300");
    fs::create_dir_all(&old).unwrap();
    let bytes = r#"{"imported":{"id":"f3e45e0f-24cc-480b-9a6c-8b1f5e3d7a2c"}}
{"start":"01:58:00","text":"prior content"}
"#;
    fs::write(old.join("conversation_transcript.jsonl"), bytes).unwrap();
    run_once(root.path(), note(REV), REV).unwrap();
    assert_eq!(
        fs::read_to_string(old.join("conversation_transcript.jsonl")).unwrap(),
        bytes
    );
    assert!(!output(root.path()).exists());
    let mut distinct = note(REV);
    distinct["web_url"] = json!("https://notes.granola.ai/d/a-different-id");
    let new_id = "not_2d3tmYTlCICgjy";
    distinct["id"] = json!(new_id);
    let mut listing = page(REV);
    listing["notes"][0]["id"] = json!(new_id);
    sync_with(
        root.path(),
        &mut Fake::new(vec![(200, listing), (200, distinct)]),
        &mut good,
    )
    .unwrap();
    let s = state(root.path()).unwrap();
    assert!(
        directory(root.path(), new_id, &s.notes[new_id])
            .unwrap()
            .join(OUTPUT)
            .is_file()
    );
}
#[test]
fn production_publication_and_thinking_reader_expose_all_source_content() {
    let root = root();
    let mut api = Fake::new(vec![(200, page(REV)), (200, note(REV))]);
    sync_with(root.path(), &mut api, &mut publish).unwrap();
    let (text, _) = solstone_core_transcripts::cluster(
        root.path(),
        "20261101",
        &solstone_core_transcripts::Sources {
            transcripts: true,
            percepts: false,
            talents: solstone_core_transcripts::TalentSource::Disabled,
            voices: None,
            heard_owner: None,
            strava_tiles: false,
        },
    );
    for expected in [
        "First words.",
        "Second words.",
        "agreed a date",
        "ask about funding",
        "weekly planning",
        "r@example.org",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains("You:"));
}

#[test]
fn a_disconnect_prevents_the_live_transport_from_making_its_next_request() {
    let root = root();
    {
        let _held = hold(root.path()).unwrap();
        set_key(root.path(), Some("grn_test_transport_key")).unwrap();
    }
    let mut api = LiveApi::new(root.path().to_path_buf(), "grn_test_transport_key".into());
    set_key(root.path(), None).unwrap();
    assert_eq!(api.get("/v1/notes", &[]), Err(Error::Stopped));
}
#[test]
fn malformed_turns_do_not_overwrite_a_meeting() {
    let root = root();
    run_once(root.path(), note(REV), REV).unwrap();
    let path = output(root.path());
    let prior = fs::read(&path).unwrap();
    let mut bad = note("2026-11-03T09:30:00Z");
    bad["transcript"][0]["end_time"] = json!("not a date");
    assert_eq!(
        run_once(root.path(), bad, "2026-11-03T09:30:00Z"),
        Err(Error::Response)
    );
    assert_eq!(fs::read(&path).unwrap(), prior);
}

#[test]
fn a_note_without_spoken_turns_is_ready_for_thinking() {
    let root = root();
    let mut summary_only = note(REV);
    summary_only["transcript"] = json!([]);
    run_once(root.path(), summary_only, REV).unwrap();
    let saved = state(root.path()).unwrap();
    let p = &saved.notes[ID];
    let status = solstone_core_system_health::read_segment_data_state(
        root.path(),
        &p.day,
        &p.segment,
        Some(&stream(ID)),
        now(),
    );
    assert!(solstone_core_system_health::segment_fully_sensed(&status));
    assert_eq!(status.0.get("audio").map(String::as_str), Some("analyzed"));
    let (text, _) = solstone_core_transcripts::cluster(
        root.path(),
        "20261101",
        &solstone_core_transcripts::Sources {
            transcripts: true,
            percepts: false,
            talents: solstone_core_transcripts::TalentSource::Disabled,
            voices: None,
            heard_owner: None,
            strava_tiles: false,
        },
    );
    assert!(text.contains("agreed a date"));
    assert!(!text.contains("First words."));
}
